use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registration {
    pub tokens: BTreeMap<String, Credential>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credential {
    pub label: String,
    pub digest: String,
    pub created: u64,
}

pub(crate) fn validate(data: &crate::engine::Data) -> Result<(), String> {
    let mut digests = std::collections::BTreeSet::new();
    for (id, registration) in &data.bots {
        if !data.users.values().any(|account| account.id == *id) || registration.tokens.len() > 8 {
            return Err("Invalid bot registration.".into());
        }
        let mut labels = std::collections::BTreeSet::new();
        for (token_id, token) in &registration.tokens {
            if uuid::Uuid::parse_str(token_id).is_err()
                || token.label.is_empty()
                || token.digest.len() != 64
                || !token.digest.bytes().all(|b| b.is_ascii_hexdigit())
                || !digests.insert(&token.digest)
                || !labels.insert(&token.label)
            {
                return Err("Invalid bot credential metadata.".into());
            }
        }
    }
    Ok(())
}
