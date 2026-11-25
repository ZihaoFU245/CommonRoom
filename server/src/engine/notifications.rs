use super::{
    authorization::{Action, Scope},
    *,
};
use crate::push::{Event, Subscription};

impl Engine {
    pub(crate) fn push_installed(&self, digest: &str) -> bool {
        self.data
            .sessions
            .get(digest)
            .is_some_and(|session| session.push.is_some())
    }

    pub(crate) fn push_sessions(&self) -> Vec<String> {
        let at = now();
        self.data
            .sessions
            .iter()
            .filter(|(_, session)| session.push.is_some() && session.expires > at)
            .map(|(digest, _)| digest.clone())
            .collect()
    }

    pub(crate) fn push_delivery(
        &self,
        digest: &str,
        event: &Event,
    ) -> Option<(Subscription, String)> {
        let runtime = self.push.as_ref()?;
        if runtime.connected(digest) || event.expired() {
            return None;
        }
        let session = self.data.sessions.get(digest)?;
        let subscription = session.push.as_ref()?;
        if session.expires <= now() {
            return None;
        }
        let view = self.notification_view(&session.username, event)?;
        Some((subscription.clone(), view))
    }

    pub(crate) fn notification_view(&self, user: &str, event: &Event) -> Option<String> {
        let account = self.data.users.get(user)?;
        let preferences = account.metadata.get("@global");
        let pref = |key: &str| {
            preferences
                .and_then(|values| values.get(key))
                .map(String::as_str)
        };
        let muted = pref("preferences.notifications_muted").map_or_else(
            || pref("preferences.notifications") == Some("false"),
            |value| value == "true",
        );
        if account.disabled || account.id == event.author || event.expired() || muted {
            return None;
        }
        let reply_author = event.reply_author.as_deref().filter(|id| !id.is_empty());
        let replied = reply_author == Some(account.id.as_str());
        let target = self.bot_index.conversations.get(&event.conversation)?;
        let (view, cursor, scope, room_wide) = if event.private {
            let key = target.strip_prefix("@private:")?;
            let peer = helpers::private_peer(key, user)?;
            // Only the recipient of this append can get a direct notification.
            if self.data.users.get(peer)?.id != event.author {
                return None;
            }
            (
                format!("@direct:{peer}"),
                format!("dm:{key}"),
                Scope::Private(event.conversation.clone()),
                false,
            )
        } else {
            let name = target;
            let room = self.data.rooms.get(name)?;
            (
                name.clone(),
                format!("room:{name}"),
                Scope::Room(event.conversation.clone()),
                event.mention_all && room.members.contains(user),
            )
        };
        if !self.allows(Some(user), &scope, Action::Read)
            || self.read_position(user, &cursor) >= event.sequence
        {
            return None;
        }
        let mentioned = event.mentions.contains(&account.id);
        let matches = match pref(&format!("preferences.notification.{}", event.conversation))
            .or_else(|| pref("preferences.notification.global"))
        {
            None => {
                let enabled = |key| {
                    pref(&format!("{key}.{}", event.conversation)).or_else(|| pref(key))
                        != Some("false")
                };
                let reply_matches = replied && enabled("preferences.notification_default_replies");
                if event.bot_author && reply_author.is_some() {
                    reply_matches
                } else {
                    let bot_excluded = event.bot_author
                        && enabled("preferences.notification_exclude_bots")
                        || event.bot_mention
                            && enabled("preferences.notification_exclude_bot_mentions");
                    !bot_excluded
                        && (reply_matches
                            || event.private && enabled("preferences.notification_default_direct")
                            || room_wide
                                && enabled("preferences.notification_default_mentions_all")
                            || mentioned && enabled("preferences.notification_default_mentions_me"))
                }
            }

            Some("mentions") => room_wide || mentioned,
            Some("off") => false,
            Some(rule) => notification_filter::parse(rule).is_ok_and(|rules| {
                notification_filter::matches(
                    &rules,
                    &notification_filter::Facts {
                        author: &event.author_name,
                        has_role: &|role| self.authorization.has_role(&event.author, &scope, role),
                        mentions_me: mentioned,
                        mentions_all: room_wide,
                        replies_to_me: replied,
                        contains_media: &|| {
                            *event
                                .media
                                .get_or_init(|| !media::media_references(&event.text).is_empty())
                        },
                        contains_keyword: &|keyword| event.text.contains(keyword),
                    },
                )
            }),
        };
        matches.then_some(view)
    }
}
