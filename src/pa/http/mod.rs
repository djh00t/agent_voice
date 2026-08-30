//! Shared HTTP wire projections and a redacted error envelope.
//!
//! This module contains response-only values shared by later HTTP packages.
//! It does not own request routing, status mapping, authentication, or
//! runtime behavior.

use std::fmt;

use serde::Serialize;
use time::OffsetDateTime;

use crate::pa::domain::{AppointmentSlot, Quote, QuoteId};

/// Response projection for a validated quote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuoteDto {
    pub quote_id: QuoteId,
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl From<&Quote> for QuoteDto {
    fn from(value: &Quote) -> Self {
        Self {
            quote_id: value.quote_id(),
            issued_at: value.issued_at(),
            expires_at: value.expires_at(),
        }
    }
}

/// Response projection for a validated appointment slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AppointmentSlotDto {
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
}

impl From<&AppointmentSlot> for AppointmentSlotDto {
    fn from(value: &AppointmentSlot) -> Self {
        Self {
            starts_at: value.starts_at(),
            ends_at: value.ends_at(),
        }
    }
}

/// A safe, already-redacted error response envelope.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ApiError {
    code: String,
    message: String,
    request_id: String,
}

impl ApiError {
    /// Creates an error from values that have already been redacted.
    #[allow(dead_code)]
    pub(crate) fn new(
        code: &'static str,
        message: &'static str,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            request_id: request_id.into(),
        }
    }

    /// Returns the stable, safe error code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the safe client-facing message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the request correlation identifier.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

impl fmt::Debug for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiError")
            .field("code", &self.code)
            .finish()
    }
}

/// The shared HTTP operation result alias.
pub type HttpResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod contract_tests;
