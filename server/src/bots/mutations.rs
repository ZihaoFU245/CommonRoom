//! Native async message sends with short engine preparation/publication guards.
use super::{
    auth::Context,
    error::{Code, Error},
    models,
};
use crate::{
    engine::{PendingMessage, database},
    web::{App, Change},
};
use axum::http::HeaderMap;
use serde_json::{Value, json};

pub(super) async fn message(
    app: App,
    headers: HeaderMap,
    input: models::MessageInput,
    key: String,
) -> Result<Value, Error> {
    let admission = app.io_permit().map_err(Error::storage)?;
    tokio::spawn(async move {
        let _admission = admission;
        if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::new(Code::Unavailable, "Server stopping."));
        }
        let context = app
            .inspect_engine(move |engine, _| Context::from_headers(engine, &headers))
            .await?;
        let permit = app.mutations.conversation(input.conversation_id.clone()).await?;
        let mut append: PendingMessage = app
            .with_conversation(&permit, {
                let context = context.clone();
                let input = input.clone();
                move |engine| {
                    let user = super::api::admit(engine, &context)?;
                    context.permits(engine, "send", crate::bots::auth::Scope::Conversation(&input.conversation_id))?;
                    engine.prepare_append(&user, &input)
                }
            })
            .await?;
        let digest = models::message_digest(&input)?;
        if let Some(replayed) = append.replay(&key, &digest).await? {
            app.with_conversation(&permit, move |engine| -> Result<_, Error> {
                context.user(engine).map(|_| ())
            })
            .await?;
            return Ok(json!({"message":models::message_json(&replayed.message),"replayed":replayed.replayed}));
        }
        append.resolve_reply(input.reply_to.as_deref()).await?;
        let media = append.media_records().await?;
        // Metadata is held stable by the permit, while eligibility is checked
        // again under native row locks in the message's transaction.
        if !media.is_empty() {
        append = app
            .with_conversation(&permit, {
                let context = context.clone();
                move |engine| -> Result<_, Error> {
                    context.user(engine)?;
                    engine.validate_append_media(&append, &media)?;
                    Ok(append)
                }
            })
            .await?;
        }
        let commit = append
            .save(Some((&key, &digest)))
            .await
            .map_err(|cause| match cause {
                database::Error::ReceiptConflict => Error::new(
                    Code::IdempotencyConflict,
                    "Key already used for another message.",
                ),
                database::Error::ReceiptCapacity => {
                    Error::new(Code::IdempotencyCapacity, "Wait for receipts to expire.")
                }
                cause => Error::from(crate::engine::EngineError::from(cause)),
            })?;
        let changes = app.changes.clone();
        let sent = app
            .with_conversation(&permit, move |engine| -> Result<_, Error> {
                let sent = engine.publish_message(append, commit)?;
                // Publish committed state even if a credential expired during I/O.
                let _ = changes.send(Change::Conversation(input.conversation_id));
                context.user(engine)?;
                Ok(sent)
            })
            .await?;
        Ok(json!({"message":models::message_json(&sent.message),"replayed":sent.replayed}))
    })
    .await
    .map_err(Error::storage)?
}

pub(super) async fn reaction(
    app: App,
    headers: HeaderMap,
    id: String,
    message_id: String,
    reaction: String,
    active: bool,
) -> Result<Value, Error> {
    let admission = app.io_permit().map_err(Error::storage)?;
    tokio::spawn(async move {
        let _admission = admission;
        if !models::valid_id(&message_id) {
            return Err(Error::new(Code::InvalidRequest, "Invalid message ID."));
        }
        if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::new(Code::Unavailable, "Server stopping."));
        }
        let context = app
            .inspect_engine(move |engine, _| Context::from_headers(engine, &headers))
            .await?;
        let permit = app.mutations.conversation(id.clone()).await?;
        let history = app
            .with_conversation(&permit, {
                let context = context.clone();
                let id = id.clone();
                move |engine| -> Result<_, Error> {
                    let user = super::api::admit(engine, &context)?;
                    context.permits(engine, "read", crate::bots::auth::Scope::Conversation(&id))?;
                    context.permits(
                        engine,
                        "react",
                        crate::bots::auth::Scope::Conversation(&id),
                    )?;
                    engine.bot_target(&user, &id, crate::engine::authorization::Action::Read)?;
                    Ok(engine.history_store().clone())
                }
            })
            .await?;
        let original = history
            .locate_async(&message_id)
            .await
            .map_err(Error::storage)?
            .filter(|(conversation, _)| conversation == &id)
            .map(|(_, message)| message)
            .ok_or_else(|| Error::new(Code::NotFound, "Message not found in this conversation."))?;
        let pending = app
            .with_conversation(&permit, {
                let context = context.clone();
                let id = id.clone();
                move |engine| {
                    let user = context.user(engine)?;
                    engine.prepare_reaction(&user, &id, original, &reaction, active)
                }
            })
            .await?;
        if let Some(pending) = pending {
            let commit = pending
                .save(None)
                .await
                .map_err(|cause| Error::from(crate::engine::EngineError::from(cause)))?;
            let changes = app.changes.clone();
            app.with_conversation(&permit, move |engine| -> Result<_, Error> {
                engine.publish_message(pending, commit)?;
                let _ = changes.send(Change::Conversation(id));
                context.user(engine).map(|_| ())
            })
            .await?;
        }
        Ok(json!({"ok":true}))
    })
    .await
    .map_err(Error::storage)?
}

pub(super) async fn read(
    app: App,
    headers: HeaderMap,
    id: String,
    through: u64,
    acknowledge: bool,
) -> Result<Value, Error> {
    let admission = app.io_permit().map_err(Error::storage)?;
    tokio::spawn(async move {
        let _admission = admission;
        if through > 9_007_199_254_740_991 {
            return Err(Error::new(Code::InvalidRequest, "Invalid sequence."));
        }
        let context = app
            .inspect_engine(move |engine, _| Context::from_headers(engine, &headers))
            .await?;
        let ordering = app.mutations.conversation(id.clone()).await?;
        let cursor: crate::engine::CursorWrite = app
            .with_conversation(&ordering, {
                let context = context.clone();
                move |engine| -> Result<_, Error> {
                    let user = super::api::admit(engine, &context)?;
                    if !acknowledge {
                        return Err(Error::new(
                            Code::InvalidRequest,
                            "Explicit shared unread acknowledgement required.",
                        ));
                    }
                    context.permits(engine, "read", crate::bots::auth::Scope::Conversation(&id))?;
                    context.permits(
                        engine,
                        "read_ack",
                        crate::bots::auth::Scope::Conversation(&id),
                    )?;
                    let target = engine.bot_target(
                        &user,
                        &id,
                        crate::engine::authorization::Action::Read,
                    )?;
                    Ok(engine.prepare_read(&user, &target.view, through)?)
                }
            })
            .await?;
        if !cursor.unchanged && !cursor.retained().await? {
            return Err(Error::new(
                Code::MessageGone,
                "Read position is no longer retained.",
            ));
        }
        cursor.save().await?;
        let changes = app.changes.clone();
        app.with_conversation(&ordering, move |engine| -> Result<_, Error> {
            let user = cursor.user.clone();
            if engine.publish_read(cursor)? {
                let _ = changes.send(Change::Read(user));
            }
            context.user(engine)?;
            Ok(json!({"ok":true}))
        })
        .await
    })
    .await
    .map_err(Error::storage)?
}
