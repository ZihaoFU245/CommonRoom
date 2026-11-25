use super::{EngineError, authorization::Group};

const MAX_RULES: usize = 16;
const MAX_BYTES: usize = 1024;

#[derive(PartialEq, Eq)]
pub(super) enum Actor<'a> {
    Is(&'a str),
    IsNot(&'a str),
    Role(Group),
}

#[derive(PartialEq, Eq)]
pub(super) enum Match<'a> {
    All,
    None,
    Actor(Actor<'a>),
    MentionMe,
    MentionAll,
    ReplyToMe,
    Media,
    Keyword(&'a str),
    ActorKeyword(Actor<'a>, &'a str),
}

impl<'a> Match<'a> {
    fn parse(rule: &'a str) -> Option<Self> {
        Some(match rule {
            "all" => Self::All,
            "none" => Self::None,
            "mentions_me" => Self::MentionMe,
            "mentions_all" | "@all" => Self::MentionAll,
            "is_reply_to_me" => Self::ReplyToMe,
            "contains_media" => Self::Media,
            other => {
                if let Some(keyword) = other.strip_prefix("contains_keyword:") {
                    return keyword_value(keyword).map(Self::Keyword);
                }
                let (actor, keyword) = other
                    .split_once(" contains_keyword:")
                    .map_or((other, None), |(actor, keyword)| (actor, Some(keyword)));
                let (predicate, value) = actor.split_once(':')?;
                let actor = match predicate {
                    "user_is" if super::valid_name(value) => Actor::Is(value),
                    "user_not" if super::valid_name(value) => Actor::IsNot(value),
                    "user_role" => Actor::Role(Group::parse(value).ok()?),
                    _ => return None,
                };
                match keyword {
                    Some(keyword) => Self::ActorKeyword(actor, keyword_value(keyword)?),
                    None => Self::Actor(actor),
                }
            }
        })
    }
}

fn keyword_value(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= 100 && !value.chars().any(char::is_control))
        .then_some(value)
}

pub(super) fn parse(value: &str) -> Result<Vec<Match<'_>>, EngineError> {
    let invalid = || {
        EngineError::Invalid(
            "Use up to 16 notification rules separated by semicolons. See /man notifications."
                .into(),
        )
    };
    if value.len() > MAX_BYTES {
        return Err(invalid());
    }
    let mut rules = Vec::new();
    for rule in value.split(';') {
        if rules.len() == MAX_RULES {
            return Err(invalid());
        }
        rules.push(Match::parse(rule.trim()).ok_or_else(invalid)?);
    }
    if rules.len() > 1
        && rules
            .iter()
            .any(|rule| matches!(rule, Match::All | Match::None))
    {
        return Err(EngineError::Invalid(
            "Use all or none as a standalone notification rule.".into(),
        ));
    }
    Ok(rules)
}

pub(super) struct Facts<'a> {
    pub(super) author: &'a str,
    pub(super) has_role: &'a dyn Fn(Group) -> bool,
    pub(super) mentions_me: bool,
    pub(super) mentions_all: bool,
    pub(super) replies_to_me: bool,
    pub(super) contains_media: &'a dyn Fn() -> bool,
    pub(super) contains_keyword: &'a dyn Fn(&str) -> bool,
}

pub(super) fn matches(rules: &[Match<'_>], facts: &Facts<'_>) -> bool {
    let actor_matches = |actor: &Actor<'_>| match actor {
        Actor::Is(user) => facts.author == *user,
        Actor::IsNot(user) => facts.author != *user,
        Actor::Role(role) => (facts.has_role)(*role),
    };
    let mut keywords = Vec::new();
    let mut check_media = false;
    for rule in rules {
        let matched = match rule {
            Match::All => true,
            Match::None => false,
            Match::Actor(actor) => actor_matches(actor),
            Match::MentionMe => facts.mentions_me,
            Match::MentionAll => facts.mentions_all,
            Match::ReplyToMe => facts.replies_to_me,
            Match::Media => {
                check_media = true;
                false
            }
            Match::Keyword(keyword) => {
                keywords.push(*keyword);
                false
            }
            Match::ActorKeyword(actor, keyword) => {
                if actor_matches(actor) {
                    keywords.push(*keyword);
                }
                false
            }
        };
        if matched {
            return true;
        }
    }
    (check_media && (facts.contains_media)()) || keywords.into_iter().any(facts.contains_keyword)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn rule_union_checks_cheap_matches_and_actor_guards_before_content() {
        let scans = Cell::new(0);
        let facts = Facts {
            author: "alice",
            has_role: &|role| role == Group::Admin,
            mentions_me: true,
            mentions_all: false,
            replies_to_me: true,
            contains_media: &|| false,
            contains_keyword: &|keyword| {
                scans.set(scans.get() + 1);
                keyword == "incident"
            },
        };
        for rule in [
            "contains_keyword:incident; mentions_me",
            "user_role:admin",
            "is_reply_to_me",
            "user_not:bob",
        ] {
            assert!(matches(&parse(rule).unwrap(), &facts));
        }
        assert!(!matches(
            &parse("user_is:bob contains_keyword:incident").unwrap(),
            &facts
        ));
        assert_eq!(scans.get(), 0);
        assert!(matches(
            &parse("user_is:alice contains_keyword:incident").unwrap(),
            &facts
        ));
        assert_eq!(scans.get(), 1);
        for invalid in [
            "none; mentions_me",
            "user_role:owner",
            "contains_keyword:",
            "",
        ] {
            assert!(parse(invalid).is_err());
        }
    }
}
