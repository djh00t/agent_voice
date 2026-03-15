//! Persistent caller phone-book storage and normalization helpers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// The mutable fields that the assistant may manage for the active caller.
pub const EDITABLE_CALLER_FIELDS: &[&str] = &[
    "first_name",
    "last_name",
    "email",
    "company",
    "timezone",
    "preferred_language",
    "notes",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// The on-disk phone-book document.
pub struct PhoneBook {
    #[serde(default)]
    pub callers: HashMap<String, CallerRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A single caller record keyed by caller ID.
pub struct CallerRecord {
    pub caller_id: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub call_count: u64,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub preferred_language: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl CallerRecord {
    fn new(caller_id: &str, now: String) -> Self {
        Self {
            caller_id: caller_id.to_string(),
            first_seen_at: now.clone(),
            last_seen_at: now,
            call_count: 0,
            first_name: None,
            last_name: None,
            email: None,
            company: None,
            timezone: None,
            preferred_language: None,
            notes: Vec::new(),
        }
    }

    pub fn missing_fields(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.first_name.is_none() {
            missing.push("first_name");
        }
        if self.last_name.is_none() {
            missing.push("last_name");
        }
        if self.email.is_none() {
            missing.push("email");
        }
        if self.company.is_none() {
            missing.push("company");
        }
        missing
    }

    fn sanitize(&mut self) {
        self.notes.retain(|note| !note_mentions_language(note));
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// A partial update to a caller record.
pub struct CallerUpdate {
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub preferred_language: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Thread-safe phone-book storage backed by a JSON file.
pub struct PhoneBookStore {
    path: PathBuf,
    phone_book: RwLock<PhoneBook>,
}

impl PhoneBookStore {
    /// Loads the phone book from disk, creating an empty store when absent.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut phone_book = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read phone book {}", path.display()))?;
            serde_json::from_str(&raw)
                .with_context(|| format!("failed to parse phone book {}", path.display()))?
        } else {
            PhoneBook::default()
        };
        sanitize_phone_book(&mut phone_book);

        Ok(Self {
            path,
            phone_book: RwLock::new(phone_book),
        })
    }

    /// Touches a caller record for an inbound or outbound call occurrence.
    pub fn touch_caller(&self, caller_id: &str) -> Result<CallerRecord> {
        let now = now_rfc3339();
        let snapshot = {
            let mut phone_book = self.phone_book.write();
            let caller = phone_book
                .callers
                .entry(caller_id.to_string())
                .or_insert_with(|| CallerRecord::new(caller_id, now.clone()));
            caller.last_seen_at = now;
            caller.call_count += 1;
            caller.clone()
        };
        self.persist()?;
        Ok(snapshot)
    }

    /// Returns a cloned caller record by caller ID.
    pub fn get(&self, caller_id: &str) -> Option<CallerRecord> {
        self.phone_book.read().callers.get(caller_id).cloned()
    }

    /// Merges a partial caller update into the stored record and persists it.
    pub fn merge_update(&self, caller_id: &str, update: CallerUpdate) -> Result<CallerRecord> {
        let snapshot = {
            let mut phone_book = self.phone_book.write();
            let caller = phone_book
                .callers
                .entry(caller_id.to_string())
                .or_insert_with(|| CallerRecord::new(caller_id, now_rfc3339()));

            merge_option(&mut caller.first_name, update.first_name);
            merge_option(&mut caller.last_name, update.last_name);
            merge_option(
                &mut caller.email,
                update.email.map(|value| value.to_ascii_lowercase()),
            );
            merge_option(&mut caller.company, update.company);
            merge_option(&mut caller.timezone, update.timezone);
            merge_option(&mut caller.preferred_language, update.preferred_language);
            merge_notes(&mut caller.notes, update.notes);
            caller.sanitize();
            caller.last_seen_at = now_rfc3339();
            caller.clone()
        };
        self.persist()?;
        Ok(snapshot)
    }

    /// Returns all caller records currently in the phone book.
    pub fn all(&self) -> Vec<CallerRecord> {
        self.phone_book.read().callers.values().cloned().collect()
    }

    fn persist(&self) -> Result<()> {
        persist_phone_book(&self.path, &self.phone_book.read())
    }
}

fn persist_phone_book(path: &Path, phone_book: &PhoneBook) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create phone book dir {}", parent.display()))?;
    }
    let tmp_path = path.with_extension("json.tmp");
    let payload =
        serde_json::to_vec_pretty(phone_book).context("failed to serialize phone book")?;
    fs::write(&tmp_path, payload)
        .with_context(|| format!("failed to write phone book tmp {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("failed to replace phone book {}", path.display()))?;
    Ok(())
}

fn sanitize_phone_book(phone_book: &mut PhoneBook) {
    for caller in phone_book.callers.values_mut() {
        caller.sanitize();
    }
}

fn merge_option(target: &mut Option<String>, candidate: Option<String>) {
    if let Some(value) = candidate.map(|value| value.trim().to_string())
        && !value.is_empty()
    {
        *target = Some(value);
    }
}

fn merge_notes(target: &mut Vec<String>, notes: Vec<String>) {
    for note in notes {
        let trimmed = note.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !target
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            target.push(trimmed.to_string());
        }
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Returns the set of editable phone-book fields.
pub fn editable_field_names() -> &'static [&'static str] {
    EDITABLE_CALLER_FIELDS
}

/// Normalizes an email candidate and rejects obviously invalid values.
pub fn normalize_email_candidate(value: &str) -> Option<String> {
    let normalized = value
        .trim()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '.' | ',' | ';' | ':'))
        .to_ascii_lowercase();
    if looks_like_email(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

/// Returns `true` when a string looks like a usable email address.
pub fn looks_like_email(value: &str) -> bool {
    let mut parts = value.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return false;
    }
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '@' | '.' | '_' | '-' | '+'))
}

/// Returns `true` when a value is a valid IANA timezone identifier.
pub fn is_valid_timezone(value: &str) -> bool {
    value.parse::<chrono_tz::Tz>().is_ok()
}

fn note_mentions_language(note: &str) -> bool {
    let normalized = note.to_ascii_lowercase();
    normalized.contains(" language")
        || normalized.starts_with("language ")
        || normalized.contains("speaks ")
        || normalized.contains("spoke ")
        || normalized.contains("used ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_update_fills_missing_fields() {
        let temp_dir = std::env::temp_dir().join("agent_voice_phonebook_test.json");
        let _ = fs::remove_file(&temp_dir);
        let store = PhoneBookStore::load(&temp_dir).expect("store");
        let _ = store.touch_caller("6140000").expect("touch");
        let updated = store
            .merge_update(
                "6140000",
                CallerUpdate {
                    first_name: Some("Dave".to_string()),
                    last_name: Some("Smith".to_string()),
                    email: Some("DAVE@example.com".to_string()),
                    company: None,
                    timezone: Some("Australia/Sydney".to_string()),
                    preferred_language: Some("English".to_string()),
                    notes: vec!["Interested in support".to_string()],
                },
            )
            .expect("merge");

        assert_eq!(updated.first_name.as_deref(), Some("Dave"));
        assert_eq!(updated.last_name.as_deref(), Some("Smith"));
        assert_eq!(updated.email.as_deref(), Some("dave@example.com"));
        assert_eq!(updated.timezone.as_deref(), Some("Australia/Sydney"));
        assert_eq!(updated.preferred_language.as_deref(), Some("English"));
        assert_eq!(updated.notes, vec!["Interested in support"]);

        let _ = fs::remove_file(&temp_dir);
    }
}
