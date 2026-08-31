//! Redacted, bounded administrative read projections.

#[cfg(test)]
#[path = "admin_store_tests.rs"]
mod tests;

use std::fmt;
use std::sync::Arc;

use ring::digest;
use rusqlite::Row;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::pa::admin_config::{AdminConfig, AdminConfigPatch, AdminConfigStore};
use crate::pa::store::{PaStore, StoreError, StoreResult};

const LIMIT: i64 = 100;

/// Typed, side-effect-free admin read producer.
#[derive(Clone)]
pub struct PaAdminStore {
    store: Arc<PaStore>,
    config: AdminConfigStore,
}

impl fmt::Debug for PaAdminStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PaAdminStore").finish()
    }
}

/// The complete redacted admin read model (intentionally without backup state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSnapshot {
    pub owner: AdminOwner,
    pub config: AdminConfig,
    pub connections: Vec<AdminConnection>,
    pub proposals: Vec<AdminProposal>,
    pub failures: Vec<AdminFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminOwner {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConnection {
    pub provider: AdminProvider,
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub status: AdminConnectionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminProvider {
    Google,
    Outlook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminConnectionStatus {
    Connected,
    Missing,
    Expired,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminProposal {
    pub id: i64,
    pub state: AdminProposalState,
    pub appointment_kind: AdminAppointmentKind,
    pub starts_at: String,
    pub ends_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminProposalState {
    Pending,
    Accepted,
    Declined,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminAppointmentKind {
    Callback,
    Meeting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminFailure {
    pub id: i64,
    pub category: AdminFailureCategory,
    pub occurred_at: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminFailureCategory {
    Validation,
    Auth,
    Dependency,
    Provider,
    Store,
    Unexpected,
}

impl PaAdminStore {
    pub fn new(store: Arc<PaStore>) -> Self {
        Self {
            config: AdminConfigStore::new(Arc::clone(&store)),
            store,
        }
    }

    pub fn read_snapshot(&self) -> StoreResult<AdminSnapshot> {
        self.read_snapshot_at(OffsetDateTime::now_utc())
    }

    pub fn read_snapshot_at(&self, observed_at: OffsetDateTime) -> StoreResult<AdminSnapshot> {
        Ok(AdminSnapshot {
            owner: read_owner(self.store.connection())?,
            config: self.config.read()?,
            connections: read_connections(self.store.connection(), observed_at)?,
            proposals: read_proposals(self.store.connection())?,
            failures: read_failures(self.store.connection())?,
        })
    }

    pub fn update_config(
        &self,
        expected_version: i64,
        patch: AdminConfigPatch,
    ) -> StoreResult<AdminConfig> {
        self.config.update_config(expected_version, patch)
    }
}

fn read_owner(connection: &rusqlite::Connection) -> StoreResult<AdminOwner> {
    let (email, phone): (Option<String>, Option<String>) = connection
        .query_row(
            "SELECT owner_email, owner_phone FROM configuration WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| invalid())?;
    let configured = email.as_deref().is_some_and(|value| !value.is_empty())
        || phone.as_deref().is_some_and(|value| !value.is_empty());
    let identity_fingerprint = configured.then(|| {
        fingerprint(&format!(
            "owner:v1\n{}\n{}",
            email.unwrap_or_default(),
            phone.unwrap_or_default()
        ))
    });
    Ok(AdminOwner {
        configured,
        identity_fingerprint,
    })
}

fn read_connections(
    connection: &rusqlite::Connection,
    observed_at: OffsetDateTime,
) -> StoreResult<Vec<AdminConnection>> {
    let mut statement = connection.prepare("SELECT id, provider, account_id, expires_at FROM oauth_credentials ORDER BY provider ASC, account_id ASC, id ASC LIMIT ?1").map_err(|_| invalid())?;
    let rows = statement
        .query_map([LIMIT + 2], |row| connection_from_row(row, observed_at))
        .map_err(|_| invalid())?;
    let mut connections = vec![
        missing(AdminProvider::Google),
        missing(AdminProvider::Outlook),
    ];
    for row in rows {
        let row = row.map_err(|_| invalid())?;
        if let Some(position) = connections
            .iter()
            .position(|value| value.provider == row.provider && !value.configured)
        {
            connections[position] = row;
        } else if connections.len() < LIMIT as usize {
            connections.push(row);
        }
    }
    connections.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then(left.account_fingerprint.cmp(&right.account_fingerprint))
    });
    Ok(connections)
}

fn connection_from_row(
    row: &Row<'_>,
    observed_at: OffsetDateTime,
) -> rusqlite::Result<AdminConnection> {
    let provider: String = row.get(1)?;
    let account: String = row.get(2)?;
    let expires: Option<String> = row.get(3)?;
    let provider = match provider.as_str() {
        "google" => AdminProvider::Google,
        "outlook" => AdminProvider::Outlook,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let parsed = expires
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let status = if parsed.is_some_and(|value| value <= observed_at) {
        AdminConnectionStatus::Expired
    } else {
        AdminConnectionStatus::Connected
    };
    let expires_at = parsed
        .map(timestamp)
        .transpose()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(AdminConnection {
        provider,
        configured: true,
        account_fingerprint: Some(fingerprint(&format!(
            "account:v1\n{}\n{}",
            provider_name(provider),
            account
        ))),
        expires_at,
        status,
    })
}

fn read_proposals(connection: &rusqlite::Connection) -> StoreResult<Vec<AdminProposal>> {
    let mut statement = connection.prepare("SELECT p.id, p.state, a.kind, a.starts_at, a.ends_at, p.created_at, p.updated_at FROM proposals p LEFT JOIN appointment_drafts a ON a.id = p.appointment_draft_id ORDER BY p.id ASC LIMIT ?1").map_err(|_| invalid())?;
    statement
        .query_map([LIMIT], proposal_from_row)
        .map_err(|_| invalid())?
        .map(|row| row.map_err(|_| invalid()))
        .collect()
}

fn proposal_from_row(row: &Row<'_>) -> rusqlite::Result<AdminProposal> {
    let id: i64 = row.get(0)?;
    let state: String = row.get(1)?;
    let kind: Option<String> = row.get(2)?;
    if id < 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let state = match state.as_str() {
        "pending" => AdminProposalState::Pending,
        "accepted" => AdminProposalState::Accepted,
        "declined" => AdminProposalState::Declined,
        "expired" => AdminProposalState::Expired,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let appointment_kind = match kind.as_deref() {
        Some("callback") => AdminAppointmentKind::Callback,
        Some("meeting") => AdminAppointmentKind::Meeting,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let parse = |index| {
        let parsed = parse_timestamp(&row.get::<_, String>(index)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        timestamp(parsed).map_err(|_| rusqlite::Error::InvalidQuery)
    };
    Ok(AdminProposal {
        id,
        state,
        appointment_kind,
        starts_at: parse(3)?,
        ends_at: parse(4)?,
        created_at: parse(5)?,
        updated_at: parse(6)?,
    })
}

fn read_failures(connection: &rusqlite::Connection) -> StoreResult<Vec<AdminFailure>> {
    let mut validation = connection
        .prepare("SELECT id, event_type, occurred_at FROM audit_events ORDER BY id ASC")
        .map_err(|_| invalid())?;
    for row in validation
        .query_map([], audit_event_from_row)
        .map_err(|_| invalid())?
    {
        row.map_err(|_| invalid())?;
    }
    let mut statement = connection
        .prepare("SELECT id, event_type, occurred_at FROM audit_events WHERE event_type = 'notification_retry_scheduled' ORDER BY id ASC LIMIT ?1")
        .map_err(|_| invalid())?;
    let rows = statement
        .query_map([LIMIT], audit_event_from_row)
        .map_err(|_| invalid())?;
    let mut failures = Vec::new();
    for row in rows {
        let (id, event, occurred_at) = row.map_err(|_| invalid())?;
        if event == "notification_retry_scheduled" {
            failures.push(AdminFailure {
                id,
                category: AdminFailureCategory::Dependency,
                occurred_at,
                retryable: true,
            });
        }
    }
    Ok(failures)
}

fn audit_event_from_row(row: &Row<'_>) -> rusqlite::Result<(i64, String, String)> {
    let id: i64 = row.get(0)?;
    let event: String = row.get(1)?;
    let occurred: String = row.get(2)?;
    if id < 1
        || !matches!(
            event.as_str(),
            "message_recorded"
                | "request_submitted"
                | "owner_task_submitted"
                | "proposal_created"
                | "proposal_accepted"
                | "proposal_declined"
                | "proposal_expired"
                | "proposal_promoted"
                | "notification_enqueued"
                | "notification_sent"
                | "notification_retry_scheduled"
                | "provider_cursor_advanced"
        )
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let occurred_at =
        timestamp(parse_timestamp(&occurred).map_err(|_| rusqlite::Error::InvalidQuery)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok((id, event, occurred_at))
}

fn missing(provider: AdminProvider) -> AdminConnection {
    AdminConnection {
        provider,
        configured: false,
        account_fingerprint: None,
        expires_at: None,
        status: AdminConnectionStatus::Missing,
    }
}
fn provider_name(provider: AdminProvider) -> &'static str {
    match provider {
        AdminProvider::Google => "google",
        AdminProvider::Outlook => "outlook",
    }
}
fn parse_timestamp(value: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(value, &Rfc3339)
        .or_else(|_| OffsetDateTime::parse(&format!("{}Z", value.replace(' ', "T")), &Rfc3339))
}
fn timestamp(value: OffsetDateTime) -> Result<String, time::error::Format> {
    value.to_offset(time::UtcOffset::UTC).format(&Rfc3339)
}
fn fingerprint(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = digest::digest(&digest::SHA256, value.as_bytes());
    let mut output = String::with_capacity(64);
    for byte in digest.as_ref() {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
    output
}
fn invalid() -> StoreError {
    StoreError::StoredRecordInvalid {
        resource: "admin projection",
    }
}
