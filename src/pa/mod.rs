//! Personal-assistant domain contracts.

pub mod admin_config;
pub mod auth;
pub mod availability;
pub mod crypto;
pub mod domain;
pub mod http;
pub mod oauth;
#[allow(dead_code)]
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
