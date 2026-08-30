#[path = "../src/pa/sigv4.rs"]
mod sigv4;

use ring::digest;

use sigv4::{Credentials, Request, SigV4ErrorKind, SigningKey, canonical_request, sign_request};

const ACCESS_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";
const SECRET_ACCESS_KEY: &[u8] = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
const REGION: &str = "us-east-1";
const TIMESTAMP: &str = "20130524T000000Z";
const PAYLOAD_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn fixture_headers() -> Vec<(String, String)> {
    vec![
        ("host".into(), "examplebucket.s3.amazonaws.com".into()),
        ("range".into(), "bytes=0-9".into()),
        ("x-amz-content-sha256".into(), PAYLOAD_HASH.into()),
        ("x-amz-date".into(), TIMESTAMP.into()),
    ]
}

fn fixture_request(headers: Vec<(String, String)>) -> Request {
    Request {
        method: "GET".into(),
        uri: "/test.txt".into(),
        headers,
        payload_sha256: PAYLOAD_HASH.into(),
        region: REGION.into(),
    }
}

fn fixture_credentials() -> Credentials {
    Credentials {
        access_key_id: ACCESS_KEY_ID.into(),
        secret_access_key: SECRET_ACCESS_KEY.to_vec(),
        session_token: None,
    }
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

#[test]
fn aws_s3_golden_fixture() {
    let headers = fixture_headers();
    let canonical = canonical_request("GET", "/test.txt", &headers, PAYLOAD_HASH)
        .expect("frozen AWS fixture is valid");
    let expected_canonical = "GET\n/test.txt\n\nhost:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\nx-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nx-amz-date:20130524T000000Z\n\nhost;range;x-amz-content-sha256;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(canonical, expected_canonical);

    let canonical_hash = lower_hex(digest::digest(&digest::SHA256, canonical.as_bytes()).as_ref());
    assert_eq!(
        canonical_hash,
        "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
    );

    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{TIMESTAMP}\n20130524/{REGION}/s3/aws4_request\n{canonical_hash}"
    );
    assert_eq!(
        string_to_sign,
        "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
    );

    let signed = sign_request(&fixture_request(headers), &fixture_credentials(), TIMESTAMP)
        .expect("frozen AWS fixture signs");
    assert_eq!(signed.amz_date, TIMESTAMP);
    assert_eq!(
        signed.signed_headers,
        "host;range;x-amz-content-sha256;x-amz-date"
    );
    assert_eq!(
        signed.authorization,
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    );
}

#[test]
fn header_insertion_order_is_irrelevant() {
    let first = fixture_request(fixture_headers());
    let mut reordered_headers = fixture_headers();
    reordered_headers.reverse();
    let second = fixture_request(reordered_headers);

    let first_canonical = canonical_request(
        &first.method,
        &first.uri,
        &first.headers,
        &first.payload_sha256,
    )
    .expect("first canonical request");
    let second_canonical = canonical_request(
        &second.method,
        &second.uri,
        &second.headers,
        &second.payload_sha256,
    )
    .expect("second canonical request");
    assert_eq!(first_canonical, second_canonical);

    let first_signed = sign_request(&first, &fixture_credentials(), TIMESTAMP).expect("first");
    let second_signed = sign_request(&second, &fixture_credentials(), TIMESTAMP).expect("second");
    assert_eq!(first_signed.authorization, second_signed.authorization);
}

#[test]
fn uri_query_and_unicode_rules() {
    let headers = vec![
        ("Host".into(), "examplebucket.s3.amazonaws.com".into()),
        ("x-amz-content-sha256".into(), PAYLOAD_HASH.into()),
        ("x-amz-date".into(), TIMESTAMP.into()),
    ];
    let canonical = canonical_request(
        "GET",
        "/a//./café path?acl&slash=/&slash=%&blank=&dup&dup=two",
        &headers,
        PAYLOAD_HASH,
    )
    .expect("URI and query are valid");
    assert!(canonical.starts_with(
        "GET\n/a//./caf%C3%A9%20path\nacl=&blank=&dup=&dup=two&slash=%25&slash=%2F\n"
    ));
}

#[test]
fn payload_hash_mismatch_is_rejected() {
    let request = Request {
        method: "GET".into(),
        uri: "/test.txt".into(),
        headers: vec![
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("x-amz-content-sha256".into(), "0".repeat(64)),
        ],
        payload_sha256: PAYLOAD_HASH.into(),
        region: REGION.into(),
    };
    let error = sign_request(&request, &fixture_credentials(), TIMESTAMP).unwrap_err();
    assert_eq!(error.kind, SigV4ErrorKind::PayloadHashMismatch);
}

#[test]
fn invalid_timestamp_and_duplicate_header_are_rejected() {
    let timestamp_error = sign_request(
        &fixture_request(fixture_headers()),
        &fixture_credentials(),
        "20130524T000000",
    )
    .unwrap_err();
    assert_eq!(timestamp_error.kind, SigV4ErrorKind::InvalidTimestamp);

    let duplicate_headers = vec![
        ("host".into(), "examplebucket.s3.amazonaws.com".into()),
        ("HOST".into(), "examplebucket.s3.amazonaws.com".into()),
        ("x-amz-content-sha256".into(), PAYLOAD_HASH.into()),
        ("x-amz-date".into(), TIMESTAMP.into()),
    ];
    let duplicate_error =
        canonical_request("GET", "/test.txt", &duplicate_headers, PAYLOAD_HASH).unwrap_err();
    assert_eq!(duplicate_error.kind, SigV4ErrorKind::DuplicateHeader);
}

#[test]
fn debug_and_error_redact_sentinels() {
    const SENTINEL: &str = "SENTINEL_SECRET_URI_HEADER_TOKEN_SIGNATURE";
    let request = Request {
        method: "GET".into(),
        uri: SENTINEL.into(),
        headers: vec![("host".into(), SENTINEL.into())],
        payload_sha256: SENTINEL.into(),
        region: "us-east-1".into(),
    };
    let credentials = Credentials {
        access_key_id: SENTINEL.into(),
        secret_access_key: SENTINEL.as_bytes().to_vec(),
        session_token: Some(SENTINEL.into()),
    };
    let signed = sigv4::SignedRequest {
        authorization: SENTINEL.into(),
        signed_headers: SENTINEL.into(),
        amz_date: SENTINEL.into(),
    };
    let signing_key =
        SigningKey::derive(b"secret", "20130524", "us-east-1", "s3").expect("valid key");
    let error = sigv4::SigV4Error {
        kind: SigV4ErrorKind::InvalidHeader,
    };

    for debug in [
        format!("{request:?}"),
        format!("{credentials:?}"),
        format!("{signed:?}"),
        format!("{signing_key:?}"),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(!debug.contains(SENTINEL), "leaked sentinel: {debug}");
    }
}
