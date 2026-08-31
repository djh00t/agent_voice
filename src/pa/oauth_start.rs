//! Deterministic OAuth authorization-start and state-store contracts.

use std::collections::HashMap;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use ring::digest;

use crate::config::{OAuthProvider, OAuthProviderConfig};

/// Result returned by authorization-start and state-store operations.
pub type OAuthResult<T> = Result<T, OAuthError>;

/// Fixed, non-sensitive errors for the OAuth start boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthError {
    InvalidState,
    StateAlreadyUsed,
    StateExpired,
    InvalidCode,
    InvalidProvider,
    RandomnessFailure,
    StateStoreFailure,
    InvalidConfiguration,
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidState => "invalid OAuth state",
            Self::StateAlreadyUsed => "OAuth state already used",
            Self::StateExpired => "OAuth state expired",
            Self::InvalidCode => "invalid OAuth code",
            Self::InvalidProvider => "invalid OAuth provider",
            Self::RandomnessFailure => "OAuth randomness failure",
            Self::StateStoreFailure => "OAuth state-store failure",
            Self::InvalidConfiguration => "invalid OAuth configuration",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OAuthError {}

/// Injectable source of cryptographically secure random bytes.
pub trait SecureRandom {
    fn fill(&self, bytes: &mut [u8]) -> OAuthResult<()>;
}

/// Single-use OAuth state persistence boundary used by the callback child.
pub trait OAuthStateStore {
    fn reserve(
        &mut self,
        state: &str,
        verifier: &str,
        expires_at: DateTime<Utc>,
    ) -> OAuthResult<()>;

    fn consume(&mut self, state: &str, now: DateTime<Utc>) -> OAuthResult<String>;

    /// Removes entries whose replay window has ended.
    fn purge_expired(&mut self, _now: DateTime<Utc>) {}
}

struct StateEntry {
    verifier: String,
    expires_at: DateTime<Utc>,
    consumed: bool,
}

/// Deterministic in-memory state store for tests and process-local callers.
#[derive(Default)]
pub struct InMemoryOAuthStateStore {
    entries: HashMap<String, StateEntry>,
}

impl InMemoryOAuthStateStore {
    /// Creates an empty state store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl fmt::Debug for InMemoryOAuthStateStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryOAuthStateStore")
            .field("entry_count", &self.entries.len())
            .finish()
    }
}

impl OAuthStateStore for InMemoryOAuthStateStore {
    fn reserve(
        &mut self,
        state: &str,
        verifier: &str,
        expires_at: DateTime<Utc>,
    ) -> OAuthResult<()> {
        if state.trim().is_empty() {
            return Err(OAuthError::InvalidState);
        }
        if verifier.trim().is_empty() {
            return Err(OAuthError::InvalidCode);
        }
        if self.entries.contains_key(state) {
            return Err(OAuthError::StateAlreadyUsed);
        }

        self.entries.insert(
            state.to_owned(),
            StateEntry {
                verifier: verifier.to_owned(),
                expires_at,
                consumed: false,
            },
        );
        Ok(())
    }

    fn consume(&mut self, state: &str, now: DateTime<Utc>) -> OAuthResult<String> {
        if state.trim().is_empty() {
            return Err(OAuthError::InvalidState);
        }

        let entry = self
            .entries
            .get_mut(state)
            .ok_or(OAuthError::InvalidState)?;
        if entry.consumed {
            return Err(OAuthError::StateAlreadyUsed);
        }
        if now >= entry.expires_at {
            return Err(OAuthError::StateExpired);
        }

        entry.consumed = true;
        Ok(std::mem::take(&mut entry.verifier))
    }

    fn purge_expired(&mut self, now: DateTime<Utc>) {
        self.entries.retain(|_, entry| entry.expires_at > now);
    }
}

/// Values returned to the caller that needs to redirect a browser.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthStart {
    pub state: String,
    pub code_verifier: String,
    pub authorize_url: reqwest::Url,
}

impl fmt::Debug for OAuthStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthStart")
            .field("state", &"<redacted>")
            .field("code_verifier", &"<redacted>")
            .field("authorize_url", &"<redacted>")
            .finish()
    }
}

/// Creates an authorization URL and reserves its single-use PKCE state.
pub fn begin(
    provider: OAuthProvider,
    config: &OAuthProviderConfig,
    state_store: &mut dyn OAuthStateStore,
    nonce_source: &dyn SecureRandom,
    now: DateTime<Utc>,
) -> OAuthResult<OAuthStart> {
    let client_id = config
        .require_client_id(provider)
        .map_err(|_| OAuthError::InvalidConfiguration)?;
    if config.authorize_url.query().is_some() || config.authorize_url.fragment().is_some() {
        return Err(OAuthError::InvalidConfiguration);
    }
    if config.scopes.is_empty() || config.scopes.iter().any(|scope| scope.trim().is_empty()) {
        return Err(OAuthError::InvalidConfiguration);
    }
    let expires_at = now
        .checked_add_signed(Duration::minutes(10))
        .ok_or(OAuthError::InvalidConfiguration)?;

    let state = random_base64url_32(nonce_source)?;
    let code_verifier = random_base64url_32(nonce_source)?;
    let code_challenge =
        URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, code_verifier.as_bytes()));
    let authorize_url = build_authorize_url(
        provider,
        &config.authorize_url,
        client_id,
        config.redirect_uri.as_str(),
        &config.scopes.join(" "),
        &state,
        &code_challenge,
    )?;

    state_store.reserve(&state, &code_verifier, expires_at)?;
    state_store.purge_expired(now);
    Ok(OAuthStart {
        state,
        code_verifier,
        authorize_url,
    })
}

fn random_base64url_32(nonce_source: &dyn SecureRandom) -> OAuthResult<String> {
    let mut bytes = [0u8; 32];
    nonce_source
        .fill(&mut bytes)
        .map_err(|_| OAuthError::RandomnessFailure)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn build_authorize_url(
    provider: OAuthProvider,
    authorize_url: &reqwest::Url,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    code_challenge: &str,
) -> OAuthResult<reqwest::Url> {
    let mut url = authorize_url.clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", scope)
            .append_pair("state", state)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256");
        if provider == OAuthProvider::Google {
            query.append_pair("access_type", "offline");
        }
    }
    Ok(url)
}
