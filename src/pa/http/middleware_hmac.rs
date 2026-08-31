//! Bounded HMAC admission for protected PA HTTP requests.
//!
//! The adapter owns request-body buffering and authentication admission only.
//! It does not generate request IDs, validate idempotency keys, invoke routes,
//! or render HTTP responses.

use std::fmt;
use std::sync::Arc;

use http::HeaderMap;

use crate::pa::auth::{self, AuthError, ReplayGuard};
use crate::pa::store::PaStore;

use super::ApiError;

/// Maximum request body size admitted by the protected-request boundary.
pub const MAX_HTTP_REQUEST_BODY_BYTES: usize = 64 * 1024;

/// A cloneable state bundle for the HMAC admission adapter.
///
/// The secret is supplied by validated runtime configuration. It is never
/// read from a request or an ad hoc environment lookup.
#[derive(Clone)]
pub struct HmacAdmissionState {
    store: Arc<PaStore>,
    secret: Arc<[u8]>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync + 'static>,
}

impl HmacAdmissionState {
    /// Creates state from the existing PA store, validated secret, and clock.
    pub fn new<S>(
        store: Arc<PaStore>,
        secret: S,
        clock: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self
    where
        S: Into<Arc<[u8]>>,
    {
        Self {
            store,
            secret: secret.into(),
            clock: Arc::new(clock),
        }
    }
}

impl fmt::Debug for HmacAdmissionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("HmacAdmissionState").finish()
    }
}

/// A successful, opaque handoff from authentication to the integration layer.
pub struct AuthenticatedRequest {
    body: Vec<u8>,
    authenticated: AuthenticatedMarker,
}

impl AuthenticatedRequest {
    /// Returns the exact body bytes buffered during admission.
    pub fn body(&self) -> &[u8] {
        let _ = &self.authenticated;
        &self.body
    }

    /// Transfers the exact body bytes to the downstream request builder.
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }
}

impl fmt::Debug for AuthenticatedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedRequest")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy)]
struct AuthenticatedMarker;

/// Result of bounded HMAC admission.
pub enum HmacAdmissionResult {
    /// Authentication succeeded and the unchanged body is available.
    Accepted(AuthenticatedRequest),
    /// Authentication or bounded-body admission failed with a fixed envelope.
    Rejected(ApiError),
}

impl fmt::Debug for HmacAdmissionResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted(_) => formatter.write_str("HmacAdmissionResult::Accepted"),
            Self::Rejected(error) => formatter
                .debug_tuple("HmacAdmissionResult::Rejected")
                .field(error)
                .finish(),
        }
    }
}

/// Authenticates one protected PA request against a persistent replay store.
#[derive(Clone)]
pub struct HmacAdmission {
    state: HmacAdmissionState,
}

impl HmacAdmission {
    /// Creates an admission adapter from shared state.
    pub fn new(state: HmacAdmissionState) -> Self {
        Self { state }
    }

    /// Admits a request using the injected production clock.
    pub fn admit<M, B>(
        &self,
        method: M,
        path_and_query: &str,
        body: B,
        headers: &HeaderMap,
        request_id: &str,
    ) -> HmacAdmissionResult
    where
        M: AsRef<[u8]>,
        B: AsRef<[u8]>,
    {
        self.admit_at(
            method,
            path_and_query,
            body,
            headers,
            request_id,
            (self.state.clock)(),
        )
    }

    /// Admits a request at a fixed timestamp for deterministic test seams.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit_at<M, B>(
        &self,
        method: M,
        path_and_query: &str,
        body: B,
        headers: &HeaderMap,
        request_id: &str,
        now: i64,
    ) -> HmacAdmissionResult
    where
        M: AsRef<[u8]>,
        B: AsRef<[u8]>,
    {
        let body = match buffer_body(body.as_ref()) {
            Some(body) => body,
            None => {
                return HmacAdmissionResult::Rejected(ApiError::new(
                    "request_body_too_large",
                    "request body is too large",
                    request_id.to_owned(),
                ));
            }
        };
        let method = match std::str::from_utf8(method.as_ref()) {
            Ok(method) => method,
            Err(_) => return rejected_authentication(request_id),
        };

        let timestamp = match required_header(headers, auth::X_AV_TIMESTAMP) {
            Ok(value) => value,
            Err(HeaderCardinality::Missing) => return rejected_required(request_id),
            Err(HeaderCardinality::Duplicate) => return rejected_authentication(request_id),
        };
        let nonce = match required_header(headers, auth::X_AV_NONCE) {
            Ok(value) => value,
            Err(HeaderCardinality::Missing) => return rejected_required(request_id),
            Err(HeaderCardinality::Duplicate) => return rejected_authentication(request_id),
        };
        let signature = match required_header(headers, auth::X_AV_SIGNATURE) {
            Ok(value) => value,
            Err(HeaderCardinality::Missing) => return rejected_required(request_id),
            Err(HeaderCardinality::Duplicate) => return rejected_authentication(request_id),
        };

        let mut replay_guard = StoreReplayGuard::new(self.state.store.as_ref());
        match auth::verify_request(
            self.state.secret.as_ref(),
            method,
            path_and_query,
            timestamp,
            nonce,
            signature,
            &body,
            now,
            &mut replay_guard,
        ) {
            Ok(()) => HmacAdmissionResult::Accepted(AuthenticatedRequest {
                body,
                authenticated: AuthenticatedMarker,
            }),
            Err(AuthError::NonceReplay) if replay_guard.persistence_failed() => {
                rejected_unavailable(request_id)
            }
            Err(AuthError::NonceReplay) => rejected_replay(request_id),
            Err(_) if replay_guard.persistence_failed() => rejected_unavailable(request_id),
            Err(_) => rejected_authentication(request_id),
        }
    }
}

impl fmt::Debug for HmacAdmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("HmacAdmission").finish()
    }
}

fn buffer_body(body: &[u8]) -> Option<Vec<u8>> {
    if body.len() > MAX_HTTP_REQUEST_BODY_BYTES {
        return None;
    }
    Some(body.to_vec())
}

fn required_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<&'a [u8], HeaderCardinality> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or(HeaderCardinality::Missing)?;
    if values.next().is_some() {
        return Err(HeaderCardinality::Duplicate);
    }
    Ok(value.as_bytes())
}

#[derive(Debug, Clone, Copy)]
enum HeaderCardinality {
    Missing,
    Duplicate,
}

struct StoreReplayGuard<'a> {
    store: &'a PaStore,
    persistence_failed: bool,
}

impl<'a> StoreReplayGuard<'a> {
    fn new(store: &'a PaStore) -> Self {
        Self {
            store,
            persistence_failed: false,
        }
    }

    fn persistence_failed(&self) -> bool {
        self.persistence_failed
    }
}

impl ReplayGuard for StoreReplayGuard<'_> {
    fn check_and_record(&mut self, nonce: &str, now: i64) -> bool {
        match self.store.consume_replay_nonce(nonce, now) {
            Ok(consumed) => consumed,
            Err(_) => {
                self.persistence_failed = true;
                false
            }
        }
    }
}

fn rejected_required(request_id: &str) -> HmacAdmissionResult {
    HmacAdmissionResult::Rejected(ApiError::new(
        "authentication_required",
        "authentication required",
        request_id.to_owned(),
    ))
}

fn rejected_authentication(request_id: &str) -> HmacAdmissionResult {
    HmacAdmissionResult::Rejected(ApiError::new(
        "authentication_failed",
        "authentication failed",
        request_id.to_owned(),
    ))
}

fn rejected_replay(request_id: &str) -> HmacAdmissionResult {
    HmacAdmissionResult::Rejected(ApiError::new(
        "authentication_replay",
        "authentication replay detected",
        request_id.to_owned(),
    ))
}

fn rejected_unavailable(request_id: &str) -> HmacAdmissionResult {
    HmacAdmissionResult::Rejected(ApiError::new(
        "middleware_unavailable",
        "middleware is unavailable",
        request_id.to_owned(),
    ))
}
