mod api;
pub(crate) mod error;
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
