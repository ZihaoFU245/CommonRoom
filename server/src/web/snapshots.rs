//! Share baselines by metadata, visible conversation revisions and viewer cursors.
use super::*;
use crate::engine::EngineError;
use axum::extract::ws::Utf8Bytes;
use std::collections::HashMap;
use std::collections::VecDeque;
use tokio::sync::OnceCell;

type Key = (
    String,
    bool,
    crate::engine::SnapshotStamp,
    u64,
    bool,
    Option<String>,
    String,
);
#[derive(Clone)]
pub(super) struct Baseline {
    pub(super) frame: Utf8Bytes,
    pub(super) etag: Option<axum::http::HeaderValue>,
    complete: bool,
    resume: Option<String>,
}
type Value = Arc<OnceCell<Result<Baseline, SnapshotError>>>;
#[derive(Default)]
pub struct SnapshotCache {
    limits: crate::config::SnapshotCacheConfig,
    entries: HashMap<Key, (Value, usize)>,
    order: VecDeque<Key>,
    bytes: usize,
    hits: u64,
    misses: u64,
    coalesced: u64,
    builds: u64,
    failures: u64,
    incomplete: u64,
    evictions: u64,
}
impl SnapshotCache {
    pub(crate) fn new(limits: crate::config::SnapshotCacheConfig) -> Self {
        Self {
            limits,
            ..Default::default()
        }
    }

    pub(crate) fn stats(&self) -> serde_json::Value {
        serde_json::json!({"hits":self.hits,"misses":self.misses,"coalesced":self.coalesced,
            "builds":self.builds,"failures":self.failures,"incomplete":self.incomplete,"evictions":self.evictions,
            "entries":self.entries.len(),"bytes":self.bytes,"entry_limit":self.limits.max_entries,"byte_limit":self.limits.max_bytes})
    }
    fn claim(&mut self, key: &Key) -> Value {
        if let Some((value, _)) = self.entries.get(key) {
            self.hits += 1;
            if value.get().is_none() {
                self.coalesced += 1;
            }
            self.order.retain(|entry| entry != key);
            self.order.push_back(key.clone());
            return value.clone();
        }
        self.misses += 1;
        // Retain older browser checkpoints within the existing cache budget.
        // Bot snapshots have no resume protocol.
        self.order.retain(|old| {
            let keep = old.0 != key.0
                || old.1 != key.1
                || if key.1 {
                    old.4 != key.4
                } else {
                    old.2.metadata == key.2.metadata
                };
            if !keep && let Some((_, size)) = self.entries.remove(old) {
                self.bytes = self.bytes.saturating_sub(size);
            }
            keep
        });
        let value = Arc::new(OnceCell::new());
        self.entries.insert(key.clone(), (value.clone(), 0));
        self.order.push_back(key.clone());
        self.trim();
        value
    }
    fn resume_stamp(
        &self,
        key: &Key,
        token: &str,
    ) -> Option<(crate::engine::SnapshotStamp, Baseline)> {
        self.entries.iter().find_map(|(previous, (entry, _))| {
            if previous.0 != key.0
                || previous.1
                || previous.2.metadata != key.2.metadata
                || previous.4 != key.4
                || previous.6 != key.6
            {
                return None;
            }
            let baseline = entry.get()?.as_ref().ok()?;
            (baseline.resume.as_deref() == Some(token))
                .then(|| (previous.2.clone(), baseline.clone()))
        })
    }
    fn finish(&mut self, key: &Key, value: &Value, size: usize) {
        if let Some((current, accounted)) = self.entries.get_mut(key)
            && Arc::ptr_eq(current, value)
            && *accounted == 0
        {
            *accounted = size;
            self.bytes += size;
        }
        self.trim();
    }
    fn trim(&mut self) {
        while u64::try_from(self.bytes).unwrap_or(u64::MAX) > self.limits.max_bytes
            || self.entries.len() > self.limits.max_entries
        {
            if let Some(key) = self.order.pop_front() {
                if let Some((_, size)) = self.entries.remove(&key) {
                    self.bytes -= size;
                    self.evictions += 1;
                }
            } else {
                break;
            }
        }
    }
}
#[derive(Clone, Debug)]
enum SnapshotError {
    Engine(EngineError),
    Bot(crate::bots::error::Error),
}
impl From<EngineError> for SnapshotError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}
impl From<crate::bots::error::Error> for SnapshotError {
    fn from(error: crate::bots::error::Error) -> Self {
        Self::Bot(error)
    }
}
impl From<SnapshotError> for EngineError {
    fn from(error: SnapshotError) -> Self {
        match error {
            SnapshotError::Engine(error) => error,
            SnapshotError::Bot(error) => error.into(),
        }
    }
}
impl From<SnapshotError> for crate::bots::error::Error {
    fn from(error: SnapshotError) -> Self {
        match error {
            SnapshotError::Engine(error) => error.into(),
            SnapshotError::Bot(error) => error,
        }
    }
}
#[derive(Clone)]
enum Viewer {
    Browser {
        headers: axum::http::HeaderMap,
        bot: bool,
        view: Option<String>,
    },
    Bot(crate::bots::auth::Context),
}
enum Plan {
    Browser(Box<crate::engine::snapshot::SnapshotPlan>),
    Bot(crate::engine::snapshot::BotSnapshotPlan),
}
impl Viewer {
    fn capture(
        &self,
        engine: &Engine,
        config: &Config,
    ) -> Result<(Key, Arc<crate::engine::HistoryStore>, String), SnapshotError> {
        let (user, profile, bot, name_colors) = match self {
            Self::Browser { headers, bot, .. } => {
                let user = super::auth::identity(engine, config, headers)
                    .ok_or_else(|| EngineError::Forbidden("Credential revoked.".into()))?;
                (user.clone(), user, *bot, super::colors::enabled(headers))
            }
            Self::Bot(context) => {
                let user = context.user(engine)?;
                (
                    user,
                    format!("{}:{}", context.account, context.credential),
                    true,
                    false,
                )
            }
        };
        let presence = if !bot
            && engine.allows(
                Some(&user),
                &crate::engine::authorization::Scope::Server,
                crate::engine::authorization::Action::Directory,
            ) {
            engine.presence_revision()
        } else {
            0
        };
        Ok((
            (
                profile,
                bot,
                engine.snapshot_stamp(&user, bot)?,
                presence,
                name_colors,
                match self {
                    Self::Browser { view, .. } => view.clone(),
                    Self::Bot(_) => None,
                },
                engine
                    .active_account_id(&user)
                    .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
                    .to_owned(),
            ),
            engine.history_store().clone(),
            user,
        ))
    }
    fn plan(
        &self,
        engine: &Engine,
        config: &Config,
        expected: &Key,
    ) -> Result<Plan, SnapshotError> {
        let (actual, _, user) = self.capture(engine, config)?;
        if actual != *expected {
            return Err(EngineError::Conflict("Snapshot changed; retry shortly.".into()).into());
        }
        match self {
            Self::Browser { bot, .. } => {
                if *bot {
                    Ok(Plan::Bot(engine.bot_snapshot_plan(&user)?))
                } else {
                    engine
                        .snapshot_plan_for(&user, expected.4)
                        .map(|mut plan| {
                            if let Some(view) = &expected.5 {
                                plan.prioritize(view);
                            }
                            Plan::Browser(Box::new(plan))
                        })
                        .ok_or_else(|| {
                            SnapshotError::Engine(EngineError::Unavailable(
                                "Snapshot unavailable.".into(),
                            ))
                        })
                }
            }
            Self::Bot(context) => Ok(Plan::Bot(context.snapshot_plan(engine)?)),
        }
    }
}
impl Plan {
    async fn finish(self) -> Result<Baseline, EngineError> {
        match self {
            Self::Browser(plan) => {
                let (mut snapshot, complete) = plan.finish_async().await?;
                tokio::task::spawn_blocking(move || {
                    let resume = complete.then(|| uuid::Uuid::new_v4().to_string());
                    snapshot.resume.clone_from(&resume);
                    serde_json::to_string(&snapshot).map(|frame| {
                        let etag = if complete {
                            revalidation::etag(frame.as_bytes())
                        } else {
                            None
                        };
                        Baseline {
                            frame: frame.into(),
                            etag,
                            complete,
                            resume,
                        }
                    })
                })
                .await
                .map_err(EngineError::unavailable)?
                .map_err(EngineError::from)
            }
            Self::Bot(plan) => {
                let snapshot = plan.finish_async().await?;
                let complete = snapshot
                    .conversations
                    .iter()
                    .all(|conversation| conversation.unread.is_some());
                tokio::task::spawn_blocking(move || {
                    serde_json::to_string(&snapshot).map(|frame| Baseline {
                        frame: frame.into(),
                        etag: None,
                        complete,
                        resume: None,
                    })
                })
                .await
                .map_err(EngineError::unavailable)?
                .map_err(EngineError::from)
            }
        }
    }
}

pub(crate) async fn value_async(
    app: &App,
    headers: &axum::http::HeaderMap,
    bot: bool,
) -> Result<Utf8Bytes, EngineError> {
    browser_baseline(app, headers, bot, None)
        .await
        .map(|baseline| baseline.frame)
}
pub(super) async fn browser_baseline(
    app: &App,
    headers: &axum::http::HeaderMap,
    bot: bool,
    view: Option<String>,
) -> Result<Baseline, EngineError> {
    value_for(
        app,
        Viewer::Browser {
            headers: headers.clone(),
            bot,
            view,
        },
    )
    .await
    .map_err(Into::into)
}
/// Subscribe before this capture so later commits also reach the live stream.
pub(super) async fn resume_browser(
    app: &App,
    headers: &axum::http::HeaderMap,
    token: String,
    interests: &mut super::transport::Interests,
    tails: Option<&mut super::deltas::Tails>,
) -> Result<Option<Vec<Utf8Bytes>>, EngineError> {
    let viewer = Viewer::Browser {
        headers: headers.clone(),
        bot: false,
        view: None,
    };
    let (current, history, user) = app
        .inspect_engine({
            let viewer = viewer.clone();
            move |engine, config| viewer.capture(engine, config)
        })
        .await
        .map_err(EngineError::from)?;
    history.ensure_available_async().await?;
    let previous = app
        .snapshots
        .lock()
        .map_err(|_| EngineError::Unavailable("Snapshot cache unavailable.".into()))?
        .resume_stamp(&current, &token);
    let Some((previous, baseline)) = previous else {
        return Ok(None);
    };

    // Render authoritative tails first. Delta-capable clients reconstruct them
    // from the exact checkpoint and can safely overlap the live subscription.
    let mut changes = vec![Change::Presence];
    if previous.read != current.2.read {
        changes.push(Change::Read(user.clone()));
    }
    if previous.history.len() != current.2.history.len() {
        return Ok(None);
    }
    let mut changed = 0;
    for ((old_id, old_revision), (id, revision)) in previous.history.iter().zip(&current.2.history)
    {
        if old_id != id || old_revision > revision {
            return Ok(None);
        }
        if old_revision != revision {
            changed += 1;
            if changed > 4 {
                return Ok(None);
            }
            changes.push(Change::Conversation(id.clone()));
        }
    }
    let (resolved, mut frames) =
        super::updates::browser_with_interests_async(app, headers, false, &changes, interests)
            .await?;
    let same = app
        .inspect_engine(move |engine, config| -> Result<_, EngineError> {
            let (latest, _, _) = viewer.capture(engine, config)?;
            Ok(user == resolved
                && latest.0 == current.0
                && latest.2 == current.2
                && latest.6 == current.6)
        })
        .await?;
    if !same {
        return Ok(None);
    }
    let candidate = tails.as_ref().map(|_| {
        let mut candidate = super::deltas::Tails::default();
        candidate.encode(baseline.frame);
        for frame in &mut frames {
            *frame = candidate.encode(std::mem::take(frame));
        }
        candidate
    });
    if frames.iter().map(|frame| frame.len()).sum::<usize>() > 512 * 1024 {
        return Ok(None);
    }
    if let Some((tails, candidate)) = tails.zip(candidate) {
        *tails = candidate;
    }
    frames.push(Utf8Bytes::from_static(r#"{"kind":"resumed"}"#));
    Ok(Some(frames))
}
pub(crate) async fn bot_value_async(
    app: &App,
    context: &crate::bots::auth::Context,
) -> Result<Utf8Bytes, crate::bots::error::Error> {
    value_for(app, Viewer::Bot(context.clone()))
        .await
        .map(|baseline| baseline.frame)
        .map_err(Into::into)
}
// Synchronous domain/test callers bridge once for the complete snapshot, rather
// than once for every unread/page/health operation. Transports await directly.
#[cfg(test)]
pub(crate) fn value(
    app: &App,
    headers: &axum::http::HeaderMap,
    bot: bool,
) -> Result<Utf8Bytes, EngineError> {
    crate::engine::database::block_on_storage(value_async(app, headers, bot))?
}
#[cfg(test)]
pub(crate) fn bot_value(
    app: &App,
    context: &crate::bots::auth::Context,
) -> Result<Utf8Bytes, crate::bots::error::Error> {
    crate::engine::database::block_on_storage(bot_value_async(app, context))
        .map_err(crate::bots::error::Error::storage)?
}
async fn value_for(app: &App, viewer: Viewer) -> Result<Baseline, SnapshotError> {
    for _ in 0..3 {
        if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Unavailable("Server is shutting down.".into()).into());
        }
        let (initial, history, _) = app
            .inspect_engine({
                let viewer = viewer.clone();
                move |engine, config| viewer.capture(engine, config)
            })
            .await?;
        let result = cached(app, initial.clone(), || async {
            let plan = app
                .inspect_engine({
                    let viewer = viewer.clone();
                    let expected = initial.clone();
                    move |engine, config| viewer.plan(engine, config, &expected)
                })
                .await?;
            plan.finish().await.map_err(Into::into)
        })
        .await;
        // Recheck health even on a complete cache hit, then validate authority
        // and the current revision after all native database awaits have ended.
        history.ensure_available_async().await?;
        let unchanged = app
            .inspect_engine({
                let viewer = viewer.clone();
                let expected = initial.clone();
                move |engine, config| -> Result<_, SnapshotError> {
                    Ok(viewer.capture(engine, config)?.0 == expected)
                }
            })
            .await?;
        if unchanged {
            return result;
        }
    }
    Err(EngineError::Conflict("Snapshot changed; retry shortly.".into()).into())
}
async fn cached<F: std::future::Future<Output = Result<Baseline, SnapshotError>>>(
    app: &App,
    key: Key,
    build: impl FnOnce() -> F,
) -> Result<Baseline, SnapshotError> {
    let value = app
        .snapshots
        .lock()
        .map_err(|_| EngineError::Unavailable("Snapshot cache unavailable.".into()))?
        .claim(&key);
    let mut built = false;
    let result = value
        .get_or_init(|| async {
            built = true;
            build().await
        })
        .await
        .clone();
    let mut cache = app
        .snapshots
        .lock()
        .map_err(|_| EngineError::Unavailable("Snapshot cache unavailable.".into()))?;
    if built {
        cache.builds += 1;
        if result.is_err() {
            cache.failures += 1;
        }
        if result.as_ref().is_ok_and(|baseline| !baseline.complete) {
            cache.incomplete += 1;
        }
    }
    if let Ok(baseline) = &result
        && baseline.complete
    {
        cache.finish(&key, &value, baseline.frame.len());
    } else if cache
        .entries
        .get(&key)
        .is_some_and(|(current, _)| Arc::ptr_eq(current, &value))
    {
        cache.entries.remove(&key);
        cache.order.retain(|entry| entry != &key);
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn resume_validates_current_viewer_projection_and_history() {
        use axum::http::{HeaderMap, HeaderValue, header};
        let app = crate::web::tests::login_test_app();
        {
            let mut engine = app.engine_api().unwrap();
            let hash = engine.password_hash("alice").unwrap().to_owned();
            engine
                .login("resume-session".into(), "alice", &hash, None)
                .unwrap();
            engine.execute(None, None, "/room new team").unwrap();
            engine.execute(None, None, "/room add alice #team").unwrap();
            engine
                .execute(Some("alice"), Some("team"), "First")
                .unwrap();
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("chat_session=resume-session"),
        );
        let baseline = browser_baseline(&app, &headers, false, Some("team".into()))
            .await
            .unwrap();
        let token = baseline.resume.unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(&baseline.frame).unwrap();
        assert_eq!(snapshot["resume"], token);
        app.update_presence(None, Some("alice")).unwrap();
        let mut interests = super::super::transport::Interests::default();
        let resumed = resume_browser(&app, &headers, token.clone(), &mut interests, None)
            .await
            .unwrap()
            .unwrap();
        let frames: Vec<serde_json::Value> = resumed
            .iter()
            .map(|frame| serde_json::from_str(frame).unwrap())
            .collect();
        assert_eq!(frames.last().unwrap()["kind"], "resumed");
        assert_eq!(frames[0]["online"], json!(["alice"]));
        headers.insert("x-chat-name-colors", HeaderValue::from_static("1"));
        assert!(
            resume_browser(&app, &headers, token.clone(), &mut interests, None)
                .await
                .unwrap()
                .is_none()
        );
        headers.remove("x-chat-name-colors");
        app.engine_api()
            .unwrap()
            .execute(Some("alice"), Some("team"), "Second")
            .unwrap();
        let resumed = resume_browser(&app, &headers, token.clone(), &mut interests, None)
            .await
            .unwrap()
            .unwrap();
        let frames: Vec<serde_json::Value> = resumed
            .iter()
            .map(|frame| serde_json::from_str(frame).unwrap())
            .collect();
        assert_eq!(frames.last().unwrap()["kind"], "resumed");
        assert!(frames.iter().any(
            |frame| frame["kind"] == "conversation" && frame["messages"][1]["text"] == "Second"
        ));
        app.engine_api().unwrap().logout("resume-session").unwrap();
        assert!(matches!(
            resume_browser(&app, &headers, token, &mut interests, None).await,
            Err(EngineError::Forbidden(_))
        ));
    }

    #[test]
    fn configured_snapshot_limits_evict_entries_and_oversized_frames() {
        let app = crate::web::tests::login_test_app();
        let stamp = app
            .engine_read()
            .unwrap()
            .snapshot_stamp("alice", false)
            .unwrap();
        let mut cache = SnapshotCache::new(crate::config::SnapshotCacheConfig {
            max_bytes: 4096,
            max_entries: 1,
        });
        let first = (
            "alice".into(),
            false,
            stamp.clone(),
            0,
            false,
            None,
            "one".into(),
        );
        let second = ("bob".into(), false, stamp, 0, false, None, "two".into());
        let value = cache.claim(&first);
        cache.finish(&first, &value, 1024);
        assert_eq!(cache.stats()["bytes"], 1024);
        let value = cache.claim(&second);
        assert!(!cache.entries.contains_key(&first));
        assert_eq!(cache.stats()["evictions"], 1);
        cache.finish(&second, &value, 4097);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.stats()["bytes"], 0);
        assert_eq!(cache.stats()["byte_limit"], 4096);
        assert_eq!(cache.stats()["entry_limit"], 1);
    }
    #[tokio::test]
    async fn cancelling_a_cache_builder_releases_waiters_and_allows_a_new_fill() {
        let app = crate::web::tests::login_test_app();
        let key = {
            let engine = app.engine_read().unwrap();
            (
                "cancelled-builder".into(),
                false,
                engine.snapshot_stamp("alice", false).unwrap(),
                0,
                false,
                None,
                "alice-account".into(),
            )
        };
        let (started, waiting) = tokio::sync::oneshot::channel();
        let builder = tokio::spawn({
            let app = app.clone();
            let key = key.clone();
            async move {
                cached(&app, key, || async {
                    started.send(()).unwrap();
                    std::future::pending::<Result<Baseline, SnapshotError>>().await
                })
                .await
            }
        });
        waiting.await.unwrap();
        let (following, entered) = tokio::sync::oneshot::channel();
        let follower = tokio::spawn({
            let app = app.clone();
            let key = key.clone();
            async move {
                following.send(()).unwrap();
                cached(&app, key, || async {
                    Ok(Baseline {
                        frame: "complete".into(),
                        etag: None,
                        complete: true,
                        resume: None,
                    })
                })
                .await
            }
        });
        entered.await.unwrap();
        assert!(!follower.is_finished());
        builder.abort();
        assert!(matches!(builder.await, Err(error) if error.is_cancelled()));
        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), follower)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame.frame.as_str(), "complete");
        let reused = cached(&app, key, || async {
            Err(EngineError::Internal("Completed cache entry was rebuilt.".into()).into())
        })
        .await
        .unwrap();
        assert_eq!(
            frame.frame.as_str().as_ptr(),
            reused.frame.as_str().as_ptr()
        );
        let metrics = app.snapshots.lock().unwrap().stats();
        assert_eq!(metrics["misses"], 1);
        assert_eq!(metrics["hits"], 2);
        assert_eq!(metrics["coalesced"], 1);
        assert_eq!(metrics["builds"], 1); // Only completed builds; cancellation does not invent a result.
        assert_eq!(metrics["failures"], 0);
        assert_eq!(metrics["entries"], 1);
    }
}
