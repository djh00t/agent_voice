//! OAuth callback validation and single-use state consumption.

use std::fmt;

use chrono::{DateTime, Utc};

use crate::pa::oauth_start::OAuthError;

/// Untrusted values received from an OAuth provider callback.
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
}

impl fmt::Debug for OAuthCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthCallback")
            .field("code", &"<redacted>")
            .field("state", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for OAuthCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// Opaque authorization code and consumed PKCE verifier for token exchange.
pub struct AuthorizationCode {
    code: String,
    verifier: String,
}

impl AuthorizationCode {
    /// Returns the exact authorization code for the token-exchange boundary.
    pub fn as_str(&self) -> &str {
        &self.code
    }

    /// Returns the consumed PKCE verifier to the in-crate token-exchange boundary.
    #[allow(dead_code)]
    pub(crate) fn verifier(&self) -> &str {
        &self.verifier
    }
}

impl fmt::Debug for AuthorizationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorizationCode(<redacted>)")
    }
}

impl fmt::Display for AuthorizationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// Validates callback input and consumes its reserved state exactly once.
pub fn validate_callback(
    state_store: &mut dyn crate::pa::oauth_start::OAuthStateStore,
    callback: OAuthCallback,
    now: DateTime<Utc>,
) -> crate::pa::oauth_start::OAuthResult<AuthorizationCode> {
    if callback.code.trim().is_empty() {
        return Err(OAuthError::InvalidCode);
    }

    let verifier = state_store.consume(&callback.state, now)?;
    Ok(AuthorizationCode {
        code: callback.code,
        verifier,
    })
}
