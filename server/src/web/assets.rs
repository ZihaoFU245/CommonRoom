use super::*;
use axum::{
    extract::{OriginalUri, State},
    http::header,
    response::{IntoResponse, Response},
};
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub(super) async fn assets(State(app): State<App>, OriginalUri(uri): OriginalUri) -> Response {
    let asset_path = uri
        .path()
        .strip_prefix(app.config.base_url.trim_end_matches('/'))
        .unwrap_or(uri.path());
    let path = if asset_path == "/" {
        "/index.html"
    } else {
        asset_path
    };
    if let Some((_, bytes)) = ASSETS.iter().find(|(name, _)| *name == path) {
        let content_type = if path.ends_with(".html") {
            "text/html; charset=utf-8"
        } else if path.ends_with(".js") {
            "text/javascript; charset=utf-8"
        } else if path.ends_with(".css") {
            "text/css; charset=utf-8"
        } else if path.ends_with(".svg") {
            "image/svg+xml"
        } else {
            "application/octet-stream"
        };
        if path == "/index.html" {
            let html = String::from_utf8_lossy(bytes).replace(
                "<head>",
                &format!("<head>\n    <base href=\"{}\">", app.config.base_url),
            );
            return ([(header::CONTENT_TYPE, content_type)], html).into_response();
        }
        return ([(header::CONTENT_TYPE, content_type)], *bytes).into_response();
    }
    (StatusCode::NOT_FOUND, "Asset not found. In development, open http://localhost:5173. Release: build the frontend before cargo build --release.").into_response()
}
