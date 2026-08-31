use agent_voice::pa::{OAuthCredential, PaStore, StoreError, TokenCipher};
use chrono::{DateTime, Utc};

const DATABASE_KEY: [u8; 32] = [0x41; 32];

fn cipher() -> TokenCipher {
    TokenCipher::new(DATABASE_KEY).expect("32-byte key is accepted")
}

fn expires_at(offset_seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + offset_seconds, 0).expect("valid timestamp")
}

fn raw_credential_row(
    store: &PaStore,
    provider: &str,
    account_id: &str,
) -> (Vec<u8>, Option<Vec<u8>>, Option<String>, String) {
    store
        .connection()
        .query_row(
            "SELECT access_token_ciphertext, refresh_token_ciphertext, expires_at, scopes
             FROM oauth_credentials WHERE provider = ?1 AND account_id = ?2",
            [provider, account_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("credential row")
}

#[test]
fn omitted_refresh_token_preserves_prior() {
    let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
    let cipher = cipher();
    let initial_scopes = vec!["Mail.Read".to_owned()];
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "old-access-token",
            Some("old-refresh-token"),
            expires_at(0),
            &initial_scopes,
        )
        .expect("initial insert");
    let before = raw_credential_row(&store, "google", "account-a");

    let updated_scopes = vec![
        " Mail.Read ".to_owned(),
        "Calendars.Read".to_owned(),
        "Mail.Read".to_owned(),
    ];
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "new-access-token",
            None,
            expires_at(60),
            &updated_scopes,
        )
        .expect("refresh omission update");
    let after = raw_credential_row(&store, "google", "account-a");

    assert!(before.0 != after.0);
    assert!(before.1 == after.1);
    assert!(
        !after
            .0
            .windows(b"new-access-token".len())
            .any(|window| { window == b"new-access-token" })
    );
    assert!(
        !after
            .1
            .as_deref()
            .unwrap()
            .windows(b"old-refresh-token".len())
            .any(|window| window == b"old-refresh-token")
    );

    let credential = store
        .load_oauth_credential(&cipher, "google", "account-a")
        .expect("updated credential");
    assert!(credential.access_token() == "new-access-token");
    assert!(credential.refresh_token() == Some("old-refresh-token"));
    assert!(credential.expires_at() == Some(expires_at(60)));
    assert!(credential.scopes() == ["Calendars.Read".to_owned(), "Mail.Read".to_owned()]);

    let access_envelope: agent_voice::pa::EncryptedSecret =
        serde_json::from_slice(&after.0).expect("access envelope");
    let refresh_envelope: agent_voice::pa::EncryptedSecret =
        serde_json::from_slice(after.1.as_deref().expect("refresh envelope"))
            .expect("refresh envelope");
    assert!(
        cipher
            .decrypt(&access_envelope, b"oauth:google:account-a:access")
            .is_ok()
    );
    assert!(
        cipher
            .decrypt(&refresh_envelope, b"oauth:google:account-a:refresh")
            .is_ok()
    );
    assert!(
        cipher
            .decrypt(&access_envelope, b"oauth:google:account-a:refresh")
            .is_err()
    );
    assert!(
        cipher
            .decrypt(&refresh_envelope, b"oauth:google:account-a:access")
            .is_err()
    );
}

#[test]
fn explicit_refresh_token_replaces_prior_and_upserts_one_row() {
    let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
    let cipher = cipher();
    let scopes = vec!["Mail.Read".to_owned()];
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "first-access",
            Some("first-refresh"),
            expires_at(0),
            &scopes,
        )
        .expect("initial insert");
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "second-access",
            Some("second-refresh"),
            expires_at(60),
            &scopes,
        )
        .expect("explicit refresh replacement");

    let count: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM oauth_credentials
             WHERE provider = 'google' AND account_id = 'account-a'",
            [],
            |row| row.get(0),
        )
        .expect("one credential row");
    assert!(count == 1);
    let credential = store
        .load_oauth_credential(&cipher, "google", "account-a")
        .expect("updated credential");
    assert!(credential.access_token() == "second-access");
    assert!(credential.refresh_token() == Some("second-refresh"));
}

#[test]
fn omitted_refresh_token_on_initial_insert_keeps_refresh_null() {
    let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
    let cipher = cipher();
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "access-only",
            None,
            expires_at(0),
            &["Mail.Read".to_owned()],
        )
        .expect("initial access-only insert");

    let row = raw_credential_row(&store, "google", "account-a");
    assert!(row.1.is_none());
    let credential = store
        .load_oauth_credential(&cipher, "google", "account-a")
        .expect("access-only credential");
    assert!(credential.access_token() == "access-only");
    assert!(credential.refresh_token().is_none());
}

#[test]
fn invalid_input_fails_before_writing_and_redacts_values() {
    let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
    let cipher = cipher();
    let valid_scopes = vec!["Mail.Read".to_owned()];
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "stable-access",
            Some("stable-refresh"),
            expires_at(0),
            &valid_scopes,
        )
        .expect("initial insert");
    let before = raw_credential_row(&store, "google", "account-a");

    let invalid_calls = [
        ("", "account-a", "new-access", None, valid_scopes.clone()),
        ("google", "", "new-access", None, valid_scopes.clone()),
        ("google", "account-a", " ", None, valid_scopes.clone()),
        (
            "google",
            "account-a",
            "new-access",
            Some(" "),
            valid_scopes.clone(),
        ),
        ("google", "account-a", "new-access", None, Vec::new()),
        (
            "google",
            "account-a",
            "new-access",
            None,
            vec![" ".to_owned()],
        ),
    ];
    for (provider, account_id, access_token, refresh_token, scopes) in invalid_calls {
        let error = store
            .update_oauth_tokens(
                &cipher,
                provider,
                account_id,
                access_token,
                refresh_token,
                expires_at(60),
                &scopes,
            )
            .expect_err("invalid update must fail");
        let debug = format!("{error:?}");
        let display = error.to_string();
        assert!(!debug.contains("new-access"));
        assert!(!debug.contains("stable-refresh"));
        assert!(!display.contains("new-access"));
        assert!(!display.contains("stable-refresh"));
        assert!(matches!(error, StoreError::InvalidInput { .. }));
        assert!(raw_credential_row(&store, "google", "account-a") == before);
    }
}

#[test]
fn provider_and_account_keys_are_isolated_with_one_row_each() {
    let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
    let cipher = cipher();
    let scopes = vec!["Mail.Read".to_owned()];
    for (provider, account_id, access_token) in [
        ("google", "account-a", "google-a"),
        ("google", "account-b", "google-b"),
        ("microsoft", "account-a", "microsoft-a"),
    ] {
        store
            .update_oauth_tokens(
                &cipher,
                provider,
                account_id,
                access_token,
                None,
                expires_at(0),
                &scopes,
            )
            .expect("isolated insert");
    }
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "google-a-updated",
            None,
            expires_at(60),
            &scopes,
        )
        .expect("isolated upsert");

    let count: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM oauth_credentials", [], |row| {
            row.get(0)
        })
        .expect("credential count");
    assert!(count == 3);
    assert!(
        store
            .load_oauth_credential(&cipher, "google", "account-a")
            .expect("google account a")
            .access_token()
            == "google-a-updated"
    );
    assert!(
        store
            .load_oauth_credential(&cipher, "google", "account-b")
            .expect("google account b")
            .access_token()
            == "google-b"
    );
    assert!(
        store
            .load_oauth_credential(&cipher, "microsoft", "account-a")
            .expect("microsoft account a")
            .access_token()
            == "microsoft-a"
    );
}

#[test]
fn sql_failure_rolls_back_the_whole_update() {
    let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
    let cipher = cipher();
    let scopes = vec!["Mail.Read".to_owned()];
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "stable-access",
            Some("stable-refresh"),
            expires_at(0),
            &scopes,
        )
        .expect("initial insert");
    let before = raw_credential_row(&store, "google", "account-a");
    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER reject_oauth_update
             BEFORE UPDATE ON oauth_credentials
             BEGIN SELECT RAISE(ABORT, 'update rejected'); END;",
        )
        .expect("rejecting trigger");

    let error = store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "new-access",
            Some("new-refresh"),
            expires_at(60),
            &["Calendars.Read".to_owned()],
        )
        .expect_err("trigger must reject update");
    assert!(!error.to_string().contains("new-access"));
    assert!(!error.to_string().contains("new-refresh"));
    assert!(raw_credential_row(&store, "google", "account-a") == before);
}

#[test]
fn credential_debug_redacts_updated_token_values() {
    let store = PaStore::open_in_memory(DATABASE_KEY).expect("open store");
    let cipher = cipher();
    store
        .update_oauth_tokens(
            &cipher,
            "google",
            "account-a",
            "updated-access",
            Some("updated-refresh"),
            expires_at(0),
            &["Mail.Read".to_owned()],
        )
        .expect("insert");
    let credential = store
        .load_oauth_credential(&cipher, "google", "account-a")
        .expect("load");
    let debug = format!("{credential:?}");
    assert!(!debug.contains("updated-access"));
    assert!(!debug.contains("updated-refresh"));
    assert!(debug.contains("<redacted>"));
    assert!(format!("{:?}", OAuthCredential::from(&credential)).contains("<redacted>"));
}
