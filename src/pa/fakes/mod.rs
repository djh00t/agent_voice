//! Deterministic controls shared by personal-assistant provider fakes.

pub mod backup;
pub mod calendar;
pub mod control;
pub mod mail;
pub mod triage;

pub use backup::*;
pub use calendar::*;
pub use control::*;
pub use mail::*;
pub use triage::*;

#[cfg(test)]
mod contract_tests;

#[cfg(test)]
mod tests {
    use super::{FakeControl, FakeEncryptedS3Backup};

    #[test]
    fn backup_fake_is_available_from_the_flat_fake_module() {
        let now = "2026-08-29T12:34:56Z".parse().expect("instant");
        let _ = FakeEncryptedS3Backup::new(FakeControl::new(now));
    }
}
