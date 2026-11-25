//! Private account preferences share the persisted metadata override tree.
use super::*;

pub(crate) const PREFIX: &str = "preferences.";

pub(crate) fn validate(key: &str, value: &str) -> Result<(), EngineError> {
    if default_notification_key(key) || scoped_default_id(key).is_some() {
        return if matches!(value, "true" | "false") {
            Ok(())
        } else {
            Err("Invalid user preference.".into())
        };
    }
    if key == "notification.global" || rule_scope_id(key).is_some() {
        return if matches!(value, "mentions" | "off") {
            Ok(())
        } else {
            notification_filter::parse(value).map(|_| ())
        };
    }
    let valid = match key {
        "theme" => matches!(value, "system" | "light" | "dark" | "custom"),
        "palette.base" | "palette.accent" => {
            value.len() == 7
                && value.starts_with('#')
                && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        }
        "font.chat" => value.parse::<u8>().is_ok_and(|n| (14..=24).contains(&n)),
        "font.ui" => value.parse::<u8>().is_ok_and(|n| (12..=18).contains(&n)),
        "message_preview_height" => value
            .parse::<u16>()
            .is_ok_and(|n| (256..=1024).contains(&n)),
        "message_bar_opacity" => value.parse::<u8>().is_ok_and(|n| n <= 100),
        "markdown"
        | "latex"
        | "name_colors"
        | "message_bars"
        | "name_borders"
        | "allow_html"
        | "external_avatars"
        | "notifications"
        | "notifications_muted"
        | "bot_status" => matches!(value, "true" | "false"),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("Invalid user preference.".into())
    }
}

fn default_notification_key(key: &str) -> bool {
    matches!(
        key,
        "notification_default_mentions_me"
            | "notification_default_mentions_all"
            | "notification_default_replies"
            | "notification_default_direct"
            | "notification_exclude_bots"
            | "notification_exclude_bot_mentions"
    )
}

fn rule_scope_id(key: &str) -> Option<&str> {
    conversation_id(key.strip_prefix("notification.")?)
}

fn scoped_default_id(key: &str) -> Option<&str> {
    let (setting, id) = key.split_once('.')?;
    default_notification_key(setting)
        .then(|| conversation_id(id))
        .flatten()
}

fn notification_id(key: &str) -> Option<&str> {
    rule_scope_id(key).or_else(|| scoped_default_id(key))
}

fn conversation_id(id: &str) -> Option<&str> {
    let is_uuid = |value: &str| {
        uuid::Uuid::parse_str(value).is_ok_and(|uuid| uuid.hyphenated().to_string() == value)
    };
    let valid = id.split_once(':').map_or_else(
        || is_uuid(id),
        |(left, right)| is_uuid(left) && is_uuid(right),
    );
    valid.then_some(id)
}

// Reserved preferences have validated keys and their own bounds. They
// cannot consume the existing 32-key allowance for profile/policy metadata.
pub(super) fn metadata_full(entries: &BTreeMap<String, String>, key: &str) -> bool {
    !key.starts_with(PREFIX)
        && !entries.contains_key(key)
        && entries
            .keys()
            .filter(|key| !key.starts_with(PREFIX))
            .count()
            >= 32
}

impl Engine {
    pub(crate) fn preference_sync(&self) -> SyncVersion {
        SyncVersion {
            instance: self.instance.clone(),
            revision: self
                .revision
                .saturating_add(self.presence.revision)
                .saturating_add(self.read_clock),
        }
    }
    pub(crate) fn preferences(&self, user: &str) -> BTreeMap<String, String> {
        self.data
            .users
            .get(user)
            .and_then(|account| account.metadata.get("@global"))
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| {
                let key = key.strip_prefix(PREFIX)?;
                validate(key, value).ok()?;
                Some((key.to_owned(), value.clone()))
            })
            .collect()
    }

    /// Initialization fills only missing keys under the exclusive metadata fence.
    /// Normal writes patch individual keys; null leaves a key unset until login.
    pub(crate) fn prepare_preferences(
        &mut self,
        user: &str,
        values: BTreeMap<String, Option<String>>,
        initialize: bool,
    ) -> Result<Option<metadata_write::PendingMetadata>, EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        for (key, value) in &values {
            let sample = match key.as_str() {
                "theme" => "system",
                "palette.base" | "palette.accent" => "#000000",
                "font.chat" => "16",
                "font.ui" => "14",
                "message_bar_opacity" => "50",
                "message_preview_height" => "256",
                key if key == "notification.global" || rule_scope_id(key).is_some() => "none",
                _ => "false",
            };
            validate(key, value.as_deref().unwrap_or(sample))?;
            if let Some(id) = notification_id(key).filter(|_| value.is_some()) {
                use authorization::{Action, Scope};
                let scope = self
                    .data
                    .rooms
                    .values()
                    .find(|room| room.id == id)
                    .map(|_| Scope::Room(id.to_owned()))
                    .or_else(|| {
                        self.data
                            .private
                            .iter()
                            .find(|(key, chat)| {
                                chat.id == id && helpers::private_peer(key, user).is_some()
                            })
                            .map(|_| Scope::Private(id.to_owned()))
                    });
                if !scope.is_some_and(|scope| self.allows(Some(user), &scope, Action::Read)) {
                    return Err(EngineError::Forbidden("Conversation unavailable.".into()));
                }
            }
        }
        let before = MetadataChange::accounts(&self.data, [user.to_owned()], false);
        let result = (|| {
            let account = self
                .data
                .users
                .get_mut(user)
                .ok_or("Account unavailable.")?;
            let root = account.metadata.entry("@global".into()).or_default();
            for (key, value) in values {
                let key = format!("{PREFIX}{key}");
                if initialize && root.contains_key(&key) {
                    continue;
                }
                if let Some(value) = value {
                    if key.strip_prefix(PREFIX).and_then(notification_id).is_some()
                        && !root.contains_key(&key)
                        && root
                            .keys()
                            .filter_map(|key| key.strip_prefix(PREFIX).and_then(notification_id))
                            .count()
                            >= 2048
                    {
                        return Err("Too many conversation notification settings.".into());
                    }
                    root.insert(key, value);
                } else if !initialize {
                    root.remove(&key);
                }
            }
            account.metadata.retain(|_, entries| !entries.is_empty());
            if before.unchanged(&self.data) {
                return Ok(None);
            }
            self.capture_metadata(&before, vec![]).map(Some)
        })();
        before.restore(&mut self.data);
        result
    }
}
