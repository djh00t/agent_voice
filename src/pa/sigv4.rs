//! Pure AWS Signature Version 4 signing primitives for S3.
//!
//! This module deliberately has no filesystem, network, clock, environment,
//! randomness, provider, or response-parsing responsibilities. Callers supply
//! the complete request and the timestamp used for signing.

use std::fmt;

use ring::{digest, hmac};

const SERVICE: &str = "s3";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const TERMINATOR: &str = "aws4_request";
const MAX_REGION_LEN: usize = 64;
const MAX_ACCESS_KEY_LEN: usize = 128;
const MAX_SECRET_LEN: usize = 4_096;
const MAX_SESSION_TOKEN_LEN: usize = 4_096;

/// The request values consumed by the signer.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    /// The HTTP method.
    pub method: String,
    /// The origin-form path and optional raw query.
    pub uri: String,
    /// Header names and values supplied by the caller.
    pub headers: Vec<(String, String)>,
    /// The lowercase 64-hex SHA-256 hash of the request payload.
    pub payload_sha256: String,
    /// The AWS signing region.
    pub region: String,
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Request").finish_non_exhaustive()
    }
}

/// The AWS credentials used to sign a request.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The access-key identifier.
    pub access_key_id: String,
    /// The secret access-key bytes.
    pub secret_access_key: Vec<u8>,
    /// The optional session token that must be sent as a signed header.
    pub session_token: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .finish_non_exhaustive()
    }
}

/// The deterministic outputs needed to send a signed request.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedRequest {
    /// The complete AWS authorization header value.
    pub authorization: String,
    /// The semicolon-separated canonical signed-header names.
    pub signed_headers: String,
    /// The validated timestamp inserted into the request headers.
    pub amz_date: String,
}

impl fmt::Debug for SignedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedRequest")
            .finish_non_exhaustive()
    }
}

/// The safe categories of signer failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigV4ErrorKind {
    /// The HTTP method is empty or not a valid ASCII token.
    InvalidMethod,
    /// The URI is not valid origin form or contains forbidden input.
    InvalidUri,
    /// The timestamp or signing date is not valid AWS format.
    InvalidTimestamp,
    /// The signing region is empty, too long, or not printable ASCII.
    InvalidRegion,
    /// The credential values are empty, too long, or malformed.
    InvalidCredential,
    /// A header name or value contains malformed input.
    InvalidHeader,
    /// More than one header has the same lowercase name.
    DuplicateHeader,
    /// A required signing header is absent.
    MissingRequiredHeader,
    /// The request and signed payload hashes do not match.
    PayloadHashMismatch,
    /// A header that cannot be signed was supplied.
    UnsupportedHeader,
    /// A cryptographic operation failed.
    #[allow(dead_code)]
    Crypto,
}

/// A redacted, stable signer error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SigV4Error {
    /// The safe category of the failure.
    pub kind: SigV4ErrorKind,
}

impl fmt::Debug for SigV4Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SigV4Error")
            .field("kind", &self.kind)
            .finish()
    }
}

impl fmt::Display for SigV4Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AWS Signature Version 4 signing failed")
    }
}

impl std::error::Error for SigV4Error {}

impl SigV4Error {
    fn new(kind: SigV4ErrorKind) -> Self {
        Self { kind }
    }
}

/// A derived AWS signing key whose key bytes are private to this module.
#[derive(Clone, PartialEq, Eq)]
pub struct SigningKey(Vec<u8>);

impl fmt::Debug for SigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SigningKey").finish_non_exhaustive()
    }
}

impl SigningKey {
    /// Derives the S3 signing key for a scope date and region.
    pub fn derive(
        secret: &[u8],
        date: &str,
        region: &str,
        service: &str,
    ) -> Result<Self, SigV4Error> {
        if secret.is_empty() || secret.len() > MAX_SECRET_LEN {
            return Err(SigV4Error::new(SigV4ErrorKind::InvalidCredential));
        }
        validate_scope_date(date)?;
        validate_region(region)?;
        if service != SERVICE {
            return Err(SigV4Error::new(SigV4ErrorKind::InvalidRegion));
        }

        let mut secret_key = Vec::with_capacity(4 + secret.len());
        secret_key.extend_from_slice(b"AWS4");
        secret_key.extend_from_slice(secret);
        let date_key = hmac_bytes(&secret_key, date.as_bytes());
        let region_key = hmac_bytes(&date_key, region.as_bytes());
        let service_key = hmac_bytes(&region_key, service.as_bytes());
        let signing_key = hmac_bytes(&service_key, TERMINATOR.as_bytes());
        Ok(Self(signing_key))
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        hmac_bytes(&self.0, message)
    }
}

/// Builds an AWS canonical request from caller-supplied request components.
pub fn canonical_request(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    payload_sha256: &str,
) -> Result<String, SigV4Error> {
    Ok(canonicalize(method, uri, headers, payload_sha256)?.canonical)
}

/// Signs an S3 request with the supplied credentials and UTC timestamp.
pub fn sign_request(
    request: &Request,
    credentials: &Credentials,
    timestamp: &str,
) -> Result<SignedRequest, SigV4Error> {
    let scope_date = validate_timestamp(timestamp)?;
    validate_region(&request.region)?;
    validate_credentials(credentials)?;

    let mut headers = request.headers.clone();
    let mut existing_date = None;
    for (name, value) in &headers {
        if name.eq_ignore_ascii_case("x-amz-date") {
            let normalized = normalize_header_value(value)?;
            if existing_date.is_some() {
                return Err(SigV4Error::new(SigV4ErrorKind::DuplicateHeader));
            }
            existing_date = Some(normalized);
        }
    }
    if let Some(existing_date) = existing_date {
        if existing_date != timestamp {
            return Err(SigV4Error::new(SigV4ErrorKind::InvalidTimestamp));
        }
    } else {
        headers.push(("x-amz-date".to_owned(), timestamp.to_owned()));
    }

    validate_session_token_header(&headers, credentials.session_token.as_deref())?;
    let canonicalized = canonicalize(
        &request.method,
        &request.uri,
        &headers,
        &request.payload_sha256,
    )?;
    let canonical_hash =
        lower_hex(digest::digest(&digest::SHA256, canonicalized.canonical.as_bytes()).as_ref());
    let string_to_sign = format!(
        "{ALGORITHM}\n{timestamp}\n{scope_date}/{}/{SERVICE}/{TERMINATOR}\n{canonical_hash}",
        request.region
    );
    let signing_key = SigningKey::derive(
        &credentials.secret_access_key,
        scope_date,
        &request.region,
        SERVICE,
    )?;
    let signature = lower_hex(signing_key.sign(string_to_sign.as_bytes()).as_slice());
    let authorization = format!(
        "{ALGORITHM} Credential={}/{}/{}/{SERVICE}/{TERMINATOR},SignedHeaders={},Signature={signature}",
        credentials.access_key_id, scope_date, request.region, canonicalized.signed_headers
    );

    Ok(SignedRequest {
        authorization,
        signed_headers: canonicalized.signed_headers,
        amz_date: timestamp.to_owned(),
    })
}

struct CanonicalizedRequest {
    canonical: String,
    signed_headers: String,
}

struct CanonicalHeader {
    name: String,
    value: String,
}

fn canonicalize(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    payload_sha256: &str,
) -> Result<CanonicalizedRequest, SigV4Error> {
    validate_payload_hash(payload_sha256)?;
    let method = canonical_method(method)?;
    let (canonical_uri, canonical_query) = canonical_uri_and_query(uri)?;
    let headers = canonical_headers(headers, payload_sha256)?;
    let mut canonical_header_lines = String::new();
    let mut signed_headers = String::new();
    for (index, header) in headers.iter().enumerate() {
        if index != 0 {
            signed_headers.push(';');
        }
        signed_headers.push_str(&header.name);
        canonical_header_lines.push_str(&header.name);
        canonical_header_lines.push(':');
        canonical_header_lines.push_str(&header.value);
        canonical_header_lines.push('\n');
    }

    let canonical = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_header_lines}\n{signed_headers}\n{payload_sha256}"
    );
    Ok(CanonicalizedRequest {
        canonical,
        signed_headers,
    })
}

fn canonical_headers(
    headers: &[(String, String)],
    payload_sha256: &str,
) -> Result<Vec<CanonicalHeader>, SigV4Error> {
    let mut normalized = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        normalized.push(CanonicalHeader {
            name: normalize_header_name(name)?,
            value: normalize_header_value(value)?,
        });
    }
    normalized.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    for pair in normalized.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(SigV4Error::new(SigV4ErrorKind::DuplicateHeader));
        }
    }

    let host = normalized
        .iter()
        .find(|header| header.name == "host")
        .ok_or_else(|| SigV4Error::new(SigV4ErrorKind::MissingRequiredHeader))?;
    if host.value.is_empty() {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidHeader));
    }

    let payload_header = normalized
        .iter()
        .find(|header| header.name == "x-amz-content-sha256")
        .ok_or_else(|| SigV4Error::new(SigV4ErrorKind::MissingRequiredHeader))?;
    if payload_header.value != payload_sha256 {
        return Err(SigV4Error::new(SigV4ErrorKind::PayloadHashMismatch));
    }

    let date_header = normalized
        .iter()
        .find(|header| header.name == "x-amz-date")
        .ok_or_else(|| SigV4Error::new(SigV4ErrorKind::MissingRequiredHeader))?;
    validate_timestamp(&date_header.value)?;

    Ok(normalized)
}

fn canonical_method(method: &str) -> Result<String, SigV4Error> {
    if method.is_empty()
        || method.len() > 64
        || !method.is_ascii()
        || !method.bytes().all(is_method_byte)
    {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidMethod));
    }
    Ok(method.to_owned())
}

fn is_method_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn canonical_uri_and_query(uri: &str) -> Result<(String, String), SigV4Error> {
    if uri.chars().any(char::is_control) || uri.contains('#') {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidUri));
    }
    let (raw_path, raw_query) = uri.split_once('?').unwrap_or((uri, ""));
    let raw_path = if raw_path.is_empty() { "/" } else { raw_path };
    if !raw_path.starts_with('/') {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidUri));
    }
    let canonical_path = percent_encode_path(raw_path.as_bytes());
    let canonical_query = canonical_query(raw_query);
    Ok((canonical_path, canonical_query))
}

fn canonical_query(raw_query: &str) -> String {
    if raw_query.is_empty() {
        return String::new();
    }
    let mut pairs = raw_query
        .split('&')
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (
                percent_encode(name.as_bytes(), false),
                percent_encode(value.as_bytes(), false),
            )
        })
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &[u8], preserve_slash: bool) -> String {
    percent_encode_with_options(value, preserve_slash, false)
}

fn percent_encode_path(value: &[u8]) -> String {
    percent_encode_with_options(value, true, true)
}

fn percent_encode_with_options(
    value: &[u8],
    preserve_slash: bool,
    preserve_valid_escapes: bool,
) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        let byte = value[index];
        if preserve_valid_escapes
            && byte == b'%'
            && index + 2 < value.len()
            && is_hex_byte(value[index + 1])
            && is_hex_byte(value[index + 2])
        {
            encoded.push('%');
            encoded.push(value[index + 1] as char);
            encoded.push(value[index + 2] as char);
            index += 3;
            continue;
        }
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (preserve_slash && byte == b'/')
        {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        index += 1;
    }
    encoded
}

fn is_hex_byte(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f' | b'A'..=b'F')
}

fn normalize_header_name(name: &str) -> Result<String, SigV4Error> {
    if name.is_empty() || !name.is_ascii() || !name.bytes().all(is_header_name_byte) {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidHeader));
    }
    let normalized = name.to_ascii_lowercase();
    if normalized == "authorization" {
        return Err(SigV4Error::new(SigV4ErrorKind::UnsupportedHeader));
    }
    Ok(normalized)
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn normalize_header_value(value: &str) -> Result<String, SigV4Error> {
    if value
        .chars()
        .any(|character| character.is_control() && character != ' ' && character != '\t')
    {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidHeader));
    }

    let mut normalized = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        if matches!(character, ' ' | '\t') {
            if !normalized.is_empty() {
                pending_space = true;
            }
            continue;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        normalized.push(character);
    }
    Ok(normalized)
}

fn validate_payload_hash(payload_sha256: &str) -> Result<(), SigV4Error> {
    if payload_sha256.len() != 64
        || !payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SigV4Error::new(SigV4ErrorKind::PayloadHashMismatch));
    }
    Ok(())
}

fn validate_timestamp(timestamp: &str) -> Result<&str, SigV4Error> {
    let bytes = timestamp.as_bytes();
    if bytes.len() != 16
        || bytes[8] != b'T'
        || bytes[15] != b'Z'
        || !bytes[..8].iter().all(u8::is_ascii_digit)
        || !bytes[9..15].iter().all(u8::is_ascii_digit)
    {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidTimestamp));
    }
    let hour = two_digits(&bytes[9..11]);
    let minute = two_digits(&bytes[11..13]);
    let second = two_digits(&bytes[13..15]);
    if hour > 23 || minute > 59 || second > 59 {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidTimestamp));
    }
    validate_scope_date(&timestamp[..8])?;
    Ok(&timestamp[..8])
}

fn validate_scope_date(date: &str) -> Result<(), SigV4Error> {
    let bytes = date.as_bytes();
    if bytes.len() != 8 || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidTimestamp));
    }
    let year = four_digits(&bytes[..4]);
    let month = two_digits(&bytes[4..6]);
    let day = two_digits(&bytes[6..8]);
    if month == 0 || month > 12 || day == 0 || day > days_in_month(year, month) {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidTimestamp));
    }
    Ok(())
}

fn four_digits(bytes: &[u8]) -> u16 {
    (bytes[0] - b'0') as u16 * 1_000
        + (bytes[1] - b'0') as u16 * 100
        + (bytes[2] - b'0') as u16 * 10
        + (bytes[3] - b'0') as u16
}

fn two_digits(bytes: &[u8]) -> u8 {
    (bytes[0] - b'0') * 10 + bytes[1] - b'0'
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn validate_region(region: &str) -> Result<(), SigV4Error> {
    if region.is_empty()
        || region.len() > MAX_REGION_LEN
        || !region
            .bytes()
            .all(|byte| byte.is_ascii() && (0x21..=0x7e).contains(&byte))
    {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidRegion));
    }
    Ok(())
}

fn validate_credentials(credentials: &Credentials) -> Result<(), SigV4Error> {
    if !is_bounded_ascii_identifier(&credentials.access_key_id, MAX_ACCESS_KEY_LEN)
        || credentials.secret_access_key.is_empty()
        || credentials.secret_access_key.len() > MAX_SECRET_LEN
    {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidCredential));
    }
    if let Some(token) = &credentials.session_token
        && !is_bounded_ascii_identifier(token, MAX_SESSION_TOKEN_LEN)
    {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidCredential));
    }
    Ok(())
}

fn is_bounded_ascii_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii() && (0x21..=0x7e).contains(&byte))
}

fn validate_session_token_header(
    headers: &[(String, String)],
    expected: Option<&str>,
) -> Result<(), SigV4Error> {
    let mut matching = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("x-amz-security-token"));
    let Some(expected) = expected else {
        if matching.next().is_some() {
            return Err(SigV4Error::new(SigV4ErrorKind::InvalidCredential));
        }
        return Ok(());
    };
    let Some((_, value)) = matching.next() else {
        return Err(SigV4Error::new(SigV4ErrorKind::MissingRequiredHeader));
    };
    if matching.next().is_some() {
        return Err(SigV4Error::new(SigV4ErrorKind::DuplicateHeader));
    }
    if normalize_header_value(value)? != expected {
        return Err(SigV4Error::new(SigV4ErrorKind::InvalidCredential));
    }
    Ok(())
}

fn hmac_bytes(key_bytes: &[u8], message: &[u8]) -> Vec<u8> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key_bytes);
    hmac::sign(&key, message).as_ref().to_vec()
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

#[cfg(test)]
mod tests {
    use super::*;

    const TIMESTAMP: &str = "20130524T000000Z";
    const PAYLOAD_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn headers() -> Vec<(String, String)> {
        vec![
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("x-amz-content-sha256".into(), PAYLOAD_HASH.into()),
            ("x-amz-date".into(), TIMESTAMP.into()),
        ]
    }

    #[test]
    fn canonical_uri_preserves_valid_percent_escapes() {
        let canonical = canonical_request("GET", "/folder/a%20b", &headers(), PAYLOAD_HASH)
            .expect("origin-form URI is valid");

        assert!(canonical.starts_with("GET\n/folder/a%20b\n\n"));
    }

    #[test]
    fn canonical_uri_encodes_unescaped_bytes_and_malformed_percent() {
        let canonical = canonical_request("GET", "/folder/a%2/%ZZ café", &headers(), PAYLOAD_HASH)
            .expect("origin-form URI is valid");

        assert!(canonical.starts_with("GET\n/folder/a%252/%25ZZ%20caf%C3%A9\n\n"));
    }
}
