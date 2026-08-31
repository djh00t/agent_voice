//! Typed, redacted configuration read and compare-and-set updates.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use chrono_tz::Tz;
use rusqlite::{OptionalExtension, Row, Transaction, TransactionBehavior, params, types::Value};
use serde::de::{Error as DeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use time::{Time, format_description::FormatItem, macros::format_description};

use crate::pa::store::{MAX_TASK_DURATION_MINUTES, PaStore, StoreError, StoreResult};

const CONFIGURATION_ID: i64 = 1;
const MODEL_MAX_BYTES: usize = 128;
const HH_MM: &[FormatItem<'static>] = format_description!("[hour]:[minute]");

/// The closed weekday values accepted by the admin configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkingDay {
    #[default]
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl<'de> Deserialize<'de> for WorkingDay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct WorkingDayVisitor;

        impl<'de> Visitor<'de> for WorkingDayVisitor {
            type Value = WorkingDay;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a supported lowercase working day")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                WorkingDay::parse_storage(value)
                    .ok_or_else(|| E::custom("working_days contains an unsupported value"))
            }
        }

        deserializer.deserialize_str(WorkingDayVisitor)
    }
}

impl WorkingDay {
    /// Returns the stable lowercase storage and wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Monday => "monday",
            Self::Tuesday => "tuesday",
            Self::Wednesday => "wednesday",
            Self::Thursday => "thursday",
            Self::Friday => "friday",
            Self::Saturday => "saturday",
            Self::Sunday => "sunday",
        }
    }

    fn parse_storage(value: &str) -> Option<Self> {
        match value {
            "monday" => Some(Self::Monday),
            "tuesday" => Some(Self::Tuesday),
            "wednesday" => Some(Self::Wednesday),
            "thursday" => Some(Self::Thursday),
            "friday" => Some(Self::Friday),
            "saturday" => Some(Self::Saturday),
            "sunday" => Some(Self::Sunday),
            _ => None,
        }
    }
}

/// The five bounded task durations in minutes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDurations {
    pub bill: i64,
    pub callback: i64,
    pub reading: i64,
    pub email_reply: i64,
    pub preparation: i64,
}

/// Optional task-duration members accepted by an admin patch.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDurationsPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub bill: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub callback: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub reading: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub email_reply: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub preparation: Option<i64>,
}

impl fmt::Debug for TaskDurationsPatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TaskDurationsPatch")
            .field("bill", &self.bill.is_some())
            .field("callback", &self.callback.is_some())
            .field("reading", &self.reading.is_some())
            .field("email_reply", &self.email_reply.is_some())
            .field("preparation", &self.preparation.is_some())
            .finish()
    }
}

impl TaskDurationsPatch {
    fn is_empty(&self) -> bool {
        self.bill.is_none()
            && self.callback.is_none()
            && self.reading.is_none()
            && self.email_reply.is_none()
            && self.preparation.is_none()
    }

    fn validate(&self) -> StoreResult<()> {
        for (value, field) in [
            (self.bill, "task_duration_bill_minutes"),
            (self.callback, "task_duration_callback_minutes"),
            (self.reading, "task_duration_reading_minutes"),
            (self.email_reply, "task_duration_email_reply_minutes"),
            (self.preparation, "task_duration_preparation_minutes"),
        ] {
            if let Some(value) = value {
                validate_task_duration_minutes(value, field)?;
            }
        }
        Ok(())
    }
}

/// The safe, fixed-column admin configuration projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    pub version: i64,
    pub owner_timezone: Option<String>,
    pub working_days: Vec<WorkingDay>,
    pub working_window_start: String,
    pub working_window_end: String,
    pub minimum_notice_minutes: i64,
    pub booking_horizon_days: i64,
    pub meeting_buffer_minutes: i64,
    pub retention_days: i64,
    pub task_duration_minutes: TaskDurations,
    pub model: String,
    pub updated_at: String,
}

/// A non-empty, typed allowlist of configuration changes.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfigPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub owner_timezone: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub working_days: Option<Vec<WorkingDay>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub working_window_start: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub working_window_end: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub minimum_notice_minutes: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub booking_horizon_days: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub meeting_buffer_minutes: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub retention_days: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub task_duration_minutes: Option<TaskDurationsPatch>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reject_null"
    )]
    pub model: Option<String>,
}

impl fmt::Debug for AdminConfigPatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminConfigPatch")
            .field("owner_timezone", &self.owner_timezone.is_some())
            .field("working_days", &self.working_days.is_some())
            .field("working_window_start", &self.working_window_start.is_some())
            .field("working_window_end", &self.working_window_end.is_some())
            .field(
                "minimum_notice_minutes",
                &self.minimum_notice_minutes.is_some(),
            )
            .field("booking_horizon_days", &self.booking_horizon_days.is_some())
            .field(
                "meeting_buffer_minutes",
                &self.meeting_buffer_minutes.is_some(),
            )
            .field("retention_days", &self.retention_days.is_some())
            .field(
                "task_duration_minutes",
                &self.task_duration_minutes.is_some(),
            )
            .field("model", &self.model.is_some())
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminConfigPatchWire {
    #[serde(default, deserialize_with = "reject_null")]
    owner_timezone: Option<String>,
    #[serde(default, deserialize_with = "reject_null")]
    working_days: Option<Vec<WorkingDay>>,
    #[serde(default, deserialize_with = "reject_null")]
    working_window_start: Option<String>,
    #[serde(default, deserialize_with = "reject_null")]
    working_window_end: Option<String>,
    #[serde(default, deserialize_with = "reject_null")]
    minimum_notice_minutes: Option<i64>,
    #[serde(default, deserialize_with = "reject_null")]
    booking_horizon_days: Option<i64>,
    #[serde(default, deserialize_with = "reject_null")]
    meeting_buffer_minutes: Option<i64>,
    #[serde(default, deserialize_with = "reject_null")]
    retention_days: Option<i64>,
    #[serde(default, deserialize_with = "reject_null")]
    task_duration_minutes: Option<TaskDurationsPatch>,
    #[serde(default, deserialize_with = "reject_null")]
    model: Option<String>,
}

impl<'de> Deserialize<'de> for AdminConfigPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AdminConfigPatchWire::deserialize(deserializer)?;
        let patch = Self {
            owner_timezone: wire.owner_timezone,
            working_days: wire.working_days,
            working_window_start: wire.working_window_start,
            working_window_end: wire.working_window_end,
            minimum_notice_minutes: wire.minimum_notice_minutes,
            booking_horizon_days: wire.booking_horizon_days,
            meeting_buffer_minutes: wire.meeting_buffer_minutes,
            retention_days: wire.retention_days,
            task_duration_minutes: wire.task_duration_minutes,
            model: wire.model,
        };
        patch
            .validate()
            .map_err(|error| DeError::custom(error.to_string()))?;
        Ok(patch)
    }
}

impl AdminConfigPatch {
    fn validate(&self) -> StoreResult<()> {
        if self.owner_timezone.is_none()
            && self.working_days.is_none()
            && self.working_window_start.is_none()
            && self.working_window_end.is_none()
            && self.minimum_notice_minutes.is_none()
            && self.booking_horizon_days.is_none()
            && self.meeting_buffer_minutes.is_none()
            && self.retention_days.is_none()
            && self.task_duration_minutes.is_none()
            && self.model.is_none()
        {
            return Err(invalid("patch"));
        }
        if let Some(timezone) = &self.owner_timezone {
            validate_timezone(timezone, "owner_timezone")?;
        }
        if let Some(days) = &self.working_days {
            validate_working_days(days, "working_days")?;
        }
        if let Some(start) = &self.working_window_start {
            validate_time(start, "working_window_start")?;
        }
        if let Some(end) = &self.working_window_end {
            validate_time(end, "working_window_end")?;
        }
        if let Some(value) = self.minimum_notice_minutes {
            validate_non_negative(value, "minimum_notice_minutes")?;
        }
        if let Some(value) = self.booking_horizon_days {
            validate_positive(value, "booking_horizon_days")?;
        }
        if let Some(value) = self.meeting_buffer_minutes {
            validate_non_negative(value, "meeting_buffer_minutes")?;
        }
        if let Some(value) = self.retention_days {
            validate_positive(value, "retention_days")?;
        }
        if let Some(durations) = &self.task_duration_minutes {
            if durations.is_empty() {
                return Err(invalid("task_duration_minutes"));
            }
            durations.validate()?;
        }
        if let Some(model) = &self.model {
            validate_model(model, "model")?;
        }
        Ok(())
    }

    fn apply(&self, current: &AdminConfig) -> StoreResult<AdminConfig> {
        let mut next = current.clone();
        if let Some(timezone) = &self.owner_timezone {
            next.owner_timezone = Some(validate_timezone(timezone, "owner_timezone")?);
        }
        if let Some(days) = &self.working_days {
            validate_working_days(days, "working_days")?;
            next.working_days = days.clone();
        }
        if let Some(start) = &self.working_window_start {
            validate_time(start, "working_window_start")?;
            next.working_window_start = start.clone();
        }
        if let Some(end) = &self.working_window_end {
            validate_time(end, "working_window_end")?;
            next.working_window_end = end.clone();
        }
        if let (Ok(start), Ok(end)) = (
            Time::parse(&next.working_window_start, HH_MM),
            Time::parse(&next.working_window_end, HH_MM),
        ) {
            if end <= start {
                return Err(invalid("working_window"));
            }
        } else {
            return Err(invalid("working_window"));
        }
        if let Some(value) = self.minimum_notice_minutes {
            next.minimum_notice_minutes = value;
        }
        if let Some(value) = self.booking_horizon_days {
            next.booking_horizon_days = value;
        }
        if let Some(value) = self.meeting_buffer_minutes {
            next.meeting_buffer_minutes = value;
        }
        if let Some(value) = self.retention_days {
            next.retention_days = value;
        }
        if let Some(durations) = &self.task_duration_minutes {
            if let Some(value) = durations.bill {
                next.task_duration_minutes.bill = value;
            }
            if let Some(value) = durations.callback {
                next.task_duration_minutes.callback = value;
            }
            if let Some(value) = durations.reading {
                next.task_duration_minutes.reading = value;
            }
            if let Some(value) = durations.email_reply {
                next.task_duration_minutes.email_reply = value;
            }
            if let Some(value) = durations.preparation {
                next.task_duration_minutes.preparation = value;
            }
        }
        if let Some(model) = &self.model {
            next.model = model.clone();
        }
        validate_config(&next)?;
        Ok(next)
    }
}

/// The typed configuration producer shared by later admin adapters.
#[derive(Clone)]
pub struct AdminConfigStore {
    store: Arc<PaStore>,
}

impl fmt::Debug for AdminConfigStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AdminConfigStore").finish()
    }
}

impl AdminConfigStore {
    /// Creates a producer around the shared encrypted PA store.
    pub fn new(store: Arc<PaStore>) -> Self {
        Self { store }
    }

    /// Reads the singleton configuration using fixed, allowlisted columns.
    pub fn read(&self) -> StoreResult<AdminConfig> {
        let raw = query_raw(self.store.connection()).map_err(|_| config_invalid())?;
        decode_config(raw)
    }

    /// Applies one non-empty typed patch when the durable version matches.
    pub fn update_config(
        &self,
        expected_version: i64,
        patch: AdminConfigPatch,
    ) -> StoreResult<AdminConfig> {
        patch.validate()?;
        patch.apply(&self.read()?)?;

        let transaction =
            Transaction::new_unchecked(self.store.connection(), TransactionBehavior::Immediate)
                .map_err(|_| config_invalid())?;
        let raw_version: Value = transaction
            .query_row(
                "SELECT version FROM configuration WHERE id = ?1",
                [CONFIGURATION_ID],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| config_invalid())?
            .ok_or_else(config_invalid)?;
        let current_version = integer_value(&raw_version).ok_or_else(config_invalid)?;
        if current_version < 1 {
            return Err(config_invalid());
        }
        if current_version != expected_version {
            return Err(StoreError::CursorConflict {
                resource: "configuration",
            });
        }

        let current = decode_config(query_raw(&transaction).map_err(|_| config_invalid())?)?;
        let next = patch.apply(&current)?;
        let updated = transaction
            .execute(
                "UPDATE configuration
                 SET owner_timezone = ?1,
                     working_days = ?2,
                     working_window_start = ?3,
                     working_window_end = ?4,
                     minimum_notice_minutes = ?5,
                     booking_horizon_days = ?6,
                     meeting_buffer_minutes = ?7,
                     retention_days = ?8,
                     task_duration_bill_minutes = ?9,
                     task_duration_callback_minutes = ?10,
                     task_duration_reading_minutes = ?11,
                     task_duration_email_reply_minutes = ?12,
                     task_duration_preparation_minutes = ?13,
                     email_triage_model = ?14,
                     version = version + 1,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
                 WHERE id = ?15 AND version = ?16",
                params![
                    next.owner_timezone,
                    working_days_storage(&next.working_days),
                    next.working_window_start,
                    next.working_window_end,
                    next.minimum_notice_minutes,
                    next.booking_horizon_days,
                    next.meeting_buffer_minutes,
                    next.retention_days,
                    next.task_duration_minutes.bill,
                    next.task_duration_minutes.callback,
                    next.task_duration_minutes.reading,
                    next.task_duration_minutes.email_reply,
                    next.task_duration_minutes.preparation,
                    next.model,
                    CONFIGURATION_ID,
                    expected_version,
                ],
            )
            .map_err(|_| config_invalid())?;
        if updated != 1 {
            return Err(StoreError::CursorConflict {
                resource: "configuration",
            });
        }
        let committed = decode_config(query_raw(&transaction).map_err(|_| config_invalid())?)?;
        transaction.commit().map_err(|_| config_invalid())?;
        Ok(committed)
    }
}

struct RawConfig {
    version: Value,
    owner_timezone: Value,
    working_days: Value,
    working_window_start: Value,
    working_window_end: Value,
    minimum_notice_minutes: Value,
    booking_horizon_days: Value,
    meeting_buffer_minutes: Value,
    retention_days: Value,
    task_duration_bill_minutes: Value,
    task_duration_callback_minutes: Value,
    task_duration_reading_minutes: Value,
    task_duration_email_reply_minutes: Value,
    task_duration_preparation_minutes: Value,
    model: Value,
    updated_at: Value,
}

fn query_raw(connection: &rusqlite::Connection) -> rusqlite::Result<RawConfig> {
    connection
        .query_row(
            "SELECT version,
                    owner_timezone,
                    working_days,
                    working_window_start,
                    working_window_end,
                    minimum_notice_minutes,
                    booking_horizon_days,
                    meeting_buffer_minutes,
                    retention_days,
                    task_duration_bill_minutes,
                    task_duration_callback_minutes,
                    task_duration_reading_minutes,
                    task_duration_email_reply_minutes,
                    task_duration_preparation_minutes,
                    email_triage_model,
                    updated_at
             FROM configuration
             WHERE id = ?1",
            [CONFIGURATION_ID],
            raw_from_row,
        )
        .optional()
        .and_then(|raw| raw.ok_or(rusqlite::Error::QueryReturnedNoRows))
}

fn raw_from_row(row: &Row<'_>) -> rusqlite::Result<RawConfig> {
    Ok(RawConfig {
        version: row.get(0)?,
        owner_timezone: row.get(1)?,
        working_days: row.get(2)?,
        working_window_start: row.get(3)?,
        working_window_end: row.get(4)?,
        minimum_notice_minutes: row.get(5)?,
        booking_horizon_days: row.get(6)?,
        meeting_buffer_minutes: row.get(7)?,
        retention_days: row.get(8)?,
        task_duration_bill_minutes: row.get(9)?,
        task_duration_callback_minutes: row.get(10)?,
        task_duration_reading_minutes: row.get(11)?,
        task_duration_email_reply_minutes: row.get(12)?,
        task_duration_preparation_minutes: row.get(13)?,
        model: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn decode_config(raw: RawConfig) -> StoreResult<AdminConfig> {
    let version = integer_value(&raw.version).ok_or_else(config_invalid)?;
    let owner_timezone = optional_text(&raw.owner_timezone).ok_or_else(config_invalid)?;
    let owner_timezone = owner_timezone
        .map(|value| validate_timezone(&value, "owner_timezone"))
        .transpose()
        .map_err(|_| config_invalid())?;
    let working_days = text_value(&raw.working_days)
        .and_then(parse_working_days)
        .ok_or_else(config_invalid)?;
    let working_window_start = text_value(&raw.working_window_start)
        .and_then(canonical_time)
        .ok_or_else(config_invalid)?;
    let working_window_end = text_value(&raw.working_window_end)
        .and_then(canonical_time)
        .ok_or_else(config_invalid)?;
    let start = Time::parse(&working_window_start, HH_MM).map_err(|_| config_invalid())?;
    let end = Time::parse(&working_window_end, HH_MM).map_err(|_| config_invalid())?;
    if end <= start {
        return Err(config_invalid());
    }
    let minimum_notice_minutes =
        integer_value(&raw.minimum_notice_minutes).ok_or_else(config_invalid)?;
    let booking_horizon_days =
        integer_value(&raw.booking_horizon_days).ok_or_else(config_invalid)?;
    let meeting_buffer_minutes =
        integer_value(&raw.meeting_buffer_minutes).ok_or_else(config_invalid)?;
    let retention_days = integer_value(&raw.retention_days).ok_or_else(config_invalid)?;
    let durations = TaskDurations {
        bill: integer_value(&raw.task_duration_bill_minutes).ok_or_else(config_invalid)?,
        callback: integer_value(&raw.task_duration_callback_minutes).ok_or_else(config_invalid)?,
        reading: integer_value(&raw.task_duration_reading_minutes).ok_or_else(config_invalid)?,
        email_reply: integer_value(&raw.task_duration_email_reply_minutes)
            .ok_or_else(config_invalid)?,
        preparation: integer_value(&raw.task_duration_preparation_minutes)
            .ok_or_else(config_invalid)?,
    };
    let model = text_value(&raw.model)
        .ok_or_else(config_invalid)?
        .to_owned();
    let updated_at = text_value(&raw.updated_at)
        .and_then(canonical_timestamp)
        .ok_or_else(config_invalid)?;
    let config = AdminConfig {
        version,
        owner_timezone,
        working_days,
        working_window_start,
        working_window_end,
        minimum_notice_minutes,
        booking_horizon_days,
        meeting_buffer_minutes,
        retention_days,
        task_duration_minutes: durations,
        model,
        updated_at,
    };
    validate_config(&config).map_err(|_| config_invalid())?;
    Ok(config)
}

fn validate_config(config: &AdminConfig) -> StoreResult<()> {
    validate_positive(config.version, "version")?;
    if let Some(timezone) = &config.owner_timezone {
        validate_timezone(timezone, "owner_timezone")?;
    }
    validate_working_days(&config.working_days, "working_days")?;
    validate_time(&config.working_window_start, "working_window_start")?;
    validate_time(&config.working_window_end, "working_window_end")?;
    let start = Time::parse(&config.working_window_start, HH_MM).map_err(|_| config_invalid())?;
    let end = Time::parse(&config.working_window_end, HH_MM).map_err(|_| config_invalid())?;
    if end <= start {
        return Err(config_invalid());
    }
    validate_non_negative(config.minimum_notice_minutes, "minimum_notice_minutes")?;
    validate_positive(config.booking_horizon_days, "booking_horizon_days")?;
    validate_non_negative(config.meeting_buffer_minutes, "meeting_buffer_minutes")?;
    validate_positive(config.retention_days, "retention_days")?;
    for (value, field) in [
        (
            config.task_duration_minutes.bill,
            "task_duration_bill_minutes",
        ),
        (
            config.task_duration_minutes.callback,
            "task_duration_callback_minutes",
        ),
        (
            config.task_duration_minutes.reading,
            "task_duration_reading_minutes",
        ),
        (
            config.task_duration_minutes.email_reply,
            "task_duration_email_reply_minutes",
        ),
        (
            config.task_duration_minutes.preparation,
            "task_duration_preparation_minutes",
        ),
    ] {
        validate_task_duration_minutes(value, field)?;
    }
    validate_model(&config.model, "model")?;
    if canonical_timestamp(&config.updated_at).is_none() {
        return Err(config_invalid());
    }
    Ok(())
}

fn parse_working_days(value: &str) -> Option<Vec<WorkingDay>> {
    let days = value
        .split(',')
        .map(WorkingDay::parse_storage)
        .collect::<Option<Vec<_>>>()?;
    if days.is_empty() || has_duplicate_days(&days) {
        None
    } else {
        Some(days)
    }
}

fn validate_working_days(days: &[WorkingDay], field: &'static str) -> StoreResult<()> {
    if days.is_empty() || has_duplicate_days(days) {
        Err(invalid(field))
    } else {
        Ok(())
    }
}

fn has_duplicate_days(days: &[WorkingDay]) -> bool {
    days.iter()
        .enumerate()
        .any(|(index, day)| days[..index].contains(day))
}

fn working_days_storage(days: &[WorkingDay]) -> String {
    days.iter()
        .map(|day| day.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn canonical_time(value: &str) -> Option<String> {
    let parsed = Time::parse(value, HH_MM).ok()?;
    let canonical = parsed.format(HH_MM).ok()?;
    (canonical == value).then_some(canonical)
}

fn validate_time(value: &str, field: &'static str) -> StoreResult<()> {
    if canonical_time(value).is_some() {
        Ok(())
    } else {
        Err(invalid(field))
    }
}

fn validate_timezone(value: &str, field: &'static str) -> StoreResult<String> {
    if value.is_empty() || value.trim() != value || value.parse::<Tz>().is_err() {
        Err(invalid(field))
    } else {
        Ok(value.to_owned())
    }
}

fn validate_model(value: &str, field: &'static str) -> StoreResult<()> {
    if value.is_empty()
        || value.len() > MODEL_MAX_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        Err(invalid(field))
    } else {
        Ok(())
    }
}

fn validate_non_negative(value: i64, field: &'static str) -> StoreResult<()> {
    if value < 0 {
        Err(invalid(field))
    } else {
        Ok(())
    }
}

fn validate_positive(value: i64, field: &'static str) -> StoreResult<()> {
    if value < 1 {
        Err(invalid(field))
    } else {
        Ok(())
    }
}

fn validate_task_duration_minutes(value: i64, field: &'static str) -> StoreResult<()> {
    validate_positive(value, field)?;
    if value > MAX_TASK_DURATION_MINUTES {
        Err(invalid(field))
    } else {
        Ok(())
    }
}

fn canonical_timestamp(value: &str) -> Option<String> {
    let parsed = if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        parsed.with_timezone(&Utc)
    } else {
        let parsed = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()?;
        DateTime::<Utc>::from_naive_utc_and_offset(parsed, Utc)
    };
    Some(parsed.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn text_value(value: &Value) -> Option<&str> {
    match value {
        Value::Text(value) => Some(value),
        _ => None,
    }
}

fn optional_text(value: &Value) -> Option<Option<String>> {
    match value {
        Value::Null => Some(None),
        Value::Text(value) => Some(Some(value.clone())),
        _ => None,
    }
}

fn integer_value(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        _ => None,
    }
}

fn invalid(field: &'static str) -> StoreError {
    StoreError::InvalidInput { field }
}

fn config_invalid() -> StoreError {
    StoreError::StoredRecordInvalid {
        resource: "configuration",
    }
}

fn reject_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)?
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("null is invalid"))
}

#[cfg(test)]
#[path = "admin_config_tests.rs"]
mod tests;
