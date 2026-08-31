#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/pa/oauth_start.rs"]
mod oauth_start;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use ring::digest;

use config::{OAuthProvider, PaOAuthConfig};
use oauth_start::{
    InMemoryOAuthStateStore, OAuthError, OAuthResult, OAuthStart, OAuthStateStore, SecureRandom,
};

const NOW: DateTime<Utc> = DateTime::from_timestamp(1_725_000_000, 0).unwrap();

struct DeterministicRandom {
    chunks: RefCell<VecDeque<Vec<u8>>>,
    calls: Cell<usize>,
    fail_at: Option<usize>,
}

impl DeterministicRandom {
    fn new(chunks: impl IntoIterator<Item = [u8; 32]>) -> Self {
        Self {
            chunks: RefCell::new(chunks.into_iter().map(|chunk| chunk.to_vec()).collect()),
            calls: Cell::new(0),
            fail_at: None,
        }
    }

    fn failing_after(call: usize) -> Self {
        Self {
            chunks: RefCell::new(VecDeque::from([vec![0u8; 32]])),
            calls: Cell::new(0),
            fail_at: Some(call),
        }
    }

    fn calls(&self) -> usize {
        self.calls.get()
    }
}

impl SecureRandom for DeterministicRandom {
    fn fill(&self, bytes: &mut [u8]) -> OAuthResult<()> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if self.fail_at == Some(call) {
            return Err(OAuthError::RandomnessFailure);
        }
        let Some(chunk) = self.chunks.borrow_mut().pop_front() else {
            return Err(OAuthError::RandomnessFailure);
        };
        if chunk.len() != bytes.len() {
            return Err(OAuthError::RandomnessFailure);
        }
        bytes.copy_from_slice(&chunk);
        Ok(())
    }
}

fn now_plus(minutes: i64) -> DateTime<Utc> {
    NOW + Duration::minutes(minutes)
}

fn config_with_id(provider: OAuthProvider, client_id: &str) -> PaOAuthConfig {
    let mut config = PaOAuthConfig::default();
    match provider {
        OAuthProvider::Microsoft => config.microsoft.client_id = Some(client_id.to_owned()),
        OAuthProvider::Google => config.google.client_id = Some(client_id.to_owned()),
    }
    config
}

fn query_pairs(url: &reqwest::Url) -> Vec<(String, String)> {
    url.query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn assert_base64url_32_bytes(value: &str) {
    assert_eq!(value.len(), 43);
    assert!(!value.contains('='));
    assert!(value.bytes().all(|byte| {
        byte.is_ascii_uppercase()
            || byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || byte == b'-'
            || byte == b'_'
    }));
    assert_eq!(URL_SAFE_NO_PAD.decode(value).unwrap().len(), 32);
}

fn expected_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()))
}

#[test]
fn pkce_url_contract() {
    for (provider, access_type) in [
        (OAuthProvider::Microsoft, None),
        (OAuthProvider::Google, Some("offline")),
    ] {
        let config = config_with_id(provider, "client-id");
        let random = DeterministicRandom::new([[0x11; 32], [0x22; 32]]);
        let mut store = InMemoryOAuthStateStore::default();
        let start = oauth_start::begin(
            provider,
            config.for_provider(provider),
            &mut store,
            &random,
            NOW,
        )
        .expect("validated config starts authorization");

        let expected_state = URL_SAFE_NO_PAD.encode([0x11; 32]);
        let expected_verifier = URL_SAFE_NO_PAD.encode([0x22; 32]);
        assert!(start.state == expected_state, "state encoding mismatch");
        assert!(
            start.code_verifier == expected_verifier,
            "code-verifier encoding mismatch"
        );
        assert_base64url_32_bytes(&start.state);
        assert_base64url_32_bytes(&start.code_verifier);
        assert_eq!(random.calls(), 2);

        let expected_redirect = config
            .for_provider(provider)
            .redirect_uri
            .as_str()
            .to_owned();
        let expected_scope = config.for_provider(provider).scopes.join(" ");
        let challenge = expected_challenge(&expected_verifier);
        let mut expected = vec![
            ("response_type".to_owned(), "code".to_owned()),
            ("client_id".to_owned(), "client-id".to_owned()),
            ("redirect_uri".to_owned(), expected_redirect),
            ("scope".to_owned(), expected_scope),
            ("state".to_owned(), expected_state),
            ("code_challenge".to_owned(), challenge),
            ("code_challenge_method".to_owned(), "S256".to_owned()),
        ];
        if let Some(access_type) = access_type {
            expected.push(("access_type".to_owned(), access_type.to_owned()));
        }
        assert!(
            query_pairs(&start.authorize_url) == expected,
            "authorization query contract mismatch"
        );
        assert!(!start.authorize_url.as_str().contains(&start.code_verifier));
        assert_eq!(start.authorize_url.fragment(), None);
        assert_eq!(start.authorize_url.query_pairs().count(), expected.len());

        let verifier = store
            .consume(&start.state, now_plus(1))
            .expect("reserved verifier can be consumed");
        assert_eq!(verifier, expected_verifier);
    }
}

#[test]
fn redirect_uri_is_preserved_and_only_authorize_parameters_are_emitted() {
    let mut config = config_with_id(OAuthProvider::Google, "client-id");
    config.google.redirect_uri =
        reqwest::Url::parse("https://app.example.test/callback?next=a%20b").unwrap();
    config.google.token_url =
        reqwest::Url::parse("https://token.sentinel.test/never-read").unwrap();
    let random = DeterministicRandom::new([[0x31; 32], [0x32; 32]]);
    let mut store = InMemoryOAuthStateStore::default();

    let start = oauth_start::begin(
        OAuthProvider::Google,
        &config.google,
        &mut store,
        &random,
        NOW,
    )
    .expect("valid URL-only start");
    let pairs = query_pairs(&start.authorize_url);

    assert!(
        pairs
            .iter()
            .find(|(key, _)| key == "redirect_uri")
            .map(|(_, value)| value.as_str())
            == Some(config.google.redirect_uri.as_str()),
        "redirect URI query value was not preserved"
    );
    assert!(!pairs.iter().any(|(key, _)| key == "token_url"));
    assert!(!start.authorize_url.as_str().contains("never-read"));
    assert!(pairs.iter().all(|(key, _)| {
        matches!(
            key.as_str(),
            "response_type"
                | "client_id"
                | "redirect_uri"
                | "scope"
                | "state"
                | "code_challenge"
                | "code_challenge_method"
                | "access_type"
        )
    }));
}

#[test]
fn prequeried_or_fragmented_authorize_url_fails_before_randomness_or_store_mutation() {
    for authorize_url in [
        "https://accounts.example.test/authorize?already=present",
        "https://accounts.example.test/authorize#fragment",
    ] {
        let mut config = config_with_id(OAuthProvider::Microsoft, "client-id");
        config.microsoft.authorize_url = reqwest::Url::parse(authorize_url).unwrap();
        let random = DeterministicRandom::failing_after(0);
        let mut store = InMemoryOAuthStateStore::default();

        let result = oauth_start::begin(
            OAuthProvider::Microsoft,
            &config.microsoft,
            &mut store,
            &random,
            NOW,
        );

        assert_eq!(result, Err(OAuthError::InvalidConfiguration));
        assert_eq!(random.calls(), 0);
        assert_eq!(
            store.consume(&URL_SAFE_NO_PAD.encode([0x41; 32]), NOW),
            Err(OAuthError::InvalidState)
        );
    }
}

#[test]
fn missing_client_id_and_expiry_overflow_fail_before_randomness_or_reservation() {
    let config = PaOAuthConfig::default();
    let random = DeterministicRandom::failing_after(0);
    let mut store = InMemoryOAuthStateStore::default();
    let result = oauth_start::begin(
        OAuthProvider::Microsoft,
        &config.microsoft,
        &mut store,
        &random,
        NOW,
    );
    assert_eq!(result, Err(OAuthError::InvalidConfiguration));
    assert_eq!(random.calls(), 0);

    let config = config_with_id(OAuthProvider::Microsoft, "client-id");
    let random = DeterministicRandom::new([[0x51; 32], [0x52; 32]]);
    let mut store = InMemoryOAuthStateStore::default();
    let result = oauth_start::begin(
        OAuthProvider::Microsoft,
        &config.microsoft,
        &mut store,
        &random,
        DateTime::<Utc>::MAX_UTC,
    );
    assert_eq!(result, Err(OAuthError::InvalidConfiguration));
    assert_eq!(random.calls(), 0);
    assert_eq!(
        store.consume(&URL_SAFE_NO_PAD.encode([0x51; 32]), NOW),
        Err(OAuthError::InvalidState)
    );
}

#[test]
fn state_store_is_single_use_expiring_and_failure_atomic() {
    let mut store = InMemoryOAuthStateStore::default();
    let expires_at = now_plus(10);

    assert_eq!(
        store.reserve("", "verifier", expires_at),
        Err(OAuthError::InvalidState)
    );
    assert_eq!(
        store.reserve("state", "", expires_at),
        Err(OAuthError::InvalidCode)
    );
    store
        .reserve("state", "verifier", expires_at)
        .expect("first reservation");
    assert_eq!(
        store.reserve("state", "replacement", expires_at),
        Err(OAuthError::StateAlreadyUsed)
    );
    assert_eq!(store.consume("", NOW), Err(OAuthError::InvalidState));
    assert_eq!(store.consume("unknown", NOW), Err(OAuthError::InvalidState));
    assert_eq!(
        store.consume("state", expires_at),
        Err(OAuthError::StateExpired)
    );
    assert!(
        matches!(
            store.consume("state", expires_at - Duration::seconds(1)),
            Ok(verifier) if verifier == "verifier"
        ),
        "reserved verifier was not returned"
    );
    assert_eq!(
        store.consume("state", NOW),
        Err(OAuthError::StateAlreadyUsed)
    );
}

#[test]
fn state_store_purges_expired_entries_past_the_replay_window() {
    let mut store = InMemoryOAuthStateStore::default();
    store
        .reserve("expired", "discard-me", NOW)
        .expect("expired fixture reservation");
    store.purge_expired(NOW);

    assert!(
        matches!(store.consume("expired", NOW), Err(OAuthError::InvalidState)),
        "expired entry remained after purge"
    );
}

#[test]
fn begin_failure_from_duplicate_state_does_not_replace_existing_verifier() {
    let config = config_with_id(OAuthProvider::Microsoft, "client-id");
    let random = DeterministicRandom::new([[0x61; 32], [0x62; 32], [0x61; 32], [0x62; 32]]);
    let mut store = InMemoryOAuthStateStore::default();
    let first = oauth_start::begin(
        OAuthProvider::Microsoft,
        &config.microsoft,
        &mut store,
        &random,
        NOW,
    )
    .expect("first start");
    assert_eq!(
        oauth_start::begin(
            OAuthProvider::Microsoft,
            &config.microsoft,
            &mut store,
            &random,
            NOW,
        ),
        Err(OAuthError::StateAlreadyUsed)
    );
    assert!(
        matches!(
            store.consume(&first.state, NOW),
            Ok(verifier) if verifier == first.code_verifier
        ),
        "duplicate start replaced the reserved verifier"
    );
}

#[test]
fn randomness_failure_never_reserves_partial_state() {
    let config = config_with_id(OAuthProvider::Google, "client-id");
    let random = DeterministicRandom::failing_after(1);
    let mut store = InMemoryOAuthStateStore::default();
    assert_eq!(
        oauth_start::begin(
            OAuthProvider::Google,
            &config.google,
            &mut store,
            &random,
            NOW,
        ),
        Err(OAuthError::RandomnessFailure)
    );
    assert_eq!(random.calls(), 2);
    assert_eq!(
        store.consume(&URL_SAFE_NO_PAD.encode([0u8; 32]), NOW),
        Err(OAuthError::InvalidState)
    );
}

#[test]
fn start_debug_and_errors_redact_state_verifier_and_complete_urls() {
    const SENTINEL: &str = "state-verifier-client-url-secret-sentinel";
    let start = OAuthStart {
        state: SENTINEL.to_owned(),
        code_verifier: SENTINEL.to_owned(),
        authorize_url: reqwest::Url::parse(&format!(
            "https://authorize.example.test/start?client_id={SENTINEL}"
        ))
        .unwrap(),
    };
    let debug = format!("{start:?}");
    assert!(!debug.contains(SENTINEL));
    assert!(!debug.contains("authorize.example.test"));

    for error in [
        OAuthError::InvalidState,
        OAuthError::StateAlreadyUsed,
        OAuthError::StateExpired,
        OAuthError::InvalidCode,
        OAuthError::InvalidProvider,
        OAuthError::RandomnessFailure,
        OAuthError::StateStoreFailure,
        OAuthError::InvalidConfiguration,
    ] {
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(SENTINEL));
    }
}
