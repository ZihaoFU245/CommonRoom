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

pub(crate) struct Coordinator {
    ordering_timeout: std::time::Duration,
    wait_stats: WaitStats,
    conversation_waits: WaitStats,
    metadata_waits: WaitStats,
    audit_waits: WaitStats,
    metadata: Arc<RwLock<()>>,
    recovering: std::sync::atomic::AtomicBool,
    recovery_done: tokio::sync::Notify,
    pub(crate) audit: AsyncMutex<()>,
    conversations: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}
#[derive(Default)]
struct WaitStats {
    completed: std::sync::atomic::AtomicU64,
    timeouts: std::sync::atomic::AtomicU64,
    total_us: std::sync::atomic::AtomicU64,
    max_us: std::sync::atomic::AtomicU64,
}
impl WaitStats {
    fn record(&self, us: u64, timeout: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.total_us.fetch_add(us, Relaxed);
        self.max_us.fetch_max(us, Relaxed);
        self.completed.fetch_add(1, Relaxed);
        if timeout {
            self.timeouts.fetch_add(1, Relaxed);
        }
    }
    fn stats(&self) -> serde_json::Value {
        use std::sync::atomic::Ordering::Relaxed;
        serde_json::json!({"completed":self.completed.load(Relaxed),
            "timeouts":self.timeouts.load(Relaxed),"total_us":self.total_us.load(Relaxed),
            "max_us":self.max_us.load(Relaxed)})
    }
}
impl Default for Coordinator {
    fn default() -> Self {
        Self::new(std::time::Duration::from_secs(5))
    }
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
    _metadata: Option<OwnedRwLockReadGuard<()>>,
}
impl Coordinator {
    pub(crate) fn new(ordering_timeout: std::time::Duration) -> Self {
        Self {
            ordering_timeout,
            wait_stats: WaitStats::default(),
            conversation_waits: WaitStats::default(),
            metadata_waits: WaitStats::default(),
            audit_waits: WaitStats::default(),
            metadata: Arc::default(),
            recovering: false.into(),
            recovery_done: Default::default(),
            audit: Default::default(),
            conversations: Mutex::default(),
        }
    }
    pub(crate) async fn audit(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, EngineError> {
        self.wait(&self.audit_waits, self.audit.lock()).await
    }
    pub(crate) fn stats(&self) -> serde_json::Value {
        let mut stats = self.wait_stats.stats();
        stats["conversations"] = self.conversation_waits.stats();
        stats["metadata"] = self.metadata_waits.stats();
        stats["audit"] = self.audit_waits.stats();
        stats
    }
    async fn wait<T>(
        &self,
        stats: &WaitStats,
        future: impl std::future::Future<Output = T>,
    ) -> Result<T, EngineError> {
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(self.ordering_timeout, future).await;
        let us = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.wait_stats.record(us, result.is_err());
        stats.record(us, result.is_err());
        result
            .map_err(|_| EngineError::Unavailable("Operation ordering busy; retry shortly.".into()))
    }
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
    pub(crate) async fn metadata(self: &Arc<Self>) -> Result<MetadataPermit, EngineError> {
        let guard = self
            .wait(&self.metadata_waits, self.metadata.clone().write_owned())
            .await?;
        Ok(MetadataPermit(Arc::new(MetadataReservation {
            coordinator: self.clone(),
            _metadata: guard,
        })))
    }
    // A known committed write must recover authoritative memory even under load.
    async fn recovery_metadata(self: &Arc<Self>) -> MetadataPermit {
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
        // Install cleanup before the first await. Cancellation while waiting on
        // a conversation or metadata lock must also remove unused weak entries.
        let mut reservation = Reservation {
            coordinator: self.clone(),
            ids,
            conversations: Vec::with_capacity(conversations.len()),
            _metadata: None,
        };
        self.wait(&self.conversation_waits, async {
            for conversation in conversations {
                reservation
                    .conversations
                    .push(conversation.lock_owned().await);
            }
            reservation._metadata = Some(self.metadata.clone().read_owned().await);
        })
        .await?;
        Ok(ConversationPermit(Arc::new(reservation)))
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
                let _metadata = app.mutations.recovery_metadata().await;
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
mod tests;
