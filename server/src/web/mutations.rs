//! Conversation ordering and metadata stability for asynchronous mutations.
//! Message transactions share the metadata fence. Administrative plans take it
//! exclusively before preparation, so authority stays stable through native commit
//! and publication. Engine guards cover memory operations only.
#[cfg(test)]
use crate::engine::database;
use crate::engine::{Engine, EngineError};
#[cfg(test)]
use std::ops::{Deref, DerefMut};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
};

#[derive(Default)]
pub(crate) struct Coordinator {
    metadata: Arc<RwLock<()>>,
    recovering: std::sync::atomic::AtomicBool,
    recovery_done: tokio::sync::Notify,
    pub(crate) audit: AsyncMutex<()>,
    conversations: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}
#[derive(Clone)]
pub(crate) struct ConversationPermit(Arc<Reservation>);
#[derive(Clone)]
pub(crate) struct MetadataPermit(Arc<MetadataReservation>);
struct MetadataReservation {
    coordinator: Arc<Coordinator>,
    _metadata: OwnedRwLockWriteGuard<()>,
}
struct Reservation {
    coordinator: Arc<Coordinator>,
    ids: Vec<String>,
    conversations: Vec<OwnedMutexGuard<()>>,
    _metadata: OwnedRwLockReadGuard<()>,
}
impl Coordinator {
    pub(crate) fn recovering(&self) -> bool {
        self.recovering.load(std::sync::atomic::Ordering::Acquire)
    }
    pub(crate) async fn wait_for_recovery(&self) {
        loop {
            let notified = self.recovery_done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.recovering() {
                break;
            }
            notified.await;
        }
    }
    pub(crate) async fn metadata(self: &Arc<Self>) -> MetadataPermit {
        MetadataPermit(Arc::new(MetadataReservation {
            coordinator: self.clone(),
            _metadata: self.metadata.clone().write_owned().await,
        }))
    }
    #[cfg(test)]
    pub(crate) fn exclusive(&self) -> Result<OwnedRwLockWriteGuard<()>, EngineError> {
        if let Ok(permit) = self.metadata.clone().try_write_owned() {
            return Ok(permit);
        }
        database::block_on_storage(self.metadata.clone().write_owned()).map_err(Into::into)
    }
    pub(crate) async fn conversation(
        self: &Arc<Self>,
        id: String,
    ) -> Result<ConversationPermit, EngineError> {
        self.conversations(vec![id]).await
    }
    pub(crate) async fn conversations(
        self: &Arc<Self>,
        mut ids: Vec<String>,
    ) -> Result<ConversationPermit, EngineError> {
        ids.sort();
        ids.dedup();
        let conversations = {
            let mut entries = self.conversations.lock().map_err(|_| {
                EngineError::Unavailable(
                    "Mutation ordering unavailable; restart the service.".into(),
                )
            })?;
            entries.retain(|_, entry| entry.strong_count() > 0);
            ids.iter()
                .map(|id| match entries.get(id).and_then(Weak::upgrade) {
                    Some(conversation) => conversation,
                    None => {
                        let conversation = Arc::new(AsyncMutex::new(()));
                        entries.insert(id.clone(), Arc::downgrade(&conversation));
                        conversation
                    }
                })
                .collect::<Vec<_>>()
        };
        // Acquire every conversation in stable order before the metadata fence.
        // A queued metadata writer can then run between lock acquisition and
        // preparation without waiting on a permit that waits on that writer.
        let mut guards = Vec::with_capacity(conversations.len());
        for conversation in conversations {
            guards.push(conversation.lock_owned().await);
        }
        let metadata = self.metadata.clone().read_owned().await;
        Ok(ConversationPermit(Arc::new(Reservation {
            coordinator: self.clone(),
            ids,
            conversations: guards,
            _metadata: metadata,
        })))
    }
}
impl ConversationPermit {
    fn belongs_to(&self, coordinator: &Arc<Coordinator>) -> bool {
        Arc::ptr_eq(&self.0.coordinator, coordinator)
    }
}
trait EnginePermit: Clone + Send + 'static {
    fn belongs_to(&self, coordinator: &Arc<Coordinator>) -> bool;
}
impl EnginePermit for ConversationPermit {
    fn belongs_to(&self, coordinator: &Arc<Coordinator>) -> bool {
        ConversationPermit::belongs_to(self, coordinator)
    }
}
impl EnginePermit for MetadataPermit {
    fn belongs_to(&self, coordinator: &Arc<Coordinator>) -> bool {
        Arc::ptr_eq(&self.0.coordinator, coordinator)
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.conversations.clear();
        if let Ok(mut entries) = self.coordinator.conversations.lock() {
            for id in &self.ids {
                if entries
                    .get(id)
                    .is_some_and(|entry| entry.strong_count() == 0)
                {
                    entries.remove(id);
                }
            }
        }
    }
}
#[cfg(test)]
pub struct EngineGuard<'a> {
    pub(super) engine: std::sync::RwLockWriteGuard<'a, Engine>,
    pub(super) _metadata: OwnedRwLockWriteGuard<()>,
}
#[cfg(test)]
impl Deref for EngineGuard<'_> {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.engine
    }
}
#[cfg(test)]
impl DerefMut for EngineGuard<'_> {
    fn deref_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }
}
impl super::App {
    /// Only owned operation preparation/publication belongs here. Database work
    /// must be awaited outside the engine guard, while the permit remains held.
    pub(crate) async fn with_conversation<T, E>(
        &self,
        permit: &ConversationPermit,
        mutate: impl FnOnce(&mut Engine) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<EngineError> + Send + 'static,
    {
        self.with_ordering(permit, mutate).await
    }
    pub(crate) async fn with_metadata<T, E>(
        &self,
        permit: &MetadataPermit,
        mutate: impl FnOnce(&mut Engine) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<EngineError> + Send + 'static,
    {
        self.with_ordering(permit, mutate).await
    }
    async fn with_ordering<T, E, P: EnginePermit>(
        &self,
        permit: &P,
        mutate: impl FnOnce(&mut Engine) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<EngineError> + Send + 'static,
    {
        if !permit.belongs_to(&self.mutations) {
            return Err(
                EngineError::Internal("Mutation permit belongs to another engine.".into()).into(),
            );
        }
        let app = self.clone();
        match self.engine.try_write() {
            Ok(engine) => {
                return std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    let mut engine = engine;
                    let result = engine
                        .ensure_memory()
                        .map_err(E::from)
                        .and_then(|()| mutate(&mut engine));
                    let recover = engine.needs_memory_recovery();
                    drop(engine);
                    if recover {
                        app.schedule_memory_recovery();
                    }
                    result
                }))
                .map_err(|_| E::from(EngineError::unavailable("Mutation worker panicked")))?;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(EngineError::unavailable("Engine state poisoned").into());
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let permit = permit.clone();
        tokio::task::spawn_blocking(move || {
            let mut engine = app
                .engine
                .write()
                .map_err(|_| EngineError::unavailable("Engine state poisoned"))?;
            let result = engine
                .ensure_memory()
                .map_err(E::from)
                .and_then(|()| mutate(&mut engine));
            let recover = engine.needs_memory_recovery();
            drop(engine);
            drop(permit);
            if recover {
                app.schedule_memory_recovery();
            }
            result
        })
        .await
        .map_err(|error| E::from(EngineError::unavailable(error)))?
    }

    fn schedule_memory_recovery(&self) {
        use std::sync::atomic::Ordering;
        if self.mutations.recovering.swap(true, Ordering::AcqRel) {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let result = async {
                // Recovery is single-flight and participates in shutdown drain.
                let _io = app
                    .io_jobs
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(EngineError::unavailable)?;
                let _metadata = app.mutations.metadata().await;
                let database = {
                    let mut engine = app
                        .engine
                        .write()
                        .map_err(|_| EngineError::unavailable("Engine state poisoned"))?;
                    engine.begin_memory_recovery()
                };
                let Some(database) = database else {
                    return Ok::<_, EngineError>(());
                };
                let (snapshot, cursors) = database.recovery_snapshot().await?;
                {
                    let mut engine = app
                        .engine
                        .write()
                        .map_err(|_| EngineError::unavailable("Engine state poisoned"))?;
                    engine.restore_committed_metadata(snapshot, cursors)?;
                }
                // Invalidate all audiences only after validated state is installed.
                let _ = app.changes.send(super::Change::All);
                Ok(())
            }
            .await;
            if let Err(error) = result {
                tracing::error!(%error,"Committed-state recovery failed; restart required");
            }
            app.mutations.recovering.store(false, Ordering::Release);
            app.mutations.recovery_done.notify_waiters();
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn uncontended_memory_phases_stay_on_the_async_thread() {
        let app = crate::web::tests::login_test_app();
        let thread = std::thread::current().id();
        let actual: Result<_, EngineError> = app
            .inspect_engine(|_, _| Ok(std::thread::current().id()))
            .await;
        assert_eq!(actual.unwrap(), thread);
        let permit = app
            .mutations
            .conversation("memory-only".into())
            .await
            .unwrap();
        let actual: Result<_, EngineError> = app
            .with_conversation(&permit, |_| Ok(std::thread::current().id()))
            .await;
        assert_eq!(actual.unwrap(), thread);
    }

    #[tokio::test]
    async fn shutdown_yields_drain_permits_to_queued_metadata_recovery() {
        let mut app =
            crate::web::media::tests::app(Engine::open(std::path::Path::new(":memory:")).unwrap());
        app.io_jobs = Arc::new(tokio::sync::Semaphore::new(
            usize::try_from(super::super::IO_JOBS).unwrap(),
        ));
        let id = {
            let mut engine = app.engine().unwrap();
            engine
                .execute(Some("alice"), None, "/room new recovered")
                .unwrap();
            engine.data.rooms["recovered"].id.clone()
        };
        let permit = app.mutations.conversation(id.clone()).await.unwrap();
        let pending = app
            .with_conversation(&permit, |engine| {
                engine.prepare_browser_message("alice", Some("recovered"), "durable", false)
            })
            .await
            .unwrap();
        let commit = pending.save(None).await.unwrap();
        app.engine
            .write()
            .unwrap()
            .data
            .rooms
            .get_mut("recovered")
            .unwrap()
            .id = uuid::Uuid::new_v4().to_string();
        let occupied = app
            .io_jobs
            .clone()
            .acquire_many_owned(super::super::IO_JOBS)
            .await
            .unwrap();
        app.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let shutdown = tokio::spawn({
            let app = app.clone();
            async move { app.checkpoint_after_jobs().await }
        });
        tokio::task::yield_now().await;
        assert!(
            app.with_conversation(&permit, move |engine| engine
                .publish_message(pending, commit))
                .await
                .is_err()
        );
        drop(permit);
        drop(occupied);
        tokio::time::timeout(Duration::from_secs(2), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(app.io_jobs.is_closed());
        assert_eq!(app.engine_read().unwrap().data.rooms["recovered"].id, id);
    }

    #[tokio::test]
    async fn committed_publication_mismatch_reloads_metadata_and_resumes_service() {
        recovery_contract(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
            false,
        )
        .await;
    }
    #[tokio::test]
    async fn failed_authoritative_reload_remains_unavailable() {
        recovery_contract(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
            true,
        )
        .await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
    async fn postgres_committed_publication_mismatch_recovers_authoritative_metadata() {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        recovery_contract(Engine::open_postgres(&fixture.url).unwrap(), false).await;
    }
    async fn recovery_contract(engine: Engine, corrupt_database: bool) {
        let app = crate::web::media::tests::app(engine);
        let (id, database) = {
            let mut engine = app.engine().unwrap();
            engine
                .execute(Some("alice"), None, "/room new recovered")
                .unwrap();
            (
                engine.data.rooms["recovered"].id.clone(),
                engine.auth_store(),
            )
        };
        let permit = app.mutations.conversation(id.clone()).await.unwrap();
        let pending = app
            .with_conversation(&permit, |engine| {
                engine.prepare_browser_message("alice", Some("recovered"), "durable message", false)
            })
            .await
            .unwrap();
        let commit = pending.save(None).await.unwrap();
        {
            let mut engine = app.engine.write().unwrap();
            engine.data.rooms.get_mut("recovered").unwrap().id = uuid::Uuid::new_v4().to_string();
            if corrupt_database {
                engine.test_corrupt_recovery_metadata();
            }
        }
        let mut changes = app.changes.subscribe();
        let result = app
            .with_conversation(&permit, move |engine| {
                engine.publish_message(pending, commit)
            })
            .await;
        assert!(result.is_err());
        drop(permit);
        tokio::time::timeout(Duration::from_secs(2), async {
            while app
                .mutations
                .recovering
                .load(std::sync::atomic::Ordering::Acquire)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let restored: Result<_, EngineError> = app
            .inspect_engine(|engine, _| Ok(engine.data.rooms["recovered"].id.clone()))
            .await;
        if corrupt_database {
            assert!(restored.is_err());
            assert!(changes.try_recv().is_err());
        } else {
            assert_eq!(restored.unwrap(), id);
            assert_eq!(changes.try_recv().unwrap(), crate::web::Change::All);
            database.health().await.unwrap();
            let permit = app.mutations.conversation(id).await.unwrap();
            let page = app
                .with_conversation(&permit, |engine| {
                    engine.prepare_browser_message(
                        "alice",
                        Some("recovered"),
                        "after recovery",
                        false,
                    )
                })
                .await
                .unwrap();
            let commit = page.save(None).await.unwrap();
            app.with_conversation(&permit, move |engine| engine.publish_message(page, commit))
                .await
                .unwrap();
        }
    }
    #[tokio::test]
    async fn panicking_ordered_mutation_fails_closed_after_poisoning_the_engine() {
        let app = crate::web::tests::login_test_app();
        let permit = app
            .mutations
            .conversation("panic-test".into())
            .await
            .unwrap();
        let result: Result<(), EngineError> = app
            .with_conversation(&permit, |_engine| {
                // Inject an unexpected failure through the actual guarded worker.
                panic!("injected mutation panic")
            })
            .await;
        assert!(matches!(result, Err(EngineError::Unavailable(_))));
        assert!(app.engine.is_poisoned());
        let next: Result<(), EngineError> = app.with_conversation(&permit, |_engine| Ok(())).await;
        assert!(matches!(next, Err(EngineError::Unavailable(_))));
        let read: Result<(), EngineError> = app.inspect_engine(|_, _| Ok(())).await;
        assert!(matches!(read, Err(EngineError::Unavailable(_))));
    }
    #[tokio::test]
    async fn bulk_conversations_do_not_deadlock_a_queued_metadata_writer() {
        let coordinator = Arc::new(Coordinator::default());
        let one = coordinator.conversation("b".into()).await.unwrap();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn({
            let coordinator = coordinator.clone();
            async move {
                let metadata = coordinator.metadata.clone().write_owned().await;
                entered.send(()).unwrap();
                release_rx.await.unwrap();
                drop(metadata);
            }
        });
        tokio::task::yield_now().await;
        let bulk = tokio::spawn({
            let coordinator = coordinator.clone();
            async move {
                coordinator
                    .conversations(vec!["b".into(), "a".into(), "a".into()])
                    .await
                    .unwrap()
            }
        });
        tokio::task::yield_now().await;
        drop(one);
        tokio::time::timeout(Duration::from_millis(100), entered_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(!bulk.is_finished());
        release.send(()).unwrap();
        writer.await.unwrap();
        let bulk = tokio::time::timeout(Duration::from_millis(100), bulk)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(10),
                coordinator.conversations(vec!["a".into(), "b".into()])
            )
            .await
            .is_err()
        );
        drop(bulk);
        let reversed = coordinator
            .conversations(vec!["b".into(), "a".into()])
            .await
            .unwrap();
        drop(reversed);
        assert!(coordinator.conversations.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn conversations_order_independently_and_metadata_waits_for_publication() {
        let coordinator = Arc::new(Coordinator::default());
        let one = coordinator.conversation("one".into()).await.unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(10),
                coordinator.conversation("one".into())
            )
            .await
            .is_err()
        );
        let two = tokio::time::timeout(
            Duration::from_millis(50),
            coordinator.conversation("two".into()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(10),
                coordinator.metadata.clone().write_owned()
            )
            .await
            .is_err()
        );
        drop(one);
        drop(two);
        let metadata = coordinator.metadata.clone().write_owned().await;
        assert!(coordinator.conversations.lock().unwrap().is_empty());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(10),
                coordinator.conversation("cancelled".into())
            )
            .await
            .is_err()
        );
        drop(metadata);
        let next = coordinator.conversation("next".into()).await.unwrap();
        drop(next);
        assert!(coordinator.conversations.lock().unwrap().is_empty());
    }
}
