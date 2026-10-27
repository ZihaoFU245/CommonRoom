//! Owned monotonic read cursor operations above the async storage boundary.
use super::*;
pub(crate) struct CursorWrite {
    database: database::Handle,
    history: std::sync::Arc<HistoryStore>,
    pub(crate) user: String,
    key: String,
    conversation: String,
    through: u64,
    pub(crate) unchanged: bool,
}
impl Engine {
    pub(crate) fn prepare_read(
        &self,
        user: &str,
        view: &str,
        through: u64,
    ) -> Result<CursorWrite, EngineError> {
        if through > 9_007_199_254_740_991 {
            return Err(EngineError::Invalid("Invalid sequence.".into()));
        }
        let (key, conversation, _) = self.conversation(user, view)?;
        Ok(CursorWrite {
            database: self.store.db.clone(),
            history: self.history_store().clone(),
            user: user.into(),
            unchanged: self.read_position(user, &key) >= through,
            key,
            conversation,
            through,
        })
    }
    pub(crate) fn publish_read(&mut self, cursor: CursorWrite) -> Result<bool, EngineError> {
        self.publish_read_position(&cursor.user, &cursor.key, cursor.through)
    }
}
impl CursorWrite {
    pub(crate) async fn retained(&self) -> Result<bool, EngineError> {
        self.history
            .contains_sequence_async(&self.conversation, self.through)
            .await
    }
    pub(crate) async fn save(&self) -> Result<(), EngineError> {
        if self.unchanged {
            self.history.ensure_available_async().await?;
        } else {
            self.database
                .advance_cursor(&database::CursorAdvance {
                    user: &self.user,
                    conversation: &self.key,
                    through: self.through,
                })
                .await?;
        }
        Ok(())
    }
}
