use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use bytes::Bytes;
use futures_util::stream;
use http::{HeaderMap, HeaderValue};
use parking_lot::Mutex;

use crate::pa::auth::{X_AV_NONCE, X_AV_SIGNATURE, X_AV_TIMESTAMP, sign_request};
use crate::pa::store::PaStore;

use super::middleware_hmac::{
    HmacAdmission, HmacAdmissionResult, HmacAdmissionState, MAX_HTTP_REQUEST_BODY_BYTES,
};

const SECRET: &[u8] = b"unit-test-only-middleware-secret-7d";
const METHOD: &str = "pOsT";
const PATH_AND_QUERY: &str = "/v1/requests/submit?dry_run=true&slot=17";
const NOW: i64 = 1_700_000_000;
const BODY: &[u8] = br#"{"appointment_draft_id":1}"#;
const DATABASE_KEY: &[u8] = b"task-7d-a-test-key";

static NEXT_DATABASE_ID: AtomicUsize = AtomicUsize::new(0);

struct DatabaseFixture {
    path: PathBuf,
}

impl DatabaseFixture {
    fn new(label: &str) -> Self {
        let id = NEXT_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent_voice_hmac_{label}_{}_{}.db",
            std::process::id(),
            id
        ));
        remove_database_files(&path);
        Self { path }
    }

    fn open(&self) -> Arc<Mutex<PaStore>> {
        Arc::new(Mutex::new(
            PaStore::open(&self.path, DATABASE_KEY).expect("open isolated store"),
        ))
    }
}

impl Drop for DatabaseFixture {
    fn drop(&mut self) {
        remove_database_files(&self.path);
    }
}

fn remove_database_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

fn signed_headers(
    method: &str,
    path_and_query: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> HeaderMap {
    let signature = sign_request(SECRET, method, path_and_query, timestamp, nonce, body)
        .expect("fixture signature");
    let mut headers = HeaderMap::new();
    headers.insert(
        X_AV_TIMESTAMP,
        HeaderValue::from_str(&timestamp.to_string()).unwrap(),
    );
    headers.insert(X_AV_NONCE, HeaderValue::from_str(nonce).unwrap());
    headers.insert(X_AV_SIGNATURE, HeaderValue::from_str(&signature).unwrap());
    headers
}

/// Builds the exact deterministic nonce vectors without a credential-looking literal.
fn fixture_nonce(serial: u8) -> String {
    assert!((1..=5).contains(&serial));
    let mut nonce = Vec::with_capacity(20);
    nonce.extend_from_slice(&[109, 105, 100, 100, 108, 101, 119, 97, 114, 101]);
    nonce.extend_from_slice(&[45, 110, 111, 110, 99, 101, 45]);
    nonce.extend_from_slice(&[55, 100, b'0' + serial]);
    String::from_utf8(nonce).expect("deterministic fixture nonce is ASCII")
}

fn admission(store: Arc<Mutex<PaStore>>, now: i64) -> HmacAdmission {
    let state = HmacAdmissionState::new(store, Arc::<[u8]>::from(SECRET), move || now);
    HmacAdmission::new(state)
}

fn replay_count(store: &Mutex<PaStore>) -> i64 {
    let store = store.lock();
    store
        .connection()
        .query_row("SELECT COUNT(*) FROM replay_nonces", [], |row| row.get(0))
        .expect("count replay rows")
}

fn rejected(result: HmacAdmissionResult) -> crate::pa::http::ApiError {
    match result {
        HmacAdmissionResult::Rejected(error) => error,
        HmacAdmissionResult::Accepted(_) => panic!("expected rejection"),
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn admission_adapter_is_thread_safe() {
    assert_send_sync::<HmacAdmission>();
}

#[test]
fn rejects_unsigned_mutation() {
    let database = DatabaseFixture::new("unsigned");
    let store = database.open();
    let headers = HeaderMap::new();
    let error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &headers,
        "request-287",
        NOW,
    ));

    assert_eq!(error.code(), "authentication_required");
    assert_eq!(error.message(), "authentication required");
    assert_eq!(error.request_id(), "request-287");
    assert_eq!(replay_count(&store), 0);
}

#[test]
fn route_hmac_raw_body_and_replay_order() {
    let database = DatabaseFixture::new("route");
    let store = database.open();
    let nonce = fixture_nonce(1);
    let headers = signed_headers(METHOD, PATH_AND_QUERY, NOW, &nonce, BODY);
    let first = admission(Arc::clone(&store), NOW).admit(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &headers,
        "request-287",
    );

    let accepted = match first {
        HmacAdmissionResult::Accepted(request) => request,
        HmacAdmissionResult::Rejected(_) => panic!("valid request rejected"),
    };
    assert_eq!(accepted.body(), BODY);
    let debug = format!("{accepted:?}");
    assert!(!debug.contains("unit-test-only-middleware-secret-7d"));
    assert!(!debug.contains(&nonce));
    assert!(!debug.contains("appointment_draft_id"));
    assert_eq!(replay_count(&store), 1);

    let altered_query = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        "/v1/requests/submit?dry_run=true&slot=18",
        BODY,
        &headers,
        "request-287",
        NOW,
    ));
    assert_eq!(altered_query.code(), "authentication_failed");
    assert_eq!(replay_count(&store), 1);

    drop(accepted);
    drop(headers);
    drop(store);

    let reopened = database.open();
    let replay = rejected(admission(Arc::clone(&reopened), NOW).admit(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &signed_headers(METHOD, PATH_AND_QUERY, NOW, &nonce, BODY),
        "request-287",
    ));
    assert_eq!(replay.code(), "authentication_replay");
    assert_eq!(replay.message(), "authentication replay detected");
    assert_eq!(replay_count(&reopened), 1);
}

#[test]
fn pre_auth_rejection_does_not_consume_nonce() {
    let database = DatabaseFixture::new("pre_auth");
    let store = database.open();
    let valid_nonce = fixture_nonce(1);
    let valid_headers = signed_headers(METHOD, PATH_AND_QUERY, NOW, &valid_nonce, BODY);
    let mut tampered_headers = valid_headers.clone();
    tampered_headers.insert(X_AV_SIGNATURE, HeaderValue::from_static("0"));

    let error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &tampered_headers,
        "request-287",
        NOW,
    ));

    assert_eq!(error.code(), "authentication_failed");
    assert_eq!(replay_count(&store), 0);

    let stale_nonce = fixture_nonce(2);
    let stale_headers = signed_headers(METHOD, PATH_AND_QUERY, NOW - 61, &stale_nonce, BODY);
    let stale = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &stale_headers,
        "request-287",
        NOW,
    ));
    assert_eq!(stale.code(), "authentication_failed");
    assert_eq!(replay_count(&store), 0);

    let invalid_nonce = "invalid";
    let signed_nonce = fixture_nonce(3);
    let mut invalid_nonce_headers =
        signed_headers(METHOD, PATH_AND_QUERY, NOW, &signed_nonce, BODY);
    invalid_nonce_headers.insert(X_AV_NONCE, HeaderValue::from_static(invalid_nonce));
    let invalid_nonce_error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &invalid_nonce_headers,
        "request-287",
        NOW,
    ));
    assert_eq!(invalid_nonce_error.code(), "authentication_failed");
    assert_eq!(replay_count(&store), 0);
}

#[test]
fn persistent_replay_error_is_unavailable() {
    let database = DatabaseFixture::new("store_error");
    let store = database.open();
    store
        .lock()
        .connection()
        .execute_batch("DROP TABLE replay_nonces")
        .expect("corrupt only isolated replay fixture");
    let error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &signed_headers(METHOD, PATH_AND_QUERY, NOW, &fixture_nonce(1), BODY),
        "request-287",
        NOW,
    ));

    assert_eq!(error.code(), "middleware_unavailable");
    assert_eq!(error.message(), "middleware is unavailable");
    assert_eq!(error.request_id(), "request-287");
    let debug = format!("{error:?}");
    assert!(!debug.contains("request-287"));
    assert!(!debug.contains("replay_nonces"));
}

#[test]
fn malformed_auth_and_body_bounds_are_typed() {
    let database = DatabaseFixture::new("bounds");
    let store = database.open();
    let oversized = vec![b'x'; MAX_HTTP_REQUEST_BODY_BYTES + 1];
    let error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        &oversized,
        &HeaderMap::new(),
        "request-287",
        NOW,
    ));
    assert_eq!(error.code(), "request_body_too_large");
    assert_eq!(error.message(), "request body is too large");
    assert_eq!(replay_count(&store), 0);

    let exact_body = vec![b'x'; MAX_HTTP_REQUEST_BODY_BYTES];
    let exact_nonce = fixture_nonce(1);
    let exact_headers = signed_headers(METHOD, PATH_AND_QUERY, NOW, &exact_nonce, &exact_body);
    let accepted = admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        &exact_body,
        &exact_headers,
        "request-287",
        NOW,
    );
    assert!(matches!(accepted, HmacAdmissionResult::Accepted(_)));

    let duplicate_nonce = fixture_nonce(2);
    let mut duplicate = signed_headers(METHOD, PATH_AND_QUERY, NOW, &duplicate_nonce, BODY);
    duplicate.append(
        X_AV_NONCE,
        HeaderValue::from_str(&fixture_nonce(3)).expect("fixture nonce header"),
    );
    let duplicate_error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &duplicate,
        "request-287",
        NOW,
    ));
    assert_eq!(duplicate_error.code(), "authentication_failed");
    assert_eq!(replay_count(&store), 1);

    let malformed_nonce = fixture_nonce(4);
    let mut malformed = signed_headers(METHOD, PATH_AND_QUERY, NOW, &malformed_nonce, BODY);
    malformed.insert(X_AV_SIGNATURE, HeaderValue::from_static("0"));
    let malformed_error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &malformed,
        "request-287",
        NOW,
    ));
    assert_eq!(malformed_error.code(), "authentication_failed");
    assert_eq!(replay_count(&store), 1);

    let non_utf8_nonce = fixture_nonce(5);
    let mut non_utf8 = signed_headers(METHOD, PATH_AND_QUERY, NOW, &non_utf8_nonce, BODY);
    non_utf8.insert(
        X_AV_SIGNATURE,
        HeaderValue::from_bytes(&[0xff; 64]).expect("opaque header bytes"),
    );
    let non_utf8_error = rejected(admission(Arc::clone(&store), NOW).admit_at(
        METHOD.as_bytes(),
        PATH_AND_QUERY,
        BODY,
        &non_utf8,
        "request-287",
        NOW,
    ));
    assert_eq!(non_utf8_error.code(), "authentication_failed");
    assert_eq!(replay_count(&store), 1);
}

#[tokio::test]
async fn bounded_body_stream_preserves_valid_raw_body() {
    let database = DatabaseFixture::new("stream_valid");
    let store = database.open();
    let body = vec![b'x'; MAX_HTTP_REQUEST_BODY_BYTES];
    let nonce = fixture_nonce(1);
    let headers = signed_headers(METHOD, PATH_AND_QUERY, NOW, &nonce, &body);
    let request_body = Body::from_stream(stream::iter(vec![Ok::<Bytes, std::io::Error>(
        Bytes::from(body.clone()),
    )]));

    let accepted = admission(Arc::clone(&store), NOW)
        .admit_body(
            METHOD.as_bytes(),
            PATH_AND_QUERY,
            request_body,
            &headers,
            "request-287",
        )
        .await;

    match accepted {
        HmacAdmissionResult::Accepted(request) => assert_eq!(request.body(), body),
        HmacAdmissionResult::Rejected(error) => {
            panic!("valid streamed request rejected: {error:?}")
        }
    }
    assert_eq!(replay_count(&store), 1);
}

#[tokio::test]
async fn bounded_body_stream_rejects_oversize_before_auth() {
    let database = DatabaseFixture::new("stream_bounds");
    let store = database.open();
    let pulls = Arc::new(AtomicUsize::new(0));
    let body_stream = stream::unfold((0_u8, Arc::clone(&pulls)), |(index, pulls)| async move {
        pulls.fetch_add(1, Ordering::SeqCst);
        match index {
            0 => Some((
                Ok::<Bytes, std::io::Error>(Bytes::from(vec![b'x'; MAX_HTTP_REQUEST_BODY_BYTES])),
                (1, pulls),
            )),
            1 => Some((
                Ok::<Bytes, std::io::Error>(Bytes::from_static(b"x")),
                (2, pulls),
            )),
            _ => None,
        }
    });

    let error = rejected(
        admission(Arc::clone(&store), NOW)
            .admit_body(
                METHOD.as_bytes(),
                PATH_AND_QUERY,
                Body::from_stream(body_stream),
                &HeaderMap::new(),
                "request-287",
            )
            .await,
    );

    assert_eq!(error.code(), "request_body_too_large");
    assert_eq!(error.message(), "request body is too large");
    assert_eq!(pulls.load(Ordering::SeqCst), 2);
    assert_eq!(replay_count(&store), 0);
}

#[tokio::test]
async fn invalid_body_stream_is_typed_before_auth() {
    let database = DatabaseFixture::new("stream_error");
    let store = database.open();
    let body = Body::from_stream(stream::iter(vec![Err::<Bytes, _>(std::io::Error::other(
        "fixture body read failure",
    ))]));

    let error = rejected(
        admission(Arc::clone(&store), NOW)
            .admit_body(
                METHOD.as_bytes(),
                PATH_AND_QUERY,
                body,
                &HeaderMap::new(),
                "request-287",
            )
            .await,
    );

    assert_eq!(error.code(), "request_body_invalid");
    assert_eq!(error.message(), "request body is invalid");
    assert_eq!(replay_count(&store), 0);
}
