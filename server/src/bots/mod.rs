mod api;
pub(crate) mod auth;
mod control;
pub(crate) mod discovery;
pub(crate) mod error;
pub(crate) mod socket;
pub(crate) use api::contract;
pub mod models;
pub(crate) mod runtime;

use blake2::{Blake2s256, Digest};

pub(crate) fn digest(secret: &str) -> String {
    format!("{:x}", Blake2s256::digest(secret.as_bytes()))
}

pub(crate) fn routes() -> axum::Router<crate::web::App> {
    api::routes()
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
