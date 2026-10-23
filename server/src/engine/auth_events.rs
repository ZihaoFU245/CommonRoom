//! Bounded authentication events are separate from policy/content audit entries.
use std::net::IpAddr;

#[derive(Clone, Copy, Debug)]
pub(crate) enum AuthOutcome {
    Success,
    Failure,
    Logout,
}
impl AuthOutcome {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Logout => "logout",
        }
    }
}
pub(crate) struct AuthEvent {
    pub(super) at: u64,
    pub(super) client: Option<String>,
    pub(super) username: Option<String>,
    pub(super) outcome: AuthOutcome,
}
impl AuthEvent {
    pub(crate) fn new(
        username: Option<&str>,
        client: Option<IpAddr>,
        outcome: AuthOutcome,
    ) -> Self {
        Self {
            at: super::helpers::now(),
            client: client.map(|ip| ip.to_canonical().to_string()),
            username: username
                .filter(|name| super::valid_name(name))
                .map(str::to_owned),
            outcome,
        }
    }
}
