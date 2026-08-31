#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;

#[allow(dead_code)]
#[path = "../src/pa/oauth_start.rs"]
pub mod oauth_start;

mod pa {
    pub use crate::oauth_start;
}

#[allow(dead_code)]
#[path = "../src/pa/oauth_callback.rs"]
mod oauth_callback;

use chrono::{DateTime, Duration, Utc};

use oauth_callback::OAuthCallback;
use oauth_start::{InMemoryOAuthStateStore, OAuthError, OAuthResult, OAuthStateStore};

const NOW: DateTime<Utc> = DateTime::from_timestamp(1_725_000_000, 0).unwrap();
const EXPECTED_CODE: &str = "authorization-code with spaces";
const EXPECTED_VERIFIER: &str = "reserved-verifier";
const CALLBACK_SENTINEL: &str = "authorization-code-secret";

fn callback(code: &str, state: &str) -> OAuthCallback {
    OAuthCallback {
        code: code.to_owned(),
        state: state.to_owned(),
    }
}

fn assert_oauth_error<T>(result: OAuthResult<T>, expected: OAuthError) {
    assert!(
        matches!(result, Err(error) if error == expected),
        "unexpected OAuth error"
    );
}

fn reserved_store() -> InMemoryOAuthStateStore {
    let mut store = InMemoryOAuthStateStore::default();
    store
        .reserve(
            "reserved-state",
            "reserved-verifier",
            NOW + Duration::minutes(10),
        )
        .expect("fixture reservation");
    store
}

#[test]
fn valid_callback_returns_exact_code_and_consumes_state_once() {
    let mut store = reserved_store();
    let result = oauth_callback::validate_callback(
        &mut store,
        callback(EXPECTED_CODE, "reserved-state"),
        NOW,
    );
    let authorization_code = result.expect("valid callback");
    assert!(
        authorization_code.as_str() == EXPECTED_CODE,
        "authorization code was not preserved"
    );

    assert_oauth_error(
        oauth_callback::validate_callback(
            &mut store,
            callback(EXPECTED_CODE, "reserved-state"),
            NOW,
        ),
        OAuthError::StateAlreadyUsed,
    );
}

#[test]
fn valid_callback_preserves_consumed_verifier_for_token_exchange() {
    let mut store = reserved_store();
    let authorization_code = oauth_callback::validate_callback(
        &mut store,
        callback(EXPECTED_CODE, "reserved-state"),
        NOW,
    )
    .expect("valid callback");

    assert_eq!(
        authorization_code.verifier(),
        EXPECTED_VERIFIER,
        "consumed PKCE verifier was not preserved"
    );
}

#[test]
fn callback_rejects_blank_code_before_touching_state_store() {
    let mut store = CountingStore::default();
    assert_oauth_error(
        oauth_callback::validate_callback(&mut store, callback(" \t\n", "reserved-state"), NOW),
        OAuthError::InvalidCode,
    );
    assert_eq!(store.consume_calls, 0, "blank code touched the state store");
}

#[test]
fn callback_rejects_unknown_state() {
    let mut store = InMemoryOAuthStateStore::default();
    assert_oauth_error(
        oauth_callback::validate_callback(
            &mut store,
            callback("authorization-code", "unknown-state"),
            NOW,
        ),
        OAuthError::InvalidState,
    );
}

#[test]
fn callback_rejects_mismatched_state() {
    let mut store = reserved_store();
    assert_oauth_error(
        oauth_callback::validate_callback(
            &mut store,
            callback("authorization-code", "mismatched-state"),
            NOW,
        ),
        OAuthError::InvalidState,
    );

    let result = oauth_callback::validate_callback(
        &mut store,
        callback("authorization-code", "reserved-state"),
        NOW,
    );
    assert!(
        result.is_ok(),
        "mismatched state consumed the valid reservation"
    );
}

#[test]
fn callback_rejects_expired_state_without_consuming_it() {
    let mut store = reserved_store();
    assert_oauth_error(
        oauth_callback::validate_callback(
            &mut store,
            callback("authorization-code", "reserved-state"),
            NOW + Duration::minutes(10),
        ),
        OAuthError::StateExpired,
    );

    let result = oauth_callback::validate_callback(
        &mut store,
        callback("authorization-code", "reserved-state"),
        NOW + Duration::minutes(9),
    );
    assert!(result.is_ok(), "expired callback consumed the reservation");
}

#[test]
fn callback_propagates_store_failure_without_exposing_callback_values() {
    let mut store = CountingStore {
        error: OAuthError::StateStoreFailure,
        ..CountingStore::default()
    };
    let result = oauth_callback::validate_callback(
        &mut store,
        callback(CALLBACK_SENTINEL, "reserved-state"),
        NOW,
    );
    assert_oauth_error(result, OAuthError::StateStoreFailure);
    assert_eq!(store.consume_calls, 1, "valid code skipped the state store");
}

#[test]
fn authorization_code_debug_and_display_are_fixed_and_redacted() {
    let mut store = reserved_store();
    let authorization_code = oauth_callback::validate_callback(
        &mut store,
        callback(CALLBACK_SENTINEL, "reserved-state"),
        NOW,
    )
    .expect("valid callback");

    let debug = format!("{authorization_code:?}");
    let display = format!("{authorization_code}");
    assert!(
        debug == "AuthorizationCode(<redacted>)",
        "authorization-code debug output changed"
    );
    assert!(
        display == "<redacted>",
        "authorization-code display output changed"
    );
    assert!(
        !debug.contains(CALLBACK_SENTINEL),
        "debug output leaked the code"
    );
    assert!(
        !display.contains(CALLBACK_SENTINEL),
        "display output leaked the code"
    );
    assert!(
        !debug.contains(EXPECTED_VERIFIER),
        "debug output leaked the PKCE verifier"
    );
    assert!(
        !display.contains(EXPECTED_VERIFIER),
        "display output leaked the PKCE verifier"
    );

    let callback_value = callback(CALLBACK_SENTINEL, "state-secret");
    let callback_debug = format!("{callback_value:?}");
    let callback_display = format!("{callback_value}");
    assert!(
        callback_debug == "OAuthCallback { code: \"<redacted>\", state: \"<redacted>\" }",
        "callback debug output changed"
    );
    assert!(
        callback_display == "<redacted>",
        "callback display output changed"
    );
    assert!(
        !callback_debug.contains(CALLBACK_SENTINEL),
        "callback debug output leaked a value"
    );
    assert!(
        !callback_display.contains(CALLBACK_SENTINEL),
        "callback display output leaked a value"
    );

    let error = OAuthError::InvalidState;
    assert!(
        format!("{error:?} {error}") == "InvalidState invalid OAuth state",
        "OAuth error output changed"
    );
}

struct CountingStore {
    consume_calls: usize,
    error: OAuthError,
}

impl Default for CountingStore {
    fn default() -> Self {
        Self {
            consume_calls: 0,
            error: OAuthError::StateStoreFailure,
        }
    }
}

impl OAuthStateStore for CountingStore {
    fn reserve(
        &mut self,
        _state: &str,
        _verifier: &str,
        _expires_at: DateTime<Utc>,
    ) -> OAuthResult<()> {
        Err(self.error)
    }

    fn consume(&mut self, _state: &str, _now: DateTime<Utc>) -> OAuthResult<String> {
        self.consume_calls += 1;
        Err(self.error)
    }
}
