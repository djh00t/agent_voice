//! Axum routes for the local agent control plane.

use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{
    Json, Router,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::openai::TranscriptEvent;
use crate::service::{CallSnapshot, ServiceStatus, VoiceAgentService};

/// Builds the HTTP router for the local control API.
pub fn router(service: Arc<VoiceAgentService>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/status", get(status))
        .route("/v1/calls", get(list_calls))
        .route("/v1/calls/{call_id}", get(get_call))
        .route("/v1/calls/{call_id}/transcript", get(get_transcript))
        .route("/v1/calls/{call_id}/speak", post(speak))
        .route("/v1/calls/{call_id}/hangup", post(hangup))
        .route("/v1/dial", post(dial))
        .with_state(service)
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

async fn status(State(service): State<Arc<VoiceAgentService>>) -> Json<ServiceStatus> {
    Json(service.status())
}

async fn list_calls(State(service): State<Arc<VoiceAgentService>>) -> Json<Vec<CallSnapshot>> {
    Json(service.list_calls())
}

async fn get_call(
    Path(call_id): Path<String>,
    State(service): State<Arc<VoiceAgentService>>,
) -> ApiResult<Json<CallSnapshot>> {
    match service.call_snapshot(&call_id) {
        Some(call) => Ok(Json(call)),
        None => Err(ApiError::not_found(format!("unknown call id {}", call_id))),
    }
}

async fn get_transcript(
    Path(call_id): Path<String>,
    State(service): State<Arc<VoiceAgentService>>,
) -> ApiResult<Json<Vec<TranscriptEvent>>> {
    match service.transcript_for(&call_id) {
        Some(events) => Ok(Json(events)),
        None => Err(ApiError::not_found(format!("unknown call id {}", call_id))),
    }
}

async fn dial(
    State(service): State<Arc<VoiceAgentService>>,
    Json(body): Json<DialRequest>,
) -> ApiResult<Json<CallSnapshot>> {
    let call = service
        .dial(body.target)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(call))
}

async fn speak(
    Path(call_id): Path<String>,
    State(service): State<Arc<VoiceAgentService>>,
    Json(body): Json<SpeakRequest>,
) -> ApiResult<StatusCode> {
    service
        .speak_text(&call_id, body.text, body.voice, body.instructions)
        .await
        .map_err(ApiError::internal)?;
    Ok(StatusCode::ACCEPTED)
}

async fn hangup(
    Path(call_id): Path<String>,
    State(service): State<Arc<VoiceAgentService>>,
) -> ApiResult<StatusCode> {
    service.hangup(&call_id).map_err(ApiError::internal)?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, Deserialize)]
struct DialRequest {
    target: String,
}

#[derive(Debug, Deserialize)]
struct SpeakRequest {
    text: String,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
}

type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn internal(error: impl ToString) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[allow(dead_code)]
fn _assert_result_send_sync(_: Result<()>) {}
