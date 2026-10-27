//! Typed retry receipt lookups; command outcome serialization belongs to its API.
use super::{Backend, Error, Result};
use crate::bots::models::Receipt;
pub(crate) struct ReceiptRead<'a> {
    pub(crate) account: &'a str,
    pub(crate) key: &'a str,
    pub(crate) after: u64,
}
macro_rules! receipt_read {
    ($session:expr, $method:ident $(, $argument:expr)* $(,)?) => {{
        $session.ensure_available()?;
        let result = match $session.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::receipts::$method(db $(, $argument)*).await,
            Backend::Postgres(db) => super::postgres::receipts::$method(db $(, $argument)*).await,
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        $session.checked(result)
    }};
}
impl super::Connection {
    async fn message_receipt_async(
        &mut self,
        request: &ReceiptRead<'_>,
    ) -> Result<Option<Receipt>> {
        receipt_read!(self, message, request)
    }
    #[cfg(test)]
    async fn message_receipt_count_async(&mut self, account: &str, after: u64) -> Result<usize> {
        receipt_read!(self, message_count, account, after)
    }
    async fn command_receipt_async(
        &mut self,
        request: &ReceiptRead<'_>,
    ) -> Result<Option<(String, String)>> {
        receipt_read!(self, command, request)
    }
    #[cfg(test)]
    async fn command_receipt_usage_async(
        &mut self,
        account: &str,
        after: u64,
    ) -> Result<(usize, usize)> {
        receipt_read!(self, command_usage, account, after)
    }
}
impl super::Handle {
    #[cfg(test)]
    pub(crate) fn message_receipt_blocking(
        &self,
        request: &ReceiptRead<'_>,
    ) -> Result<Option<Receipt>> {
        super::runtime::run(self.message_receipt(request))
    }
    pub(crate) async fn message_receipt(
        &self,
        request: &ReceiptRead<'_>,
    ) -> Result<Option<Receipt>> {
        self.acquire_read()
            .await?
            .message_receipt_async(request)
            .await
    }
    #[cfg(test)]
    pub(crate) fn message_receipt_count_blocking(
        &self,
        account: &str,
        after: u64,
    ) -> Result<usize> {
        super::runtime::run(self.message_receipt_count(account, after))
    }
    #[cfg(test)]
    pub(crate) async fn message_receipt_count(&self, account: &str, after: u64) -> Result<usize> {
        self.acquire()
            .await?
            .message_receipt_count_async(account, after)
            .await
    }
    pub(crate) async fn command_receipt(
        &self,
        request: &ReceiptRead<'_>,
    ) -> Result<Option<(String, String)>> {
        self.acquire_read()
            .await?
            .command_receipt_async(request)
            .await
    }
    #[cfg(test)]
    pub(crate) fn command_receipt_usage_blocking(
        &self,
        account: &str,
        after: u64,
    ) -> Result<(usize, usize)> {
        super::runtime::run(self.command_receipt_usage(account, after))
    }
    #[cfg(test)]
    pub(crate) async fn command_receipt_usage(
        &self,
        account: &str,
        after: u64,
    ) -> Result<(usize, usize)> {
        self.acquire_read()
            .await?
            .command_receipt_usage_async(account, after)
            .await
    }
}
