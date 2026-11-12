use super::*;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(crate) async fn security(State(app): State<App>, request: Request, next: Next) -> Response {
    let rejection = {
        // Production ingress must be the configured trusted socket peer.
        // Bot endpoints additionally allow direct bearer-authenticated local/private HTTP.
        if app.config.production
            && !request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .is_some_and(|peer| trusted_proxy(&app.config, peer.0.ip()))
        {
            Some(
                (
                    StatusCode::FORBIDDEN,
                    "Request must come from the trusted proxy.",
                )
                    .into_response(),
            )
        } else if request.headers().contains_key(header::AUTHORIZATION)
            && !request
                .uri()
                .path()
                .starts_with(&format!("{}api/bot/", app.config.base_url))
        {
            Some(
                super::error(
                    StatusCode::UNAUTHORIZED,
                    "Bearer credentials are accepted only under /api/bot/v1/.",
                )
                .into_response(),
            )
        } else {
            let direct_bot = app.config.direct_bot_http
                && request
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
                Some(
                    (
                        StatusCode::BAD_REQUEST,
                        "HTTPS is required through the trusted reverse proxy.",
                    )
                        .into_response(),
                )
            } else {
                None
            }
        }
    };
    let client_origin = request
        .headers()
        .get(header::ORIGIN)
        .filter(|origin| {
            origin
                .to_str()
                .is_ok_and(|origin| app.config.allows_origin(origin))
        })
        .cloned();
    let browser_api = request
        .uri()
        .path()
        .starts_with(&format!("{}api/", app.config.base_url))
        && !request
            .uri()
            .path()
            .starts_with(&format!("{}api/bot/", app.config.base_url));
    let preflight = browser_api && request.method() == axum::http::Method::OPTIONS;
    let response = match rejection {
        Some(response) => response,
        None if preflight => {
            let headers = request.headers();
            let method_allowed = headers
                .get(header::ACCESS_CONTROL_REQUEST_METHOD)
                .is_some_and(|method| method == "GET" || method == "POST");
            let headers_allowed = headers
                .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
                .is_none_or(|headers| {
                    headers.to_str().is_ok_and(|headers| {
                        headers.split(',').all(|name| {
                            matches!(
                                name.trim().to_ascii_lowercase().as_str(),
                                "content-type" | "x-chat-name-colors"
                            )
                        })
                    })
                });
            if client_origin.is_some() && method_allowed && headers_allowed {
                StatusCode::NO_CONTENT.into_response()
            } else {
                StatusCode::FORBIDDEN.into_response()
            }
        }
        // Reject explicitly untrusted origins before dispatching browser API writes.
        None if browser_api
            && request.method() != axum::http::Method::GET
            && request.method() != axum::http::Method::HEAD
            && request.headers().contains_key(header::ORIGIN)
            && client_origin.is_none() =>
        {
            error(StatusCode::FORBIDDEN, "Untrusted request origin.").into_response()
        }
        None => next.run(request).await,
    };
    let mut response = with_security_headers(&app, response);
    if browser_api {
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Origin"));
        if let Some(origin) = client_origin {
            let headers = response.headers_mut();
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
            if preflight && response.status() == StatusCode::NO_CONTENT {
                let headers = response.headers_mut();
                // Browser-managed permission cache only; every actual request
                // still checks the current origin, session and domain authority.
                headers.insert(
                    header::ACCESS_CONTROL_MAX_AGE,
                    HeaderValue::from_static("600"),
                );
                headers.insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    HeaderValue::from_static("GET, POST"),
                );
                headers.insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    HeaderValue::from_static("Content-Type, X-Chat-Name-Colors"),
                );
            }
        }
    }
    response
}

fn with_security_headers(app: &App, mut response: Response) -> Response {
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
        if app.config.origins.is_empty() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Missing origin configuration.",
            )
            .into_response();
        }
        let connection_origins = app
            .config
            .origins
            .iter()
            .flat_map(|origin| {
                if origin == "*" {
                    vec!["https:".to_owned(), "wss:".to_owned()]
                } else {
                    vec![origin.clone(), origin.replacen("https://", "wss://", 1)]
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let media_origin = app
            .media
            .as_ref()
            .map_or("", |storage| storage.public_origin.as_str());
        let policy = format!(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' http: https: data: blob:; font-src 'self' data:; connect-src 'self' {connection_origins} {media_origin}; media-src 'self' {media_origin}; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; upgrade-insecure-requests"
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
    if app.config.allows_origin(origin) {
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
) -> Result<std::net::IpAddr, &'static str> {
    const INVALID: &str = "The trusted proxy must supply one valid client IP address.";
    let peer = peer.to_canonical();
    if !trusted_proxy(config, peer) {
        return Ok(peer);
    }
    let Some(name) = config.set_real_ip_from.as_deref() else {
        return Ok(peer);
    };
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(peer);
    };
    if values.next().is_some() {
        return Err(INVALID);
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.trim().parse::<std::net::IpAddr>().ok())
        .map(|ip| ip.to_canonical())
        .ok_or(INVALID)
}

// Browsers generally omit Origin on same-origin GET fetches. Fetch Metadata
// supplies that proof without trusting same-site sibling origins.
pub(crate) fn check_read_origin(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::ORIGIN) {
        return check_origin(app, headers);
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

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test assertions.
mod tests {
    use super::*;

    #[test]
    fn api_csp_uses_all_origins_without_a_primary_entry() {
        let mut app = crate::web::tests::login_test_app();
        let config = Arc::make_mut(&mut app.config);
        config.production = true;
        config.origins = vec![
            "https://one.example.com".into(),
            "https://two.example.com".into(),
        ];
        let response = with_security_headers(&app, StatusCode::OK.into_response());
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        for origin in [
            "https://one.example.com",
            "https://two.example.com",
            "wss://one.example.com",
            "wss://two.example.com",
        ] {
            assert!(policy.contains(origin), "{origin}");
        }
        Arc::make_mut(&mut app.config).origins = vec!["*".into()];
        let response = with_security_headers(&app, StatusCode::OK.into_response());
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(policy.contains("connect-src 'self' https: wss:"));
        assert!(!policy.contains("connect-src 'self' *"));
    }
}
