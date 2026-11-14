//! Invite lifecycle under the same metadata fence as account creation.
use super::*;
use blake2::{Blake2s256, Digest};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 12;

const LIFETIME: u64 = 24 * 60 * 60;
const RETENTION: u64 = 7 * LIFETIME;
const MAX_INVITES: usize = 1024;
pub(crate) const INVALID_INVITE: &str = "This invite is invalid or no longer available.";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Invite {
    digest: String,
    issuer: String,
    pub(super) expires: u64,
    pub(super) redeemed_at: Option<u64>,
    pub(super) redeemed_by: Option<String>,
    revoked_at: Option<u64>,
}

pub(crate) fn expired(expires: u64) -> bool {
    now() >= expires
}

fn digest(secret: &str) -> String {
    format!("{:x}", Blake2s256::digest(secret.as_bytes()))
}

pub(crate) fn parse(code: &str) -> Option<(Option<String>, String)> {
    let code = code.trim();
    let Some((id, secret)) = code.split_once('.') else {
        let normalized = code.replace('-', "").to_ascii_uppercase();
        return (normalized.len() == CODE_LENGTH
            && normalized.bytes().all(|byte| ALPHABET.contains(&byte)))
        .then(|| (None, digest(&normalized)));
    };
    if id.len() != 32
        || secret.len() != 64
        || !id
            .bytes()
            .chain(secret.bytes())
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some((
        Some(id.to_ascii_lowercase()),
        digest(&secret.to_ascii_lowercase()),
    ))
}

impl Invite {
    fn active(&self) -> bool {
        self.redeemed_at.is_none() && self.revoked_at.is_none() && !expired(self.expires)
    }
}

impl Engine {
    pub(super) fn invite_hints(&self, viewer: &str) -> BTreeMap<String, u64> {
        let rights = self.effective(Some(viewer), &authorization::Scope::Server);
        if !rights.has(authorization::Action::ManageInvites) || !rights.command("/invite") {
            return BTreeMap::new();
        }
        self.data
            .invites
            .iter()
            .filter(|(_, invite)| invite.active())
            .map(|(id, invite)| (id.clone(), invite.expires))
            .collect()
    }

    pub(crate) fn check_registration_invite(
        &self,
        id: Option<&str>,
        digest: &str,
    ) -> Result<(), EngineError> {
        self.valid_invite(id, digest).map(|_| ())
    }

    pub(super) fn valid_invite(
        &self,
        id: Option<&str>,
        digest: &str,
    ) -> Result<(&str, &Invite), EngineError> {
        self.data
            .invites
            .iter()
            .find(|(key, invite)| {
                id.is_none_or(|id| id == key.as_str()) && invite.digest == digest && invite.active()
            })
            .map(|(id, invite)| (id.as_str(), invite))
            .ok_or_else(|| INVALID_INVITE.into())
    }

    pub(super) fn prepare_invite_command(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
        parts: &[&str],
    ) -> Result<(String, Option<metadata_write::PendingMetadata>), EngineError> {
        use authorization::{Action, Scope};

        crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
        self.require_command(actor, view, "/invite")?;
        self.require_target_command(actor, &Scope::Server, "/invite")?;
        self.require(actor, &Scope::Server, Action::ManageInvites)?;
        if parts == ["/invite", "ls"] {
            let result = self
                .data
                .invites
                .iter()
                .map(|(id, invite)| {
                    let status = if invite.redeemed_at.is_some() {
                        "used"
                    } else if invite.revoked_at.is_some() {
                        "revoked"
                    } else if expired(invite.expires) {
                        "expired"
                    } else {
                        "active"
                    };
                    format!("{id}  {status}  expires {}", invite.expires)
                })
                .collect::<Vec<_>>();
            let reply = if result.is_empty() {
                "No registration invites.".into()
            } else {
                format!(
                    "Invite ID / status / expiry (UTC Unix seconds)\n{}",
                    result.join("\n")
                )
            };
            return Ok((reply, None));
        }

        self.prepare_account_change(|engine| {
            let time = now();
            match parts {
                ["/invite", "new"] => {
                    let issuer = engine
                        .principal_id(actor)
                        .ok_or("Account unavailable.")?
                        .to_owned();
                    let stale: BTreeSet<_> = engine
                        .data
                        .invites
                        .iter()
                        .filter(|(_, invite)| invite.expires.saturating_add(RETENTION) <= time)
                        .map(|(id, _)| id.clone())
                        .collect();
                    if engine.data.invites.len() - stale.len() >= MAX_INVITES {
                        return Err("Registration invite limit reached.".into());
                    }
                    let id = uuid::Uuid::new_v4().simple().to_string();
                    let mut bytes = [0; CODE_LENGTH];
                    OsRng
                        .try_fill_bytes(&mut bytes)
                        .map_err(EngineError::unavailable)?;
                    let secret: String = bytes
                        .iter()
                        .map(|byte| char::from(ALPHABET[usize::from(*byte & 31)]))
                        .collect();
                    let digest = digest(&secret);
                    if engine
                        .data
                        .invites
                        .values()
                        .any(|invite| invite.digest == digest)
                    {
                        return Err(EngineError::unavailable(
                            "Could not issue an invite. Try again.",
                        ));
                    }
                    let code = format!("{}-{}-{}", &secret[..4], &secret[4..8], &secret[8..]);
                    let mut keys = changes::DirtyKeys {
                        invites: stale,
                        ..Default::default()
                    };
                    keys.invites.insert(id.clone());
                    let before = MetadataChange::keyed(&engine.data, keys.clone());
                    for key in keys.invites {
                        engine.data.invites.remove(&key);
                    }
                    engine.data.invites.insert(
                        id.clone(),
                        Invite {
                            issuer,
                            digest,
                            expires: time + LIFETIME,
                            redeemed_at: None,
                            redeemed_by: None,
                            revoked_at: None,
                        },
                    );
                    engine.audit(actor, "/invite new", &id);
                    Ok((
                        format!("Registration invite (expires in 24 hours)\nID: {id}\n{code}"),
                        before,
                    ))
                }
                ["/invite", "revoke", id] => {
                    let before = MetadataChange::keyed(
                        &engine.data,
                        changes::DirtyKeys {
                            invites: [(*id).into()].into(),
                            ..Default::default()
                        },
                    );
                    let invite = engine
                        .data
                        .invites
                        .get_mut(*id)
                        .filter(|invite| {
                            invite.expires > time
                                && invite.revoked_at.is_none()
                                && invite.redeemed_at.is_none()
                        })
                        .ok_or("No active registration invite with that ID.")?;
                    invite.revoked_at = Some(time);
                    engine.audit(actor, "/invite revoke", id);
                    Ok(("Registration invite revoked.".into(), before))
                }
                _ => Err("Usage: /invite new | ls | revoke <id>".into()),
            }
        })
        .map(|(reply, pending)| (reply, Some(pending)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn invite_completion_updates_only_authorized_viewers() {
        let mut engine = super::super::tests::engine();
        let before = engine.snapshot_stamp("alice", false).unwrap();
        let reply = engine.execute(None, None, "/invite new").unwrap();
        let id = reply.lines().nth(1).unwrap().strip_prefix("ID: ").unwrap();
        assert!(engine.snapshot_stamp("alice", false).unwrap() != before);
        assert!(
            engine
                .snapshot("alice")
                .unwrap()
                .invite_hints
                .contains_key(id)
        );
        assert!(engine.snapshot("bob").unwrap().invite_hints.is_empty());
        engine
            .execute(None, None, &format!("/invite revoke {id}"))
            .unwrap();
        assert!(engine.snapshot("alice").unwrap().invite_hints.is_empty());
    }

    #[test]
    fn legacy_invite_codes_remain_redeemable() {
        let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let secret = "ab".repeat(32);
        engine.data.invites.insert(
            id.into(),
            Invite {
                digest: digest(&secret),
                issuer: "console".into(),
                expires: now() + LIFETIME,
                redeemed_at: None,
                redeemed_by: None,
                revoked_at: None,
            },
        );
        let (id, digest) = parse(&format!("{id}.{secret}")).unwrap();
        assert!(
            engine
                .prepare_registration("legacy-user", "hash".into(), id.as_deref(), &digest)
                .is_ok()
        );
    }

    #[test]
    fn expired_and_failed_redemptions_leave_accounts_unchanged() {
        let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
        let reply = engine.execute(None, None, "/invite new").unwrap();
        let (_, digest) = parse(reply.lines().last().unwrap()).unwrap();
        let id = engine.data.invites.keys().next().unwrap().clone();
        let expected = engine.valid_invite(None, &digest).unwrap().1.expires;
        assert!((now() + LIFETIME - 1..=now() + LIFETIME).contains(&expected));

        let pending = engine
            .prepare_registration("new-user", "hash".into(), None, &digest)
            .unwrap()
            .check_invite_expiry(now());
        assert!(database::block_on_storage(pending.save()).unwrap().is_err());
        let stored = storage::data_from_snapshot(
            database::block_on_storage({
                let db = engine.store.db.clone();
                async move { db.recovery_snapshot().await }
            })
            .unwrap()
            .unwrap()
            .0,
        )
        .unwrap();
        assert!(!stored.users.contains_key("new-user"));
        assert_eq!(stored.invites[&id].redeemed_at, None);

        engine.data.invites.get_mut(&id).unwrap().expires = now();
        assert!(
            engine
                .prepare_registration("new-user", "hash".into(), None, &digest)
                .is_err()
        );
        assert!(!engine.data.users.contains_key("new-user"));
    }
}
