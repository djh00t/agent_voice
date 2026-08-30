#![allow(dead_code)]

#[path = "../src/pa/auth.rs"]
pub mod auth;
#[path = "../src/pa/crypto.rs"]
pub mod crypto;
#[path = "../src/pa/domain.rs"]
pub mod domain;
#[path = "../src/pa/store.rs"]
pub mod store;

mod pa {
    pub use crate::{auth, crypto, domain, store};
}

#[path = "../src/pa/backup/source.rs"]
mod backup_source;
