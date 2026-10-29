//! Native profile writes keep account ordering through committed publication.
use super::media::Access;
use super::*;

pub(super) enum Update {
    Avatar(Option<String>),
    Color {
        room: Option<String>,
        value: Option<String>,
    },
    Complete(
        crate::engine::media::Media,
        tokio::sync::OwnedMutexGuard<()>,
    ),
}
pub(super) async fn update(app: &App, access: Access, update: Update) -> Result<bool, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let app = app.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let (identity, store) = super::media::native_context(&app, access).await?;
        let ordering = app
            .mutations
            .conversation(format!("account:{}", identity.account))
            .await?;
        let color = matches!(&update, Update::Color { .. });
        let current = if let Update::Complete(media, _) = &update {
            Some(store.load(&media.id).await?.0)
        } else {
            None
        };
        // The accepted worker owns the object lock through commit/publication,
        // including when its HTTP caller is cancelled.
        let (pending, _object_lock) = app
            .with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, ApiError> {
                    super::media::verify_identity(&identity, engine)?;
                    if let Update::Color {
                        room: Some(room), ..
                    } = &update
                    {
                        identity.permit_conversation(engine, room)?;
                    }
                    Ok(match update {
                        Update::Avatar(value) => (
                            engine.prepare_avatar_set(&identity.user, value.as_deref())?,
                            None,
                        ),
                        Update::Color { room, value } => (
                            engine.prepare_color_set(
                                &identity.user,
                                room.as_deref(),
                                value.as_deref(),
                            )?,
                            None,
                        ),
                        Update::Complete(media, lock) => (
                            engine.prepare_avatar_completion(
                                &identity.user,
                                media,
                                current.as_ref().ok_or_else(|| {
                                    error(StatusCode::SERVICE_UNAVAILABLE, "Missing avatar state.")
                                })?,
                            )?,
                            Some(lock),
                        ),
                    })
                }
            })
            .await?;
        let Some(pending) = pending else {
            store.health().await?;
            return app
                .with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
                    super::media::verify_identity(&identity, engine)?;
                    Ok(false)
                })
                .await;
        };
        if pending.changes_profile() {
            app.admission.profile_change(&identity.account)?;
        }
        pending.save().await?;
        let changes = app.changes.clone();
        app.with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
            let changed = engine.publish_profile(pending)?;
            if changed {
                let change = if color {
                    Change::Metadata(engine.metadata_change())
                } else {
                    Change::Avatar(identity.account.clone())
                };
                let _ = changes.send(change);
            }
            super::media::verify_identity(&identity, engine)?;
            Ok(changed)
        })
        .await
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Profile worker unavailable.",
        )
    })?
}
