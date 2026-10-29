//! Common browser/bot socket lifetime, heartbeat and frame admission.
use super::{App, admission::SocketLease};
use std::{
    future::Future,
    time::{Duration, Instant},
};

pub(crate) const MAX_FRAME_BYTES: usize = 65536;
const HEARTBEAT: Duration = Duration::from_secs(25);
const IDLE: Duration = Duration::from_secs(75);

pub(crate) async fn run(app: &App, mut lease: SocketLease, connection: impl Future<Output = ()>) {
    tokio::select! {
        biased;
        _ = shutdown(app) => {},
        _ = lease.cancelled() => {},
        _ = connection => {},
    }
}
async fn shutdown(app: &App) {
    let notified = app.shutdown.notified();
    tokio::pin!(notified);
    // Register before the flag check so shutdown cannot be lost between them.
    notified.as_mut().enable();
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    notified.await;
}
/// A subscription is only a negative routing hint, never authorization. Refresh
/// after baselines; oversized projections deliberately fall back to live routing.
#[derive(Clone, Default)]
pub(crate) struct Interests {
    initialized: bool,
    principal: Option<String>,
    epoch: Option<(u64, crate::engine::MetadataClock)>,
    ids: Option<std::collections::BTreeSet<String>>,
}
impl Interests {
    pub(crate) fn excludes(&self, change: &super::Change) -> bool {
        if let super::Change::Metadata(notice) = change
            && self
                .principal
                .as_ref()
                .is_some_and(|id| !notice.targets(id))
            && !notice.has_catalogues()
        {
            return true;
        }
        if self.stale() {
            return false;
        }
        match change {
            super::Change::Conversation(id) | super::Change::BotCommands(id) => {
                self.ids.as_ref().is_some_and(|ids| !ids.contains(id))
            }
            super::Change::BotCatalogues(changed) => self
                .ids
                .as_ref()
                .is_some_and(|ids| ids.is_disjoint(changed)),
            _ => false,
        }
    }
    pub(crate) fn bind(&mut self, principal: &str) {
        self.principal = Some(principal.into());
    }
    pub(crate) fn refresh_needed(&self, baseline: bool) -> bool {
        !self.initialized || baseline || self.stale()
    }
    fn stale(&self) -> bool {
        self.epoch
            .as_ref()
            .is_some_and(|(revision, clock)| *revision != clock.current())
    }
    pub(crate) fn refresh(
        &mut self,
        ids: std::collections::BTreeSet<String>,
        clock: crate::engine::MetadataClock,
    ) {
        self.initialized = true;
        self.epoch = Some((clock.current(), clock));
        self.ids = (ids.len() <= 4096).then_some(ids);
    }
}

pub(crate) enum Event {
    Heartbeat(bool),
    Change(Result<super::Change, tokio::sync::broadcast::error::RecvError>),
    Activity(Result<serde_json::Value, tokio::sync::broadcast::error::RecvError>),
    Incoming(Option<Result<axum::extract::ws::Message, axum::Error>>),
    Maintenance,
}
// Adapters render events and apply their frame policy; all socket polling and
// liveness behavior lives here so the two transports cannot drift independently.
pub(crate) struct Events {
    clock: Clock,
    changes: tokio::sync::broadcast::Receiver<super::Change>,
    activities: tokio::sync::broadcast::Receiver<serde_json::Value>,
    maintenance: Option<tokio::time::Interval>,
    pending: std::collections::VecDeque<super::Change>,
}
impl Events {
    pub(crate) fn new(app: &App, maintenance: bool) -> Self {
        Self {
            clock: Clock::new(),
            pending: Default::default(),
            changes: app.changes.subscribe(),
            activities: app.bot_runtimes.activities.subscribe(),
            maintenance: maintenance.then(|| tokio::time::interval(Duration::from_secs(1))),
        }
    }
    pub(crate) async fn next(&mut self, socket: &mut axum::extract::ws::WebSocket) -> Event {
        if let Some(change) = self.pending.pop_front() {
            return Event::Change(Ok(change));
        }
        tokio::select! {
            alive = self.clock.tick() => Event::Heartbeat(alive),
            change = self.changes.recv() => Event::Change(self.batch(change)),
            activity = self.activities.recv() => Event::Activity(activity),
            incoming = socket.recv() => { self.clock.received(); Event::Incoming(incoming) },
            _ = async { match &mut self.maintenance { Some(tick) => { tick.tick().await; }, None => std::future::pending().await } } => Event::Maintenance,
        }
    }
    /// Coalescing drains at most 64 events; render at most eight per worker so
    /// large conversation frames cannot reserve unbounded serialized memory.
    pub(crate) fn take_batch(&mut self, first: super::Change) -> Vec<super::Change> {
        let mut changes = vec![first];
        for _ in 1..8 {
            let Some(change) = self.pending.pop_front() else {
                break;
            };
            changes.push(change);
        }
        changes
    }
    fn batch(
        &mut self,
        first: Result<super::Change, tokio::sync::broadcast::error::RecvError>,
    ) -> Result<super::Change, tokio::sync::broadcast::error::RecvError> {
        use super::Change;
        use tokio::sync::broadcast::error::{RecvError, TryRecvError};
        let mut changes = Vec::new();
        match first {
            Ok(change) => changes.push(change),
            Err(RecvError::Lagged(_)) => changes.push(Change::All),
            Err(error) => return Err(error),
        }
        // Bound work per turn even when producers keep writing. The next turn
        // drains another batch; lag always schedules a live-authorized baseline.
        for _ in 1..64 {
            match self.changes.try_recv() {
                Ok(change) => changes.push(change),
                Err(TryRecvError::Lagged(_)) => changes.push(Change::All),
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        if changes.contains(&Change::Shutdown) {
            self.pending.clear();
            return Ok(Change::Shutdown);
        }
        let all = changes.contains(&Change::All);
        let mut metadata = crate::engine::MetadataNotice::default();
        let mut has_metadata = false;
        for change in &changes {
            if let Change::Metadata(notice) = change {
                metadata.merge(notice);
                has_metadata = true;
            }
        }
        if all {
            self.pending.push_back(Change::All);
            metadata = metadata.catalogues_only();
        }
        if has_metadata && (!all || metadata.has_catalogues()) {
            self.pending
                .push_back(Change::Metadata(std::sync::Arc::new(metadata)));
        }
        for change in changes {
            if matches!(change, Change::All | Change::Metadata(_))
                || (all
                    && !matches!(
                        change,
                        Change::BotCommands(_) | Change::BotCatalogues(_) | Change::Presence
                    ))
            {
                continue;
            }
            if !self.pending.contains(&change) {
                self.pending.push_back(change);
            }
        }
        self.pending.pop_front().ok_or(RecvError::Closed)
    }
}
pub(crate) struct Clock {
    heartbeat: tokio::time::Interval,
    last_seen: Instant,
}
impl Clock {
    pub(crate) fn new() -> Self {
        Self {
            heartbeat: tokio::time::interval(HEARTBEAT),
            last_seen: Instant::now(),
        }
    }
    pub(crate) fn received(&mut self) {
        self.last_seen = Instant::now();
    }
    pub(crate) async fn tick(&mut self) -> bool {
        self.heartbeat.tick().await;
        self.last_seen.elapsed() <= IDLE
    }
}
pub(crate) struct Budget {
    start: Instant,
    used: u32,
}
impl Budget {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            used: 0,
        }
    }
    pub(crate) fn admit(&mut self, now: Instant) -> bool {
        if now.duration_since(self.start) >= Duration::from_secs(10) {
            self.start = now;
            self.used = 0;
        }
        self.used = self.used.saturating_add(1);
        self.used <= 30
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn interests_are_bounded_negative_hints_and_baselines_always_refresh_them() {
        use super::super::Change;
        let mut interests = Interests::default();
        let mut engine = crate::engine::Engine::open(std::path::Path::new(":memory:")).unwrap();
        let clock = engine.metadata_clock();
        assert!(interests.refresh_needed(false));
        assert!(!interests.excludes(&Change::Conversation("unknown".into())));
        interests.refresh(["visible".into()].into(), clock.clone());
        assert!(!interests.refresh_needed(false));
        assert!(interests.refresh_needed(true));
        assert!(!interests.excludes(&Change::Conversation("visible".into())));
        assert!(interests.excludes(&Change::Conversation("hidden".into())));
        assert!(!interests.excludes(&Change::All));
        interests.refresh(["newly-granted".into()].into(), clock.clone());
        assert!(interests.excludes(&Change::Conversation("visible".into())));
        assert!(!interests.excludes(&Change::BotCommands("newly-granted".into())));
        engine
            .provision("new-account", "hash".into(), false, false)
            .unwrap();
        assert!(interests.refresh_needed(false));
        assert!(!interests.excludes(&Change::Conversation("previously-hidden".into())));
        interests.refresh((0..4097).map(|index| index.to_string()).collect(), clock);
        assert!(!interests.excludes(&Change::Conversation("fallback-to-live".into())));
        assert!(!interests.refresh_needed(false));
        assert!(interests.ids.is_none());
    }
    #[tokio::test]
    async fn queued_changes_coalesce_without_losing_discovery_presence_or_shutdown() {
        use super::super::Change;
        let mut app = crate::web::tests::login_test_app();
        app.changes = tokio::sync::broadcast::channel(256).0;
        let mut events = Events::new(&app, false);
        for change in [
            Change::Conversation("one".into()),
            Change::Presence,
            Change::Presence,
            Change::Avatar("profile".into()),
            Change::Avatar("profile".into()),
            Change::All,
            Change::Conversation("two".into()),
            Change::BotCommands("one".into()),
            Change::BotCommands("one".into()),
        ] {
            app.changes.send(change).unwrap();
        }
        let first = events.changes.try_recv().unwrap();
        assert_eq!(events.batch(Ok(first)).unwrap(), Change::All);
        assert_eq!(events.pending.pop_front(), Some(Change::Presence));
        assert_eq!(
            events.pending.pop_front(),
            Some(Change::BotCommands("one".into()))
        );
        assert!(events.pending.is_empty());
        app.changes
            .send(Change::Conversation("one".into()))
            .unwrap();
        app.changes.send(Change::Shutdown).unwrap();
        let first = events.changes.try_recv().unwrap();
        assert_eq!(events.batch(Ok(first)).unwrap(), Change::Shutdown);
        assert!(events.pending.is_empty());
    }
    #[tokio::test]
    async fn metadata_batches_union_audiences_and_preserve_catalogues_during_recovery() {
        use super::super::Change;
        let mut app = crate::web::tests::login_test_app();
        app.changes = tokio::sync::broadcast::channel(256).0;
        let (first, second, alice, bob) = {
            let mut engine = app.engine_api().unwrap();
            engine.execute(None, None, "/grant alice admin").unwrap();
            engine
                .provision("bob", "hash".into(), false, false)
                .unwrap();
            engine
                .execute(Some("alice"), None, "/room new hidden")
                .unwrap();
            let first = engine.metadata_change();
            engine.execute(None, None, "/room add bob #hidden").unwrap();
            engine
                .execute(Some("alice"), Some("hidden"), "/bot new helper")
                .unwrap();
            (
                first,
                engine.metadata_change(),
                engine.account_id("alice").unwrap().to_owned(),
                engine.account_id("bob").unwrap().to_owned(),
            )
        };
        let mut events = Events::new(&app, false);
        for recovery in [false, true] {
            app.changes.send(Change::Metadata(first.clone())).unwrap();
            app.changes.send(Change::Metadata(second.clone())).unwrap();
            if recovery {
                app.changes.send(Change::All).unwrap();
            }
            let first = events.changes.try_recv().unwrap();
            let change = events.batch(Ok(first)).unwrap();
            let notice = if recovery {
                assert_eq!(change, Change::All);
                events
                    .pending
                    .pop_front()
                    .and_then(|change| match change {
                        Change::Metadata(notice) => Some(notice),
                        _ => None,
                    })
                    .unwrap()
            } else {
                match change {
                    Change::Metadata(notice) => Some(notice),
                    _ => None,
                }
                .unwrap()
            };
            assert_eq!(notice.targets(&alice), !recovery);
            assert_eq!(notice.targets(&bob), !recovery);
            assert_eq!(
                notice.catalogues().collect::<Vec<_>>(),
                second.catalogues().collect::<Vec<_>>()
            );
            assert!(notice.has_catalogues());
            assert!(events.pending.is_empty());
        }
    }
    #[tokio::test]
    async fn lag_recovery_and_draining_are_bounded() {
        use super::super::Change;
        let mut app = crate::web::tests::login_test_app();
        app.changes = tokio::sync::broadcast::channel(256).0;
        let mut events = Events::new(&app, false);
        for index in 0..200 {
            app.changes
                .send(Change::Conversation(index.to_string()))
                .unwrap();
        }
        let first = events.changes.try_recv().unwrap();
        assert_eq!(
            events.batch(Ok(first)).unwrap(),
            Change::Conversation("0".into())
        );
        assert_eq!(events.pending.len(), 63);
        assert_eq!(events.changes.len(), 136);
        let rendered = events.take_batch(Change::Conversation("0".into()));
        assert_eq!(
            rendered,
            (0..8)
                .map(|index| Change::Conversation(index.to_string()))
                .collect::<Vec<_>>()
        );
        assert_eq!(events.pending.len(), 56);
        assert_eq!(events.changes.len(), 136);
        events.pending.clear();
        assert_eq!(
            events
                .batch(Err(tokio::sync::broadcast::error::RecvError::Lagged(3)))
                .unwrap(),
            Change::All
        );
        assert!(events.pending.is_empty());
    }
    #[test]
    fn frame_budget_resets_at_the_window_boundary() {
        let mut budget = Budget::new();
        let now = budget.start;
        for _ in 0..30 {
            assert!(budget.admit(now));
        }
        assert!(!budget.admit(now));
        assert!(budget.admit(now + Duration::from_secs(10)));
    }
    #[tokio::test]
    async fn preexisting_shutdown_is_observed_without_another_notification() {
        let app = crate::web::tests::login_test_app();
        app.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        tokio::time::timeout(Duration::from_millis(50), shutdown(&app))
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn shutdown_wakes_every_registered_connection() {
        let app = crate::web::tests::login_test_app();
        let first = tokio::spawn({
            let app = app.clone();
            async move { shutdown(&app).await }
        });
        let second = tokio::spawn({
            let app = app.clone();
            async move { shutdown(&app).await }
        });
        tokio::task::yield_now().await;
        app.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        app.shutdown.notify_waiters();
        tokio::time::timeout(Duration::from_millis(50), async {
            first.await.unwrap();
            second.await.unwrap();
        })
        .await
        .unwrap();
    }
}
