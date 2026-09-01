//! Personal-assistant domain contracts.

pub mod admin_config;
pub mod admin_store;
pub mod auth;
pub mod availability;
#[allow(dead_code)]
mod backup;
pub mod crypto;
pub mod domain;
pub mod http;
pub mod oauth;
mod oauth_callback;
mod oauth_start;
pub mod providers;
pub mod service;
pub mod store;

#[cfg(test)]
pub mod fakes;

pub use auth::*;
pub use availability::*;
pub use crypto::*;
pub use domain::*;
pub use providers::*;
pub use service::*;
pub use store::*;
