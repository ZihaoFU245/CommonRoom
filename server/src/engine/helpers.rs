use super::{EngineError, models::*};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use rand_core::OsRng;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= 32 && name.chars().all(name_character)
}
pub(super) fn name_character(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '-' | '_')
        || (!c.is_ascii() && !c.is_whitespace() && !c.is_control())
}

pub fn hash_password(password: &str) -> Result<String, EngineError> {
    if password.chars().count() < 3 || password.len() > crate::limits::MAX_PASSWORD_BYTES {
        return Err("Passwords must contain at least 3 characters and at most 128 bytes.".into());
    }
    Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|h| h.to_string())
        .map_err(EngineError::unavailable)
}
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

pub(super) fn parse_age(value: &str) -> Result<u64, EngineError> {
    let unit = value
        .chars()
        .last()
        .ok_or("Age must use s, m, h, d or w, e.g. 7d.")?;
    let digits = value.strip_suffix(unit).ok_or("Invalid duration.")?;
    let multiplier = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86400,
        'w' => 604800,
        _ => return Err("Age must use s, m, h, d or w, e.g. 7d.".into()),
    };
    digits
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .and_then(|n| n.checked_mul(multiplier))
        .ok_or_else(|| "Age must be a positive duration, e.g. 7d.".into())
}
pub(super) fn require_len(parts: &[&str], len: usize, usage: &str) -> Result<(), EngineError> {
    if parts.len() == len {
        Ok(())
    } else {
        Err(EngineError::Invalid(format!("Usage: {usage}")))
    }
}
/// Match the browser's blank-content check without modifying Unicode text.
pub(super) fn has_message_content(text: &str) -> bool {
    text.chars().any(|ch| {
        !ch.is_whitespace()
            && !ch.is_control()
            // Unicode Default_Ignorable_Code_Point, plus blank braille and the
            // object placeholder copied from rich text (U+FFFC).
            && !matches!(ch,
                '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{115f}'..='\u{1160}'
                | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}'
                | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}' | '\u{2800}' | '\u{3164}'
                | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{ffa0}'
                | '\u{fff0}'..='\u{fff8}' | '\u{fffc}'
                | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0000}'..='\u{e0fff}')
    })
}

pub(super) fn new_message(from: &str, to: Option<&str>, text: &str) -> Message {
    Message {
        id: uuid::Uuid::new_v4().to_string(),
        from: from.into(),
        author_id: String::new(),
        private_id: String::new(),
        to: to.map(String::from),
        text: text.into(),
        markdown: false,
        edited: false,
        time: now(),
        sequence: 0,
        reactions: BTreeMap::new(),
        reply: None,
        mentions: mentioned_names(text),
    }
}
pub(super) fn mentioned_names(text: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for (index, ch) in text.char_indices() {
        if ch != '@'
            || text[..index]
                .chars()
                .next_back()
                .is_some_and(|c| name_character(c) || c == '@')
        {
            continue;
        }
        let name: String = text[index + 1..]
            .chars()
            .take_while(|c| name_character(*c))
            .collect();
        if valid_name(&name) {
            result.insert(name);
        }
    }
    result
}
pub(super) fn private_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}:{b}")
    } else {
        format!("{b}:{a}")
    }
}
pub(super) fn private_peer<'a>(key: &'a str, user: &str) -> Option<&'a str> {
    let (a, b) = key.split_once(':')?;
    if a == user {
        Some(b)
    } else if b == user {
        Some(a)
    } else {
        None
    }
}

pub(crate) fn session_digest(token: &str) -> String {
    crate::bots::digest(&format!("session:{token}"))
}
