use super::*;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(crate) async fn security(State(app): State<App>, request: Request, next: Next) -> Response {
    // Production ingress must be the configured trusted socket peer.
    // Bot endpoints additionally allow direct bearer-authenticated local/private HTTP.
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
    let direct_bot = request
        .uri()
        .path()
        .starts_with(&format!("{}api/bot/v1/", app.config.base_url))
        && super::auth::bearer(request.headers()).is_some()
        && !request.headers().contains_key("x-forwarded-proto");
    if app.config.production
        && !direct_bot
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
        let media_origin = app
            .media
            .as_ref()
            .map_or("", |storage| storage.public_origin.as_str());
        let policy = format!(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' http: https: data: blob:; font-src 'self' data:; connect-src 'self' {websocket_origin} {media_origin}; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; upgrade-insecure-requests"
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
pub(crate) fn trusted_proxy(config: &Config, peer: std::net::IpAddr) -> bool {
    peer.to_canonical() == config.trust.to_canonical()
}
pub(crate) fn check_origin(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
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

// The trusted proxy must overwrite this header with one verified client address.
// Never select an address from an unverified forwarding chain.
pub(crate) fn client_ip(
    config: &Config,
    peer: std::net::IpAddr,
    headers: &HeaderMap,
) -> std::net::IpAddr {
    let peer = peer.to_canonical();
    if !trusted_proxy(config, peer) {
        return peer;
    }
    let Some(name) = config.set_real_ip_from.as_deref() else {
        return peer;
    };
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return peer;
    };
    if values.next().is_some() {
        return peer;
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.trim().parse::<std::net::IpAddr>().ok())
        .map_or(peer, |ip| ip.to_canonical())
}

pub(crate) fn check_origin_or_bot(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::AUTHORIZATION) {
        super::auth::bearer(headers)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Bot bearer credential required."))
            .map(|_| ())
    } else {
        check_origin(app, headers)
    }
}

// Browsers generally omit Origin on same-origin GET fetches. Fetch Metadata
// supplies that proof without trusting same-site sibling origins.
pub(crate) fn check_read_origin_or_bot(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::AUTHORIZATION) || headers.contains_key(header::ORIGIN) {
        return check_origin_or_bot(app, headers);
    }
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        == Some("same-origin")
    {
        return Ok(());
    }
    Err(error(StatusCode::FORBIDDEN, "Untrusted request origin."))
}
