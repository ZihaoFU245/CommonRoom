//! Conditional JSON responses after the handler has checked current access.
use super::*;
use axum::http::{HeaderMap, HeaderValue, header};
use blake2::{Blake2s256, Digest};

pub(super) fn etag(body: &[u8]) -> Option<HeaderValue> {
    // A proxy can apply a content encoding without changing the JSON.
    let tag = format!(r#"W/"{:x}""#, Blake2s256::digest(body));
    HeaderValue::try_from(tag).ok()
}

fn matches(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let expected = etag
        .as_bytes()
        .strip_prefix(b"W/")
        .unwrap_or(etag.as_bytes());
    headers.get_all(header::IF_NONE_MATCH).iter().any(|value| {
        let value = value.as_bytes().trim_ascii();
        if value == b"*" {
            return true;
        }
        // A comma inside an opaque tag is part of the tag, not a separator.
        let mut quoted = false;
        let mut start = 0;
        for (index, byte) in value.iter().copied().enumerate() {
            if byte == b'"' {
                quoted = !quoted;
            } else if byte == b',' && !quoted {
                if weak_eq(&value[start..index], expected) {
                    return true;
                }
                start = index + 1;
            }
        }
        weak_eq(&value[start..], expected)
    })
}

fn weak_eq(value: &[u8], expected: &[u8]) -> bool {
    let value = value.trim_ascii();
    value.strip_prefix(b"W/").unwrap_or(value) == expected
}

pub(super) fn response(
    headers: &HeaderMap,
    body: Bytes,
    etag: Option<HeaderValue>,
    vary_colors: bool,
) -> Response {
    let unchanged = etag.as_ref().is_some_and(|etag| matches(headers, etag));
    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, "application/json")], body).into_response()
    };
    if let Some(etag) = etag {
        let headers = response.headers_mut();
        headers.insert(header::ETAG, etag);
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-cache"),
        );
        headers.append(
            header::VARY,
            HeaderValue::from_static(if vary_colors {
                "Cookie, X-Chat-Name-Colors"
            } else {
                "Cookie"
            }),
        );
    }
    response
}

pub(super) fn json(
    headers: &HeaderMap,
    value: &impl serde::Serialize,
    vary_colors: bool,
) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(value).map_err(|error| {
        tracing::error!(%error, "Failed to serialize API response");
        super::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to serialize API response.",
        )
    })?;
    let tag = etag(&body);
    Ok(response(headers, body.into(), tag, vary_colors))
}
