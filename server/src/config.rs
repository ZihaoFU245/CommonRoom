use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bind: String,
    pub origins: Vec<String>,
    pub production: bool,
    pub direct_bot_http: bool,
    pub trust: std::net::IpAddr,
    pub set_real_ip_from: Option<String>,
    pub base_url: String,
    pub max_users: usize,
    pub max_rooms: usize,
    pub max_messages: usize,
    pub database: DatabaseConfig,
    pub media: MediaConfig,
    pub avatar_hosts: Vec<String>,
    pub s3: Option<S3Config>,
}
/// Backend selection is persisted; connection URLs and credentials are not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    pub backend: DatabaseBackend,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postgres_pool: Option<PostgresPoolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_cache: Option<HistoryCacheConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryCacheConfig {
    #[serde(with = "byte_size")]
    pub max_bytes: u64,
    pub max_messages: usize,
}
impl Default for HistoryCacheConfig {
    fn default() -> Self {
        Self {
            max_bytes: 32 * 1024 * 1024,
            max_messages: 10_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PostgresPoolConfig {
    pub min_readers: u32,
    pub max_readers: u32,
    pub max_writers: u32,
    pub idle_timeout_seconds: u64,
}
impl Default for PostgresPoolConfig {
    fn default() -> Self {
        Self {
            min_readers: 2,
            max_readers: 8,
            max_writers: 8,
            idle_timeout_seconds: 600,
        }
    }
}
impl DatabaseConfig {
    fn validate(&self) -> Result<(), String> {
        let pool = self.postgres_pool.clone().unwrap_or_default();
        if pool.max_readers == 0
            || pool.max_readers > 64
            || pool.min_readers > pool.max_readers
            || pool.max_writers == 0
            || pool.max_writers > 32
            || !(1..=86_400).contains(&pool.idle_timeout_seconds)
        {
            return Err("database.postgres_pool requires 0 <= min_readers <= max_readers <= 64, positive max_readers, 1 <= max_writers <= 32 and 1 <= idle_timeout_seconds <= 86400.".into());
        }
        let cache = self.history_cache.clone().unwrap_or_default();
        if !(4096..=1024 * 1024 * 1024).contains(&cache.max_bytes)
            || !(4..=100_000).contains(&cache.max_messages)
        {
            return Err("database.history_cache requires 4k <= max_bytes <= 1g and 4 <= max_messages <= 100000.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    #[default]
    Sqlite,
    Postgres,
}
/// Provider-neutral connection settings. Credentials are environment-only.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct S3Config {
    pub endpoint: String,
    pub public_endpoint: Option<String>,
    pub bucket: String,
    pub region: String,
    pub path_style: bool,
}
impl Default for S3Config {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            public_endpoint: None,
            bucket: String::new(),
            region: "us-east-1".into(),
            path_style: true,
        }
    }
}
/// Byte counts accept binary k/m/g units and retain exact integer precision.
pub(crate) fn parse_byte_size(value: &str) -> Result<u64, String> {
    let (digits, scale) = match value.as_bytes().last().map(u8::to_ascii_lowercase) {
        Some(b'k') => (&value[..value.len() - 1], 1024u64),
        Some(b'm') => (&value[..value.len() - 1], 1024u64.pow(2)),
        Some(b'g') => (&value[..value.len() - 1], 1024u64.pow(3)),
        _ => (value, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err("Use a positive byte count or integer k/m/g units, e.g. 512k, 25m, 1g.".into());
    }
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .filter(|n| *n > 0)
        .ok_or_else(|| "Byte size is zero or too large.".into())
}
pub(crate) fn format_byte_size(value: u64) -> String {
    for (suffix, scale) in [("g", 1024u64.pow(3)), ("m", 1024u64.pow(2)), ("k", 1024)] {
        if value > 0 && value.is_multiple_of(scale) {
            return format!("{}{suffix}", value / scale);
        }
    }
    value.to_string()
}
mod byte_size {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::format_byte_size(*value))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Size {
            Text(String),
            Bytes(u64),
        }
        match Size::deserialize(deserializer)? {
            Size::Text(value) => super::parse_byte_size(&value).map_err(D::Error::custom),
            Size::Bytes(value) => Ok(value),
        }
    }
}
/// Admission and retention policy, independent of the S3 provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
    #[serde(with = "byte_size")]
    pub max_size: u64,
    pub max_age: i64,
    pub pending_ttl: u64,
    pub cleanup_interval: u64,
    pub cleanup_batch: usize,
    #[serde(with = "byte_size")]
    pub user_quota: u64,
    pub pending_limit: u64,
    pub allocations_per_minute: u64,
}
impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_size: 25 * 1024 * 1024,
            max_age: 7 * 86400,
            pending_ttl: 86400,
            cleanup_interval: 86400,
            cleanup_batch: 32,
            user_quota: 1024 * 1024 * 1024,
            pending_limit: 8,
            allocations_per_minute: 30,
        }
    }
}
impl MediaConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.max_size == 0
            || self.max_size > 5_000_000_000
            || (self.max_age != -1 && !(1..=315360000).contains(&self.max_age))
            || !(3600..=604800).contains(&self.pending_ttl)
            || !(1..=86400).contains(&self.cleanup_interval)
            || !(1..=1024).contains(&self.cleanup_batch)
            || self.user_quota < self.max_size
            || self.user_quota > 1_000_000_000_000_000
            || !(1..=1024).contains(&self.pending_limit)
            || !(1..=10000).contains(&self.allocations_per_minute)
        {
            return Err("Invalid media configuration: max_size 1..5000000000 bytes, positive bounded quotas, max_age -1 or 1..315360000 seconds, pending_ttl 3600..604800, cleanup_interval 1..86400, cleanup_batch/pending_limit 1..1024, allocations_per_minute 1..10000 required.".into());
        }
        Ok(())
    }
}
impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:3000".into(),
            production: false,
            direct_bot_http: false,
            trust: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            set_real_ip_from: Some("X-Forwarded-For".into()),
            base_url: "/".into(),
            max_users: 64,
            max_rooms: 64,
            max_messages: 1000,
            database: DatabaseConfig::default(),
            media: MediaConfig::default(),
            avatar_hosts: Vec::new(),
            s3: None,
            origins: vec![
                "http://localhost:5173".into(),
                "http://127.0.0.1:5173".into(),
                "http://localhost:3000".into(),
                "http://127.0.0.1:3000".into(),
            ],
        }
    }
}
impl Config {
    /// Read backend selection without normalizing or writing configuration. The
    /// selected database/folder owner must be acquired before settings are saved.
    pub fn read_database(data: &std::path::Path) -> Result<DatabaseConfig, String> {
        match std::fs::read(data.join("config.json")) {
            Ok(bytes) => serde_json::from_slice::<Self>(&bytes)
                .map_err(|error| error.to_string())
                .and_then(|config| {
                    config.database.validate()?;
                    Ok(config.database)
                })
                .map_err(|error| format!("Invalid data/config.json: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(DatabaseConfig::default())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn load_for_database(
        data: &std::path::Path,
        selected: &DatabaseConfig,
    ) -> Result<Self, String> {
        Self::load_inner(data, Some(selected))
    }

    pub fn display(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|_| "Could not display configuration.".into())
    }
    #[cfg(test)]
    pub fn load(data: &std::path::Path) -> Result<Self, String> {
        Self::load_inner(data, None)
    }

    fn load_inner(
        data: &std::path::Path,
        selected: Option<&DatabaseConfig>,
    ) -> Result<Self, String> {
        let path = data.join("config.json");
        let raw = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        let saved: Option<Self> = if let Some(bytes) = raw.as_ref() {
            Some(
                serde_json::from_slice(bytes)
                    .map_err(|e| format!("Invalid data/config.json: {e}"))?,
            )
        } else {
            None
        };
        let database = saved
            .as_ref()
            .map_or_else(DatabaseConfig::default, |config| config.database.clone());
        if selected.is_some_and(|selected| *selected != database) {
            return Err(
                "Database settings changed during startup; restart to use the new selection."
                    .into(),
            );
        }
        let production =
            !cfg!(debug_assertions) || std::env::var("CHAT_PRODUCTION").as_deref() == Ok("1");
        let production = production || saved.as_ref().is_some_and(|c| c.production);
        let origins = if production {
            let origin = std::env::var("CHAT_ORIGIN")
                .ok()
                .or_else(|| {
                    saved
                        .as_ref()
                        .filter(|c| c.production)
                        .and_then(|c| c.origins.first().cloned())
                })
                .ok_or("First production start requires CHAT_ORIGIN=https://your-host")?;
            let uri: axum::http::Uri = origin.parse().map_err(|_| "Invalid CHAT_ORIGIN")?;
            if uri.scheme_str() != Some("https")
                || uri.authority().is_none()
                || uri.path_and_query().is_some_and(|p| p.as_str() != "/")
                || origin.ends_with('/')
            {
                return Err(
                    "CHAT_ORIGIN must be an HTTPS origin without a trailing slash or path.".into(),
                );
            }
            vec![origin]
        } else {
            saved
                .as_ref()
                .map(|c| c.origins.clone())
                .unwrap_or_else(|| Self::default().origins)
        };
        let config = Self {
            production,
            direct_bot_http: match std::env::var("CHAT_DIRECT_BOT_HTTP") {
                Ok(value) if value == "1" => true,
                Ok(value) if value == "0" => false,
                Ok(_) => return Err("CHAT_DIRECT_BOT_HTTP must be 0 or 1.".into()),
                Err(std::env::VarError::NotPresent) => {
                    saved.as_ref().is_some_and(|c| c.direct_bot_http)
                }
                Err(_) => return Err("Invalid CHAT_DIRECT_BOT_HTTP.".into()),
            },
            origins,
            max_users: saved.as_ref().map_or(64, |c| c.max_users),
            max_rooms: saved.as_ref().map_or(64, |c| c.max_rooms),
            max_messages: saved.as_ref().map_or(1000, |c| c.max_messages),
            database,
            s3: saved.as_ref().and_then(|c| c.s3.clone()),
            avatar_hosts: saved
                .as_ref()
                .map_or_else(Vec::new, |c| c.avatar_hosts.clone()),
            media: saved
                .as_ref()
                .map_or_else(MediaConfig::default, |c| c.media.clone()),
            base_url: normalize_base_url(
                &std::env::var("CHAT_BASE_URL")
                    .ok()
                    .or_else(|| saved.as_ref().map(|c| c.base_url.clone()))
                    .unwrap_or_else(|| "/".into()),
            )?,
            trust: std::env::var("CHAT_TRUST")
                .ok()
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| "CHAT_TRUST must be a single IP address.")
                })
                .transpose()?
                .or_else(|| saved.as_ref().map(|c| c.trust))
                .unwrap_or_else(|| Self::default().trust),
            set_real_ip_from: saved.as_ref().map_or_else(
                || Self::default().set_real_ip_from,
                |c| c.set_real_ip_from.clone(),
            ),
            bind: std::env::var("CHAT_BIND")
                .ok()
                .or_else(|| saved.map(|c| c.bind))
                .unwrap_or_else(|| "127.0.0.1:3000".into()),
        };
        if let Some(name) = &config.set_real_ip_from {
            axum::http::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "set_real_ip_from must be a valid HTTP header name or null.")?;
        }
        config.media.validate()?;
        if config.direct_bot_http && !private_peer(config.trust) {
            return Err("direct_bot_http requires a loopback or private trusted peer.".into());
        }
        if config.avatar_hosts.len() > 32
            || config
                .avatar_hosts
                .iter()
                .any(|host| !valid_avatar_host(host))
        {
            return Err("avatar_hosts must contain at most 32 exact canonical hostnames without schemes, ports, paths or wildcards.".into());
        }
        config.database.validate()?;
        validate_limits(config.max_users, config.max_rooms, config.max_messages)?;
        config
            .bind
            .parse::<std::net::SocketAddr>()
            .map_err(|_| "bind must be an IP address and port, such as 127.0.0.1:3000.")?;
        let normalized = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
        // Preserve operator formatting and mtime when normalization changes no values.
        let unchanged = raw.as_ref().is_some_and(|bytes| {
            serde_json::from_slice::<serde_json::Value>(bytes).ok()
                == serde_json::from_slice::<serde_json::Value>(&normalized).ok()
        });
        if !unchanged {
            persist_config(data, &path, &normalized)?;
        }
        Ok(config)
    }
}
fn persist_config(
    data: &std::path::Path,
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), String> {
    use std::io::Write;
    // A unique, exclusive sibling avoids following a stale temporary symlink.
    let temp = data.join(format!("config.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        #[cfg(unix)]
        std::fs::File::open(data)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| format!("Could not save configuration: {e}"))
}

fn normalize_base_url(value: &str) -> Result<String, String> {
    if value == "/" {
        return Ok(value.into());
    }
    if !value.starts_with('/')
        || value.trim_end_matches('/').split('/').skip(1).any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        || value.ends_with("//")
    {
        return Err(
            "base_url must be a path such as / or /commonroom/ (letters, digits, - and _).".into(),
        );
    }
    Ok(format!("{}/", value.trim_end_matches('/')))
}

fn private_peer(ip: std::net::IpAddr) -> bool {
    match ip.to_canonical() {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    }
}

pub(crate) fn valid_avatar_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.contains('*')
        && url::Host::parse(value).is_ok_and(|host| host.to_string() == value)
}

pub(crate) fn validate_limits(users: usize, rooms: usize, messages: usize) -> Result<(), String> {
    if users == 0 || rooms == 0 || messages == 0 {
        return Err("max_users, max_rooms and max_messages must be positive integers.".into());
    }
    if users > 256 || rooms > 1024 || messages > 1_000_000 {
        return Err("Limits exceed supported ceilings: max_users 256, max_rooms 1024, max_messages 1000000.".into());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test fixtures intentionally assert success.
mod tests {
    use super::*;
    #[test]
    fn postgres_pool_limits_are_bounded_and_rejected_before_config_is_saved() {
        for pool in [
            serde_json::json!({"min_readers":9,"max_readers":8}),
            serde_json::json!({"max_readers":0}),
            serde_json::json!({"max_writers":33}),
            serde_json::json!({"idle_timeout_seconds":0}),
            serde_json::json!({"idle_timeout_seconds":86401}),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.json");
            let bytes = serde_json::to_vec(
                &serde_json::json!({"database":{"backend":"postgres","postgres_pool":pool}}),
            )
            .unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(Config::read_database(directory.path()).is_err());
            assert!(Config::load(directory.path()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let selected: DatabaseConfig = serde_json::from_value(serde_json::json!({"backend":"postgres","postgres_pool":{"min_readers":0,"max_readers":3,"max_writers":2}})).unwrap();
        selected.validate().unwrap();
        assert_eq!(
            serde_json::to_value(selected).unwrap()["postgres_pool"]["max_readers"],
            3
        );
    }
    #[test]
    fn history_cache_limits_validate_before_rewriting_and_round_trip_units() {
        for cache in [
            serde_json::json!({"max_bytes":"1k"}),
            serde_json::json!({"max_bytes":"2g"}),
            serde_json::json!({"max_messages":3}),
            serde_json::json!({"max_messages":100001}),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.json");
            let bytes =
                serde_json::to_vec(&serde_json::json!({"database":{"history_cache":cache}}))
                    .unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(Config::read_database(directory.path()).is_err());
            assert!(Config::load(directory.path()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let selected: DatabaseConfig = serde_json::from_value(
            serde_json::json!({"history_cache":{"max_bytes":"8m","max_messages":2000}}),
        )
        .unwrap();
        selected.validate().unwrap();
        assert_eq!(
            serde_json::to_value(selected).unwrap()["history_cache"]["max_bytes"],
            "8m"
        );
    }
    #[test]
    fn database_selection_defaults_legacy_configs_and_rejects_secrets_without_rewriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert_eq!(
            Config::read_database(dir.path()).unwrap().backend,
            DatabaseBackend::Sqlite
        );
        assert!(!path.exists());
        std::fs::write(&path, r#"{"max_messages":42}"#).unwrap();
        let before = std::fs::read(&path).unwrap();
        let selected = Config::read_database(dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let config = Config::load_for_database(dir.path(), &selected).unwrap();
        assert_eq!(config.max_messages, 42);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["database"], serde_json::json!({"backend":"sqlite"}));
        for value in [
            serde_json::json!({"database":{"backend":"postgre"}}),
            serde_json::json!({"database":{"backend":"postgres","url":"secret-url"}}),
            serde_json::json!({"database":{"backend":"postgres","password":"secret-password"}}),
            serde_json::json!({"s3":{"access_key":"secret-key"}}),
            serde_json::json!({"s3":{"secret_key":"secret-key"}}),
            serde_json::json!({"s3":{"session_token":"secret-token"}}),
        ] {
            let raw = value.to_string();
            std::fs::write(&path, &raw).unwrap();
            assert!(Config::read_database(dir.path()).is_err());
            assert!(Config::load(dir.path()).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
    }
    #[test]
    fn database_selection_change_during_startup_is_rejected_before_saving() {
        let dir = tempfile::tempdir().unwrap();
        let selected = Config::read_database(dir.path()).unwrap();
        let raw = r#"{"database":{"backend":"postgres"}}"#;
        let path = dir.path().join("config.json");
        std::fs::write(&path, raw).unwrap();
        assert!(Config::load_for_database(dir.path(), &selected).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        let selected = Config::read_database(dir.path()).unwrap();
        assert_eq!(selected.backend, DatabaseBackend::Postgres);
        let config = Config::load_for_database(dir.path(), &selected).unwrap();
        let display: serde_json::Value = serde_json::from_str(&config.display().unwrap()).unwrap();
        assert_eq!(
            display["database"],
            serde_json::json!({"backend":"postgres"})
        );
    }
    #[test]
    fn external_avatar_hosts_and_direct_bot_trust_are_validated_before_saving() {
        for host in ["example.com", "www.gravatar.com", "127.0.0.1"] {
            assert!(valid_avatar_host(host));
        }
        for host in [
            "",
            "*.example.com",
            "https://example.com",
            "example.com:443",
            "user@example.com",
            "example.com/path",
            "EXAMPLE.com",
        ] {
            assert!(!valid_avatar_host(host), "{host}");
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for value in [
            serde_json::json!({"avatar_hosts":["*.example.com"]}),
            serde_json::json!({"direct_bot_http":true,"trust":"203.0.113.1"}),
        ] {
            let raw = value.to_string();
            std::fs::write(&path, &raw).unwrap();
            assert!(Config::load(dir.path()).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
    }
    #[test]
    fn unknown_configuration_fields_fail_without_rewriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for raw in [
            r#"{"produciton":true}"#,
            r#"{"media":{"max_szie":123}}"#,
            r#"{"s3":{"buckte":"private"}}"#,
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(
                Config::load(dir.path())
                    .err()
                    .unwrap()
                    .contains("unknown field")
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
    }
    #[test]
    fn normalized_configuration_keeps_operator_formatting_and_file_identity() {
        let dir = tempfile::tempdir().unwrap();
        Config::load(dir.path()).unwrap();
        let path = dir.path().join("config.json");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let raw = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &raw).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        Config::load(dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), raw);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn zero_limits_are_rejected_before_saving() {
        let dir = tempfile::tempdir().unwrap();
        for field in ["max_users", "max_rooms", "max_messages"] {
            std::fs::write(dir.path().join("config.json"), format!("{{\"{field}\":0}}")).unwrap();
            assert!(Config::load(dir.path()).is_err());
        }
    }
    #[test]
    fn existing_configs_default_to_a_local_proxy() {
        let config: Config = serde_json::from_str(
            r#"{"bind":"127.0.0.1:3000","production":false,"origins":["http://localhost:3000"]}"#,
        )
        .unwrap();
        assert_eq!(config.trust.to_string(), "127.0.0.1");
        assert_eq!(config.max_messages, 1000);
        assert_eq!(config.set_real_ip_from.as_deref(), Some("X-Forwarded-For"));
        let partial: Config = serde_json::from_str(r#"{"production":false}"#).unwrap();
        assert_eq!(partial.bind, "127.0.0.1:3000");
        assert!(serde_json::from_str::<Config>(r#"{"trust":"not-an-ip"}"#).is_err());
    }
    #[test]
    fn configuration_moves_with_data_folder() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let original = Config {
            bind: "127.0.0.1:3456".into(),
            origins: vec!["https://chat.example.com".into()],
            production: true,
            direct_bot_http: true,
            trust: "10.0.0.2".parse().unwrap(),
            set_real_ip_from: Some("X-Real-IP".into()),
            base_url: "/commonroom/".into(),
            max_users: 8,
            max_rooms: 12,
            max_messages: 7,
            database: DatabaseConfig {
                backend: DatabaseBackend::Postgres,
                postgres_pool: None,
                history_cache: None,
            },
            media: MediaConfig::default(),
            avatar_hosts: vec!["www.gravatar.com".into()],
            s3: None,
        };
        std::fs::write(
            source.path().join("config.json"),
            serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        std::fs::copy(
            source.path().join("config.json"),
            destination.path().join("config.json"),
        )
        .unwrap();
        let restored = Config::load(destination.path()).unwrap();
        assert!(restored.production);
        assert_eq!(restored.direct_bot_http, original.direct_bot_http);
        assert_eq!(restored.avatar_hosts, original.avatar_hosts);
        assert_eq!(restored.bind, original.bind);
        assert_eq!(restored.origins, original.origins);
        assert_eq!(restored.trust, original.trust);
        assert_eq!(restored.set_real_ip_from, original.set_real_ip_from);
        assert_eq!(restored.base_url, original.base_url);
        assert_eq!(restored.max_users, 8);
        assert_eq!(restored.max_rooms, 12);
        assert_eq!(restored.max_messages, 7);
        assert_eq!(restored.database, original.database);
    }
    #[test]
    fn base_url_is_normalized_and_safe_for_html_and_cookies() {
        assert_eq!(normalize_base_url("/").unwrap(), "/");
        assert_eq!(normalize_base_url("/commonroom").unwrap(), "/commonroom/");
        assert_eq!(normalize_base_url("/apps/chat/").unwrap(), "/apps/chat/");
        for invalid in [
            "",
            "//",
            "//evil",
            "/a//b",
            "/a/../b",
            "/a?x=1",
            "/a\"",
            "https://host/chat",
            "/a; Path=/",
        ] {
            assert!(normalize_base_url(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn invalid_real_ip_header_is_rejected_without_rewriting_config() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["", "X Real IP", "X-Real-IP\r\nInjected"] {
            let raw = serde_json::json!({"set_real_ip_from": name}).to_string();
            let path = dir.path().join("config.json");
            std::fs::write(&path, &raw).unwrap();
            assert!(Config::load(dir.path()).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
    }

    #[test]
    fn real_ip_header_defaults_on_first_start_and_can_be_disabled() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(dir.path())
                .unwrap()
                .set_real_ip_from
                .as_deref(),
            Some("X-Forwarded-For")
        );
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"set_real_ip_from":null}"#).unwrap();
        assert!(Config::load(dir.path()).unwrap().set_real_ip_from.is_none());
        assert!(Config::load(dir.path()).unwrap().set_real_ip_from.is_none());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test assertions.
mod media_config_tests {
    use super::*;
    #[test]
    fn invalid_media_policy_does_not_rewrite_configuration() {
        let dir = tempfile::tempdir().unwrap();
        for media in [
            serde_json::json!({"max_age":0}),
            serde_json::json!({"max_age":-2}),
            serde_json::json!({"max_size":0}),
            serde_json::json!({"pending_ttl":60}),
            serde_json::json!({"cleanup_interval":0}),
            serde_json::json!({"cleanup_batch":0}),
            serde_json::json!({"pending_limit":0}),
            serde_json::json!({"allocations_per_minute":0}),
            serde_json::json!({"user_quota":1}),
        ] {
            let raw = serde_json::json!({"media":media}).to_string();
            let path = dir.path().join("config.json");
            std::fs::write(&path, &raw).unwrap();
            assert!(Config::load(dir.path()).is_err());
            assert_eq!(std::fs::read_to_string(path).unwrap(), raw);
        }
        let valid:Config=serde_json::from_value(serde_json::json!({"media":{"max_age":-1},"s3":{"endpoint":"https://account.r2.cloudflarestorage.com","bucket":"private","region":"auto","path_style":true}})).unwrap();
        valid.media.validate().unwrap();
        let restored: Config = serde_json::from_str(&valid.display().unwrap()).unwrap();
        assert_eq!(restored.s3.unwrap().region, "auto");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod byte_size_tests {
    use super::*;
    #[test]
    fn size_units_round_trip_without_precision_loss_and_config_uses_readable_values() {
        for (value, bytes) in [
            ("1g", 1073741824),
            ("25m", 26214400),
            ("512K", 524288),
            ("513", 513),
        ] {
            assert_eq!(parse_byte_size(value).unwrap(), bytes);
            assert_eq!(parse_byte_size(&format_byte_size(bytes)).unwrap(), bytes);
        }
        for value in [
            "",
            "-1",
            "0",
            "1.5g",
            "1mb",
            "1g\n",
            "18446744073709551615g",
        ] {
            assert!(parse_byte_size(value).is_err(), "{value}");
        }
        let mut config = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(config["media"]["max_size"], "25m");
        assert_eq!(config["media"]["user_quota"], "1g");
        config["media"]["max_size"] = "512k".into();
        config["media"]["user_quota"] = "1G".into();
        let parsed: Config = serde_json::from_value(config).unwrap();
        assert_eq!(parsed.media.max_size, 524288);
        assert_eq!(parsed.media.user_quota, 1073741824);
        parsed.media.validate().unwrap();
    }
}
