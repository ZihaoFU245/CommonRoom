//! Diff authorized conversation tails after projection and delivery validation.
use axum::extract::ws::Utf8Bytes;
use blake2::{Blake2s256, Digest};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

#[derive(Default)]
pub(super) struct Tails(HashMap<String, Tail>);

struct Tail {
    revision: u64,
    messages: BTreeMap<u64, [u8; 32]>,
}

impl Tail {
    fn new<'a>(revision: u64, messages: impl IntoIterator<Item = &'a Value>) -> Option<Self> {
        let messages = messages
            .into_iter()
            .map(|message| {
                let sequence = message.get("sequence")?.as_u64()?;
                let hash = Blake2s256::digest(serde_json::to_vec(message).ok()?).into();
                Some((sequence, hash))
            })
            .collect::<Option<_>>()?;
        Some(Self { revision, messages })
    }
}

impl Tails {
    pub(super) fn encode(&mut self, frame: Utf8Bytes) -> Utf8Bytes {
        let Ok(value) = serde_json::from_str::<Value>(frame.as_str()) else {
            return frame;
        };
        match value["kind"].as_str() {
            Some("snapshot") => self.reset(&value),
            Some("conversation") => {
                if let Some(delta) = self.conversation(&value, frame.len()) {
                    return delta.into();
                }
            }
            _ => {}
        }
        frame
    }

    fn reset(&mut self, value: &Value) {
        self.0.clear();
        if let Some(rooms) = value["rooms"].as_array() {
            for room in rooms {
                self.insert(
                    &room["id"],
                    &value["unread"][room["name"].as_str().unwrap_or("")],
                    room["messages"].as_array().into_iter().flatten(),
                );
            }
        }
        let mut direct = HashMap::<&str, Vec<&Value>>::new();
        if let Some(messages) = value["direct"].as_array() {
            for message in messages {
                if let Some(id) = message["private_id"].as_str() {
                    direct.entry(id).or_default().push(message);
                }
            }
        }
        if let Some(access) = value["private_access"].as_object() {
            for (view, access) in access {
                let messages = access["id"].as_str().and_then(|id| direct.get(id));
                self.insert(
                    &access["id"],
                    &value["unread"][view],
                    messages.into_iter().flatten().copied(),
                );
            }
        }
    }

    fn insert<'a>(
        &mut self,
        id: &Value,
        unread: &Value,
        messages: impl IntoIterator<Item = &'a Value>,
    ) {
        if let Some((id, tail)) = id.as_str().zip(
            unread["revision"]
                .as_u64()
                .and_then(|revision| Tail::new(revision, messages)),
        ) {
            self.0.insert(id.to_owned(), tail);
        }
    }

    fn conversation(&mut self, value: &Value, full_bytes: usize) -> Option<String> {
        let id = value["id"].as_str()?;
        let messages = value["messages"].as_array()?;
        let next = Tail::new(value["unread"]["revision"].as_u64()?, messages)?;
        let delta = self.0.get(id).and_then(|previous| {
            if previous.revision >= next.revision {
                return None;
            }
            let upsert: Vec<_> = messages
                .iter()
                .filter(|message| {
                    message["sequence"].as_u64().is_some_and(|sequence| {
                        previous.messages.get(&sequence) != next.messages.get(&sequence)
                    })
                })
                .collect();
            // The ordered sequence list also describes window evictions and deletions.
            let mut delta = json!({
                "kind": "conversation_delta",
                "view": value["view"],
                "id": id,
                "base_revision": previous.revision,
                "sequences": next.messages.keys().collect::<Vec<_>>(),
                "upsert": upsert,
                "unread": value["unread"],
            });
            for key in ["message_display_names", "name_colors"] {
                if let Some(projection) = value.get(key) {
                    delta[key] = projection.clone();
                }
            }
            let encoded = delta.to_string();
            (encoded.len() < full_bytes).then_some(encoded)
        });
        self.0.insert(id.to_owned(), next);
        delta
    }
}
