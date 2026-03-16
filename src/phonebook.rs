//! Persistent caller phone-book storage and normalization helpers.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

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

/// A wildcard access-control entry that applies to any caller ID without an exact record.
pub const WILDCARD_CALLER_ID: &str = "*";

/// The special access-control entry for callers that do not present caller ID.
pub const NO_CALLER_ID_POLICY_KEY: &str = "__no_caller_id__";

/// The display label used when a call does not present caller ID.
pub const NO_CALLER_ID_DISPLAY: &str = "no-caller-id";

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
    pub disabled: bool,
    #[serde(default)]
    pub system_entry: bool,
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
            disabled: false,
            system_entry: false,
            first_name: None,
            last_name: None,
            email: None,
            company: None,
            timezone: None,
            preferred_language: None,
            notes: Vec::new(),
        }
    }

    fn new_system_entry(caller_id: &str, now: String, disabled: bool, notes: Vec<String>) -> Self {
        Self {
            caller_id: caller_id.to_string(),
            first_seen_at: now.clone(),
            last_seen_at: now,
            call_count: 0,
            disabled,
            system_entry: true,
            first_name: None,
            last_name: None,
            email: None,
            company: None,
            timezone: None,
            preferred_language: None,
            notes,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The phone-book policy record that decided whether an inbound call may proceed.
pub enum InboundPolicyMatch {
    Exact,
    Wildcard,
    NoCallerId,
}

#[derive(Debug, Clone)]
/// The resolved inbound access decision for a caller.
pub struct InboundAccessDecision {
    pub caller_id: Option<String>,
    pub allowed: bool,
    pub matched_policy: InboundPolicyMatch,
    pub matched_record_key: String,
    pub track_existing_caller: bool,
}

/// Thread-safe phone-book storage backed by a JSON file.
pub struct PhoneBookStore {
    path: PathBuf,
    phone_book: RwLock<PhoneBook>,
}

impl PhoneBookStore {
    /// Loads the phone book from disk, creating an empty store when absent.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let full_path = resolve_phone_book_path(path.into())?;

        let mut phone_book = if full_path.exists() {
            let raw = fs::read_to_string(&full_path).with_context(|| {
                format!("failed to read phone book {}", full_path.display())
            })?;
            serde_json::from_str(&raw).with_context(|| {
                format!("failed to parse phone book {}", full_path.display())
            })?
        } else {
            PhoneBook::default()
        };
        let seeded_entries = seed_policy_entries(&mut phone_book);
        sanitize_phone_book(&mut phone_book);
        if seeded_entries {
            persist_phone_book(&full_path, &phone_book)?;
        }

        Ok(Self {
            path: full_path,
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
        self.phone_book
            .read()
            .callers
            .get(caller_id)
            .filter(|caller| !caller.system_entry)
            .cloned()
    }

    /// Evaluates whether an inbound call from the supplied caller ID is allowed.
    pub fn inbound_access_decision(&self, raw_caller_id: &str) -> InboundAccessDecision {
        let caller_id = normalize_caller_id(raw_caller_id);
        let phone_book = self.phone_book.read();

        if let Some(caller_id) = caller_id {
            if let Some(record) = phone_book.callers.get(&caller_id) {
                return InboundAccessDecision {
                    caller_id: Some(caller_id.clone()),
                    allowed: !record.disabled,
                    matched_policy: InboundPolicyMatch::Exact,
                    matched_record_key: caller_id,
                    track_existing_caller: !record.disabled,
                };
            }

            let wildcard = phone_book.callers.get(WILDCARD_CALLER_ID);
            return InboundAccessDecision {
                caller_id: Some(caller_id),
                allowed: wildcard.map(|record| !record.disabled).unwrap_or(false),
                matched_policy: InboundPolicyMatch::Wildcard,
                matched_record_key: WILDCARD_CALLER_ID.to_string(),
                track_existing_caller: false,
            };
        }

        let no_caller_id = phone_book.callers.get(NO_CALLER_ID_POLICY_KEY);
        InboundAccessDecision {
            caller_id: None,
            allowed: no_caller_id.map(|record| !record.disabled).unwrap_or(false),
            matched_policy: InboundPolicyMatch::NoCallerId,
            matched_record_key: NO_CALLER_ID_POLICY_KEY.to_string(),
            track_existing_caller: false,
        }
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

fn resolve_phone_book_path(configured: PathBuf) -> Result<PathBuf> {
    if configured.as_os_str().is_empty() {
        return Err(anyhow::anyhow!("phone book path must not be empty"));
    }

    reject_parent_dir_components(&configured)?;
    if configured.is_absolute() {
        return resolve_absolute_phone_book_path(&configured);
    }

    let base_dir = std::env::current_dir()
        .context("failed to determine current working directory for phone book")?
        .canonicalize()
        .context("failed to resolve current working directory for phone book")?;
    let full_path = base_dir.join(&configured);
    if full_path.exists() {
        let canonical = full_path
            .canonicalize()
            .with_context(|| format!("failed to resolve phone book path {}", full_path.display()))?;
        if !canonical.starts_with(&base_dir) {
            return Err(anyhow::anyhow!(
                "phone book path must reside under {}",
                base_dir.display()
            ));
        }
        Ok(canonical)
    } else {
        Ok(full_path)
    }
}

fn resolve_absolute_phone_book_path(configured: &Path) -> Result<PathBuf> {
    match configured.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let (canonical_parent, relative_tail) = canonical_existing_ancestor(configured)?;
            Ok(canonical_parent.join(relative_tail))
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to resolve phone book path {}", configured.display())),
    }
}

fn canonical_existing_ancestor(path: &Path) -> Result<(PathBuf, PathBuf)> {
    let mut existing = path;
    let mut tail = Vec::new();
    while !existing.exists() {
        let component = existing.file_name().with_context(|| {
            format!(
                "phone book path {} must include a file name",
                path.display()
            )
        })?;
        tail.push(component.to_os_string());
        existing = existing.parent().with_context(|| {
            format!(
                "phone book path {} must include an existing parent directory",
                path.display()
            )
        })?;
    }

    let canonical_parent = existing
        .canonicalize()
        .with_context(|| format!("failed to resolve phone book path {}", existing.display()))?;
    let mut relative_tail = PathBuf::new();
    for component in tail.into_iter().rev() {
        relative_tail.push(component);
    }
    Ok((canonical_parent, relative_tail))
}

fn reject_parent_dir_components(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(anyhow::anyhow!(
            "phone book path must not contain parent directory traversal"
        ));
    }
    Ok(())
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

fn seed_policy_entries(phone_book: &mut PhoneBook) -> bool {
    let now = now_rfc3339();
    let mut changed = false;
    phone_book
        .callers
        .entry(WILDCARD_CALLER_ID.to_string())
        .or_insert_with(|| {
            changed = true;
            CallerRecord::new_system_entry(
                WILDCARD_CALLER_ID,
                now.clone(),
                true,
                vec![
                    "System policy entry for callers that do present caller ID but do not have an exact record."
                        .to_string(),
                    "Disabled by default to enforce deny-by-default inbound access.".to_string(),
                ],
            )
        });
    phone_book
        .callers
        .entry(NO_CALLER_ID_POLICY_KEY.to_string())
        .or_insert_with(|| {
            changed = true;
            CallerRecord::new_system_entry(
                NO_CALLER_ID_POLICY_KEY,
                now,
                true,
                vec![
                    "System policy entry for callers that do not present caller ID.".to_string(),
                    "Disabled by default to enforce deny-by-default inbound access.".to_string(),
                ],
            )
        });
    changed
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

/// Normalizes a SIP caller ID into a stable phone-book key.
pub fn normalize_caller_id(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim()
        .trim_matches(|ch: char| matches!(ch, '<' | '>' | '"' | '\''));
    if trimmed.is_empty() {
        return None;
    }

    let lowered = trimmed.to_ascii_lowercase();
    if matches!(
        lowered.as_str(),
        "anonymous" | "unknown" | "unavailable" | "private" | "restricted"
    ) {
        return None;
    }

    let without_scheme = if let Some(rest) = lowered.strip_prefix("sip:") {
        &trimmed[trimmed.len() - rest.len()..]
    } else if let Some(rest) = lowered.strip_prefix("sips:") {
        &trimmed[trimmed.len() - rest.len()..]
    } else if let Some(rest) = lowered.strip_prefix("tel:") {
        &trimmed[trimmed.len() - rest.len()..]
    } else {
        trimmed
    };

    let without_params = without_scheme
        .split([';', '?'])
        .next()
        .unwrap_or(without_scheme);
    let user_part = without_params
        .split('@')
        .next()
        .unwrap_or(without_params)
        .trim();
    if user_part.is_empty() {
        return None;
    }

    let normalized = user_part
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '_' | '.'))
        .collect::<String>();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Returns the caller label used in logs and call snapshots.
pub fn caller_id_display(raw: &str) -> String {
    normalize_caller_id(raw).unwrap_or_else(|| NO_CALLER_ID_DISPLAY.to_string())
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

    #[test]
    fn load_seeds_policy_entries_with_deny_defaults() {
        let temp_dir = std::env::temp_dir().join("agent_voice_phonebook_policy_test.json");
        let _ = fs::remove_file(&temp_dir);
        let store = PhoneBookStore::load(&temp_dir).expect("store");

        let wildcard = store
            .phone_book
            .read()
            .callers
            .get(WILDCARD_CALLER_ID)
            .cloned()
            .expect("wildcard entry");
        let no_caller_id = store
            .phone_book
            .read()
            .callers
            .get(NO_CALLER_ID_POLICY_KEY)
            .cloned()
            .expect("no caller id entry");

        assert!(wildcard.disabled);
        assert!(wildcard.system_entry);
        assert!(no_caller_id.disabled);
        assert!(no_caller_id.system_entry);

        let _ = fs::remove_file(&temp_dir);
    }

    #[test]
    fn inbound_access_denies_unknown_callers_by_default() {
        let temp_dir = std::env::temp_dir().join("agent_voice_phonebook_access_default.json");
        let _ = fs::remove_file(&temp_dir);
        let store = PhoneBookStore::load(&temp_dir).expect("store");

        let decision = store.inbound_access_decision("sip:61415850000@example.com");
        assert!(!decision.allowed);
        assert_eq!(decision.matched_policy, InboundPolicyMatch::Wildcard);
        assert_eq!(decision.caller_id.as_deref(), Some("61415850000"));

        let anonymous = store.inbound_access_decision("anonymous");
        assert!(!anonymous.allowed);
        assert_eq!(anonymous.matched_policy, InboundPolicyMatch::NoCallerId);
        assert!(anonymous.caller_id.is_none());

        let _ = fs::remove_file(&temp_dir);
    }

    #[test]
    fn inbound_access_honors_exact_and_wildcard_records() {
        let temp_dir = std::env::temp_dir().join("agent_voice_phonebook_access_exact.json");
        let _ = fs::remove_file(&temp_dir);
        let store = PhoneBookStore::load(&temp_dir).expect("store");

        store
            .merge_update(
                "61415850000",
                CallerUpdate {
                    first_name: Some("David".to_string()),
                    ..Default::default()
                },
            )
            .expect("seed explicit caller");

        let exact = store.inbound_access_decision("sip:61415850000@example.com");
        assert!(exact.allowed);
        assert_eq!(exact.matched_policy, InboundPolicyMatch::Exact);
        assert!(exact.track_existing_caller);

        {
            let mut phone_book = store.phone_book.write();
            phone_book
                .callers
                .get_mut("61415850000")
                .expect("explicit record")
                .disabled = true;
        }
        let denied_exact = store.inbound_access_decision("61415850000");
        assert!(!denied_exact.allowed);
        assert_eq!(denied_exact.matched_policy, InboundPolicyMatch::Exact);

        {
            let mut phone_book = store.phone_book.write();
            phone_book
                .callers
                .get_mut(WILDCARD_CALLER_ID)
                .expect("wildcard record")
                .disabled = false;
        }
        let wildcard = store.inbound_access_decision("sip:61419990000@example.com");
        assert!(wildcard.allowed);
        assert_eq!(wildcard.matched_policy, InboundPolicyMatch::Wildcard);
        assert!(!wildcard.track_existing_caller);

        let _ = fs::remove_file(&temp_dir);
    }

    #[test]
    fn normalize_caller_id_parses_sip_and_tel_values() {
        assert_eq!(
            normalize_caller_id("sip:61415850000@example.com"),
            Some("61415850000".to_string())
        );
        assert_eq!(
            normalize_caller_id("tel:+61415850000"),
            Some("+61415850000".to_string())
        );
        assert_eq!(normalize_caller_id("anonymous"), None);
        assert_eq!(caller_id_display("anonymous"), NO_CALLER_ID_DISPLAY);
    }

    #[test]
    fn load_rejects_parent_dir_traversal() {
        let attempted = PathBuf::from("../agent_voice_phonebook_escape.json");
        let error = match PhoneBookStore::load(attempted) {
            Ok(_) => panic!("traversal should fail"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("must not contain parent directory traversal")
        );
    }

    #[test]
    fn load_allows_absolute_paths_with_missing_leaf() {
        let temp_root = std::env::temp_dir().join(format!(
            "agent_voice_phonebook_absolute_{}",
            std::process::id()
        ));
        let nested_path = temp_root.join("nested").join("phone_book.json");
        let _ = fs::remove_dir_all(&temp_root);

        let store = PhoneBookStore::load(&nested_path).expect("absolute path should load");
        store.touch_caller("61412345678").expect("touch");
        assert!(nested_path.exists());

        let _ = fs::remove_dir_all(&temp_root);
    }
}
