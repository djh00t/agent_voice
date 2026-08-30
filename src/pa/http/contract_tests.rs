use super::{ApiError, AppointmentSlotDto, HttpResult, QuoteDto};
use crate::pa::domain::{AppointmentSlot, Quote, QuoteId};
use serde_json::json;
use time::macros::datetime;
use uuid::Uuid;

fn quote() -> Quote {
    Quote::with_id(
        QuoteId::from_uuid(Uuid::from_u128(0x00000000000000000000000000000065)),
        datetime!(2026-08-30 08:00:00 UTC),
    )
}

fn appointment_slot() -> AppointmentSlot {
    AppointmentSlot::new(
        datetime!(2026-08-30 08:15:00 UTC),
        datetime!(2026-08-30 08:45:00 UTC),
    )
    .expect("valid appointment slot")
}

#[test]
fn quote_dto_serializes_exact_keys() {
    let value = serde_json::to_value(QuoteDto::from(&quote())).expect("serialize quote DTO");

    assert_eq!(
        value,
        json!({
            "quote_id": "00000000-0000-0000-0000-000000000065",
            "issued_at": "2026-08-30T08:00:00Z",
            "expires_at": "2026-08-30T08:05:00Z",
        })
    );
}

#[test]
fn projections_preserve_validated_domain_values() {
    let quote = quote();
    let quote_dto = QuoteDto::from(&quote);
    assert_eq!(quote_dto.quote_id, quote.quote_id());
    assert_eq!(quote_dto.issued_at, quote.issued_at());
    assert_eq!(quote_dto.expires_at, quote.expires_at());

    let slot = appointment_slot();
    let slot_dto = AppointmentSlotDto::from(&slot);
    assert_eq!(slot_dto.starts_at, slot.starts_at());
    assert_eq!(slot_dto.ends_at, slot.ends_at());
}

#[test]
fn appointment_slot_dto_serializes_exact_keys() {
    let value = serde_json::to_value(AppointmentSlotDto::from(&appointment_slot()))
        .expect("serialize appointment slot DTO");

    assert_eq!(
        value,
        json!({
            "starts_at": "2026-08-30T08:15:00Z",
            "ends_at": "2026-08-30T08:45:00Z",
        })
    );
}

#[test]
fn api_error_serializes_only_safe_fields_and_redacts_debug() {
    let error = ApiError::new("invalid_request", "safe message", "request-65");

    assert_eq!(error.code(), "invalid_request");
    assert_eq!(error.message(), "safe message");
    assert_eq!(error.request_id(), "request-65");
    assert_eq!(
        serde_json::to_value(&error).expect("serialize API error"),
        json!({
            "code": "invalid_request",
            "message": "safe message",
            "request_id": "request-65",
        })
    );

    let debug = format!("{error:?}");
    assert!(debug.contains("ApiError"));
    assert!(debug.contains("invalid_request"));
    assert!(!debug.contains("safe message"));
    assert!(!debug.contains("request-65"));
}

#[test]
fn http_result_is_a_plain_result_alias() {
    let success: HttpResult<QuoteDto> = Ok(QuoteDto::from(&quote()));
    assert!(success.is_ok());

    let failure: HttpResult<()> = Err(ApiError::new("failure", "message", "request-65"));
    assert!(failure.is_err());
}
