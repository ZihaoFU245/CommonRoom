use super::*;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(super) async fn security(State(app): State<App>, request: Request, next: Next) -> Response {
    // Only a trusted, TLS-terminating proxy may reach the production listener.
    if app.config.production
        && !request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .is_some_and(|peer| trusted_proxy(&app.config, peer.0.ip()))
    {
        return (
            StatusCode::FORBIDDEN,
            "Request must come from the trusted proxy.",
        )
            .into_response();
    }
    if app.config.production
        && request
            .headers()
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            != Some("https")
    {
        return (
            StatusCode::BAD_REQUEST,
            "HTTPS is required through the trusted reverse proxy.",
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if app.config.production {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
        let Some(origin) = app.config.origins.first() else {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Missing origin configuration.",
            )
            .into_response();
        };
        let websocket_origin = origin.replacen("https://", "wss://", 1);
        let policy = format!(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self' {websocket_origin}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; upgrade-insecure-requests"
        );
        let Ok(policy) = HeaderValue::from_str(&policy) else {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Invalid security policy configuration.",
            )
            .into_response();
        };
        h.insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    response
}
pub(super) fn trusted_proxy(config: &Config, peer: std::net::IpAddr) -> bool {
    peer.to_canonical() == config.trust.to_canonical()
}
pub(super) fn check_origin(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if app.config.origins.iter().any(|o| o == origin) {
        Ok(())
    } else {
        Err(error(StatusCode::FORBIDDEN, "Untrusted request origin."))
    }
}
