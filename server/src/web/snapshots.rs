//! Share baselines by metadata, visible conversation revisions and viewer cursors.
use super::*;
use crate::engine::EngineError;
use axum::extract::ws::Utf8Bytes;
use std::collections::HashMap;
use std::collections::VecDeque;
use tokio::sync::OnceCell;

type Key = (String, bool, crate::engine::SnapshotStamp, u64, bool);
#[derive(Clone)]
struct Baseline {
    frame: Utf8Bytes,
    complete: bool,
}
type Value = Arc<OnceCell<Result<Baseline, SnapshotError>>>;
#[derive(Default)]
pub struct SnapshotCache {
    entries: HashMap<Key, (Value, usize)>,
    order: VecDeque<Key>,
    bytes: usize,
}
impl SnapshotCache {
    fn claim(&mut self, key: &Key) -> Value {
        if let Some((value, _)) = self.entries.get(key) {
            return value.clone();
        }
        self.entries.retain(|old, (_, size)| {
            let keep = old.0 != key.0 || old.1 != key.1 || old.4 != key.4;
            if !keep {
                self.bytes = self.bytes.saturating_sub(*size);
            }
            keep
        });
        self.order
            .retain(|old| old.0 != key.0 || old.1 != key.1 || old.4 != key.4);
        let value = Arc::new(OnceCell::new());
        self.entries.insert(key.clone(), (value.clone(), 0));
        self.order.push_back(key.clone());
        self.trim();
        value
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
        while self.bytes > 32 * 1024 * 1024 || self.entries.len() > 128 {
            if let Some(key) = self.order.pop_front() {
                if let Some((_, size)) = self.entries.remove(&key) {
                    self.bytes -= size;
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
            Self::Browser { headers, bot } => {
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
                        .map(|plan| Plan::Browser(Box::new(plan)))
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
    async fn finish(self) -> Result<(String, bool), EngineError> {
        match self {
            Self::Browser(plan) => {
                let (snapshot, complete) = plan.finish_async().await?;
                tokio::task::spawn_blocking(move || {
                    serde_json::to_string(&snapshot).map(|frame| (frame, complete))
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
                    serde_json::to_string(&snapshot).map(|frame| (frame, complete))
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
    value_for(
        app,
        Viewer::Browser {
            headers: headers.clone(),
            bot,
        },
    )
    .await
    .map_err(Into::into)
}
pub(crate) async fn bot_value_async(
    app: &App,
    context: &crate::bots::auth::Context,
) -> Result<Utf8Bytes, crate::bots::error::Error> {
    value_for(app, Viewer::Bot(context.clone()))
        .await
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
async fn value_for(app: &App, viewer: Viewer) -> Result<Utf8Bytes, SnapshotError> {
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
async fn cached<F: std::future::Future<Output = Result<(String, bool), SnapshotError>>>(
    app: &App,
    key: Key,
    build: impl FnOnce() -> F,
) -> Result<Utf8Bytes, SnapshotError> {
    let value = app
        .snapshots
        .lock()
        .map_err(|_| EngineError::Unavailable("Snapshot cache unavailable.".into()))?
        .claim(&key);
    let result = value
        .get_or_init(|| async {
            build().await.map(|(frame, complete)| Baseline {
                frame: frame.into(),
                complete,
            })
        })
        .await
        .clone();
    let mut cache = app
        .snapshots
        .lock()
        .map_err(|_| EngineError::Unavailable("Snapshot cache unavailable.".into()))?;
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
    result.map(|baseline| baseline.frame)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
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
            )
        };
        let (started, waiting) = tokio::sync::oneshot::channel();
        let builder = tokio::spawn({
            let app = app.clone();
            let key = key.clone();
            async move {
                cached(&app, key, || async {
                    started.send(()).unwrap();
                    std::future::pending::<Result<(String, bool), SnapshotError>>().await
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
                cached(&app, key, || async { Ok(("complete".into(), true)) }).await
            }
        });
        entered.await.unwrap();
        assert!(!follower.is_finished());
        builder.abort();
        assert!(builder.await.unwrap_err().is_cancelled());
        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), follower)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame.as_str(), "complete");
        let reused = cached(&app, key, || async {
            Err(EngineError::Internal("Completed cache entry was rebuilt.".into()).into())
        })
        .await
        .unwrap();
        assert_eq!(frame.as_str().as_ptr(), reused.as_str().as_ptr());
    }
}
