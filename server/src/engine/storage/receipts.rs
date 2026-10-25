use super::*;

impl Engine {
    pub(in crate::engine) fn bot_receipt(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<crate::bots::models::Receipt>, EngineError> {
        use crate::engine::database::OptionalExtension;
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        db.query_row(sql("SELECT digest,conversation,sequence,created FROM bot_receipts WHERE account_id=?1 AND key=?2 AND created>?3", "SELECT digest,conversation,sequence,created FROM bot_receipts WHERE account_id=$1 AND key=$2 AND created>$3"), params![account, key, now().saturating_sub(crate::bots::models::RECEIPT_TTL)], |r| Ok(crate::bots::models::Receipt {
            account_id: account.into(), key: key.into(), digest: r.get(0)?, conversation: r.get(1)?, sequence: r.get(2)?, created: r.get(3)?,
        })).optional().map_err(EngineError::from)
    }

    pub(in crate::engine) fn admit_bot_receipt(
        &self,
        account: &str,
    ) -> Result<(), crate::bots::error::Error> {
        use crate::bots::error::{Code, Error};
        let db = self.store.db.lock().map_err(Error::storage)?;
        let count: usize = db
            .query_row(
                sql(
                    "SELECT count(*) FROM bot_receipts WHERE account_id=?1 AND created>?2",
                    "SELECT count(*) FROM bot_receipts WHERE account_id=$1 AND created>$2",
                ),
                params![
                    account,
                    now().saturating_sub(crate::bots::models::RECEIPT_TTL)
                ],
                |r| r.get(0),
            )
            .map_err(Error::storage)?;
        if count >= crate::bots::models::RECEIPT_LIMIT {
            return Err(Error::new(
                Code::IdempotencyCapacity,
                "Wait for receipts to expire.",
            ));
        }
        Ok(())
    }
}
