//! Private account preferences share the persisted metadata override tree.
use super::*;

pub(crate) const PREFIX: &str = "preferences.";

pub(crate) fn validate(key: &str, value: &str) -> Result<(), EngineError> {
    let valid = match key {
        "theme" => matches!(value, "system" | "light" | "dark" | "custom"),
        "palette.base" | "palette.accent" => {
            value.len() == 7
                && value.starts_with('#')
                && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        }
        "font.chat" => value.parse::<u8>().is_ok_and(|n| (14..=24).contains(&n)),
        "font.ui" => value.parse::<u8>().is_ok_and(|n| (12..=18).contains(&n)),
        "message_bar_opacity" => value.parse::<u8>().is_ok_and(|n| n <= 100),
        "markdown" | "latex" | "name_colors" | "message_bars" | "name_borders" | "allow_html"
        | "external_avatars" | "notifications" => matches!(value, "true" | "false"),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("Invalid user preference.".into())
    }
}

// Reserved, validated preferences have a fixed key set and their own bound. They
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
                _ => "false",
            };
            validate(key, value.as_deref().unwrap_or(sample))?;
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
