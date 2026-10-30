//! Shared credential capture/health/revalidation, with request-local reuse.
use super::{App, auth::Identity};
use crate::engine::{AuthStore, Engine, EngineError};
use axum::http::HeaderMap;

#[derive(Clone)]
pub(crate) enum Principal {
    Browser(Identity),
    Bot(crate::bots::auth::Context),
}
impl Principal {
    pub(crate) fn account(&self) -> &str {
        match self {
            Self::Browser(identity) => &identity.account,
            Self::Bot(context) => &context.account,
        }
    }
}
#[derive(Clone)]
pub(crate) struct Resolved {
    app: App,
    headers: HeaderMap,
    pub(crate) principal: Principal,
    pub(crate) store: AuthStore,
}
impl Resolved {
    pub(crate) fn capture(app: &App, engine: &Engine, headers: HeaderMap) -> Option<Self> {
        let principal = if headers.contains_key(axum::http::header::AUTHORIZATION) {
            Principal::Bot(crate::bots::auth::Context::from_headers(engine, &headers).ok()?)
        } else {
            Principal::Browser(Identity::capture(
                engine,
                app.config.clone(),
                headers.clone(),
            )?)
        };
        Some(Self {
            app: app.clone(),
            headers,
            principal,
            store: engine.auth_store(),
        })
    }
    pub(crate) fn matches(&self, app: &App, headers: &HeaderMap) -> bool {
        std::sync::Arc::ptr_eq(&self.app.engine, &app.engine)
            && std::sync::Arc::ptr_eq(&self.app.config, &app.config)
            && self.headers == *headers
    }
}

/// Reuse capture only. Every caller still checks native ownership and verifies
/// live identity after the await; cached authority never authorizes delivery.
pub(crate) async fn authenticate<T, E, R>(
    app: &App,
    cached: Option<(T, AuthStore)>,
    capture: impl FnOnce(&Engine) -> Result<T, E> + Send + 'static,
    verify: impl FnOnce(&T, &Engine) -> Result<R, E> + Send + 'static,
) -> Result<(T, R), E>
where
    T: Clone + Send + 'static,
    E: From<EngineError> + Send + 'static,
    R: Send + 'static,
{
    let (principal, store) = match cached {
        Some(cached) => cached,
        None => {
            app.inspect_engine(move |engine, _| {
                capture(engine).map(|principal| (principal, engine.auth_store()))
            })
            .await?
        }
    };
    store.health().await.map_err(E::from)?;
    let verified = app
        .inspect_engine({
            let principal = principal.clone();
            move |engine, _| verify(&principal, engine)
        })
        .await?;
    Ok((principal, verified))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reused_capture_still_rejects_revocation_and_cannot_cross_headers_or_engines() {
        let app =
            crate::web::media::tests::app(Engine::open(std::path::Path::new(":memory:")).unwrap());
        let mut headers = HeaderMap::new();
        headers.insert("cookie", "chat_session=alice".parse().unwrap());
        let resolved = app
            .inspect_engine({
                let app = app.clone();
                let headers = headers.clone();
                move |engine, _| {
                    Ok::<_, EngineError>(Resolved::capture(&app, engine, headers).unwrap())
                }
            })
            .await
            .unwrap();
        assert!(resolved.matches(&app, &headers));
        let other_app = crate::web::tests::login_test_app();
        assert!(!resolved.matches(&other_app, &headers));
        let mut other_headers = headers.clone();
        other_headers.insert("cookie", "chat_session=bob".parse().unwrap());
        assert!(!resolved.matches(&app, &other_headers));
        let identity = match resolved.principal {
            Principal::Browser(identity) => identity,
            Principal::Bot(_) => panic!("Expected browser principal"),
        };
        let cached = (identity, resolved.store);
        let accepted = authenticate(
            &app,
            Some(cached.clone()),
            |_| Err(EngineError::Internal("Unexpected recapture".into())),
            Identity::verify,
        )
        .await;
        assert!(accepted.is_ok());
        app.engine().unwrap().logout("alice").unwrap();
        let denied = authenticate(
            &app,
            Some(cached),
            |_| Err(EngineError::Internal("Unexpected recapture".into())),
            Identity::verify,
        )
        .await;
        assert!(matches!(denied, Err(EngineError::Invalid(_))));
    }
}
