//! PostgreSQL indexes only fields used by SQL. Keep arbitrary message text and
//! account metadata in the original JSON text, including escaped NUL characters
//! that PostgreSQL JSONB cannot represent.
use super::{Error, Result};

pub(crate) fn message_projection(raw: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| Error::Conversion("Invalid stored message JSON"))?;
    let reply = value
        .get("reply")
        .filter(|reply| reply.is_object())
        .map(|reply| serde_json::json!({"id":reply.get("id"),"from":reply.get("from")}));
    serde_json::to_string(&serde_json::json!({
        "id":value.get("id"),"from":value.get("from"),"to":value.get("to"),
        "time":value.get("time"),"reply":reply,"mentions":value.get("mentions").cloned().unwrap_or_else(||serde_json::json!([]))
    })).map_err(|_| Error::Conversion("Cannot serialize message projection"))
}

pub(crate) fn entity_projection(raw: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| Error::Conversion("Invalid stored entity JSON"))?;
    let avatar = value
        .pointer("/metadata/@global/avatar")
        .and_then(serde_json::Value::as_str)
        .filter(|avatar| !avatar.contains('\0'));
    serde_json::to_string(&serde_json::json!({"id":value.get("id"),"expires":value.get("expires"),"metadata":{"@global":{"avatar":avatar}}}))
        .map_err(|_| Error::Conversion("Cannot serialize entity projection"))
}
