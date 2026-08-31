#[path = "../src/pa/backup/envelope.rs"]
pub mod envelope;

pub mod pa {
    pub mod backup {
        pub use crate::envelope;
    }
}

#[path = "../src/pa/backup/metadata.rs"]
mod metadata;
