//! Atomic self-service account metadata and managed-avatar completion.
use super::{Error, Result};
use crate::engine::media::Media;

pub(crate) struct ProfileWrite<'a> {
    pub(crate) user: &'a str,
    pub(crate) account: &'a str,
    pub(crate) json: Option<&'a str>,
    pub(crate) avatar: Option<&'a Media>,
    pub(crate) at: u64,
}
impl ProfileWrite<'_> {
    pub(super) fn validate(&self) -> Result<()> {
        if self
            .avatar
            .is_some_and(|media| media.uploader != self.account || media.conversation != "@avatar")
        {
            return Err(Error::InvalidRequest("Avatar ownership changed."));
        }
        if let Some(json) = self.json {
            let value: serde_json::Value = serde_json::from_str(json)
                .map_err(|_| Error::Conversion("Invalid account JSON"))?;
            if value.get("id").and_then(serde_json::Value::as_str) != Some(self.account) {
                return Err(Error::RevisionConflict);
            }
        }
        Ok(())
    }
}
impl super::Handle {
    pub(crate) async fn save_profile(&self, request: &ProfileWrite<'_>) -> Result<()> {
        request.validate()?;
        let mut session = self.acquire().await?;
        session
            .save_write_async(&super::transaction::WriteRequest::Profile(request), self)
            .await?;
        Ok(())
    }
}
