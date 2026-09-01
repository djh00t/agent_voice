pub use crate::pa::oauth_callback::{AuthorizationCode, OAuthCallback, validate_callback};
pub use crate::pa::oauth_start::{
    InMemoryOAuthStateStore, OAuthError, OAuthResult, OAuthStart, OAuthStateStore, SecureRandom,
    begin,
};

#[cfg(test)]
mod tests {
    #[test]
    fn module_surface() {
        let _: fn(
            crate::config::OAuthProvider,
            &crate::config::OAuthProviderConfig,
            &mut dyn crate::pa::oauth::OAuthStateStore,
            &dyn crate::pa::oauth::SecureRandom,
            chrono::DateTime<chrono::Utc>,
        ) -> crate::pa::oauth::OAuthResult<crate::pa::oauth::OAuthStart> = crate::pa::oauth::begin;
        let _: fn(
            &mut dyn crate::pa::oauth::OAuthStateStore,
            crate::pa::oauth::OAuthCallback,
            chrono::DateTime<chrono::Utc>,
        ) -> crate::pa::oauth::OAuthResult<crate::pa::oauth::AuthorizationCode> =
            crate::pa::oauth::validate_callback;
        let _: Option<crate::pa::oauth::OAuthError> = None;
        let _: Option<crate::pa::oauth::OAuthStart> = None;
        let _: Option<crate::pa::oauth::OAuthCallback> = None;
        let _: Option<crate::pa::oauth::InMemoryOAuthStateStore> = None;
        let _: for<'a> fn(&'a crate::pa::oauth::AuthorizationCode) -> &'a str =
            crate::pa::oauth::AuthorizationCode::as_str;
    }
}
