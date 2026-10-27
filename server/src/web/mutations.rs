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
        let app = self.clone();
        let permit = permit.clone();
        tokio::task::spawn_blocking(move || {
            if !permit.belongs_to(&app.mutations) {
                return Err(EngineError::Internal(
                    "Mutation permit belongs to another engine.".into(),
                )
                .into());
            }
            let mut engine = app.engine.write().map_err(|_| {
                EngineError::Unavailable("Server state unavailable; restart the service.".into())
            })?;
            engine.ensure_memory()?;
            let result = mutate(&mut engine);
            drop(engine);
            drop(permit);
            result
        })
        .await
        .map_err(|error| E::from(EngineError::unavailable(error)))?
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::time::Duration;
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
