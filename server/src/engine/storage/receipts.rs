use super::*;

impl Engine {
    pub(in crate::engine) fn bot_receipt(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<crate::bots::models::Receipt>, EngineError> {
        Ok(self
            .store
            .db
            .message_receipt_blocking(&database::ReceiptRead {
                account,
                key,
                after: now().saturating_sub(crate::bots::models::RECEIPT_TTL),
            })?)
    }

    pub(in crate::engine) fn admit_bot_receipt(
        &self,
        account: &str,
    ) -> Result<(), crate::bots::error::Error> {
        use crate::bots::error::{Code, Error};
        let count = self
            .store
            .db
            .message_receipt_count_blocking(
                account,
                now().saturating_sub(crate::bots::models::RECEIPT_TTL),
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
