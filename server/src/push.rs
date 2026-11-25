//! Web Push transport. Push endpoints are bearer capabilities and are never logged.
use crate::{engine::EngineError, web::App};
use axum::http::{Request, Uri, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use web_push_native::{
    Auth, WebPushBuilder,
    jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair},
    p256::PublicKey,
    vapid::VapidSignature,
};

const TTL: Duration = Duration::from_secs(120);
const VAPID_VALIDITY: Duration = Duration::from_secs(12 * 60 * 60);
const VAPID_REFRESH: Duration = Duration::from_secs(11 * 60 * 60);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Subscription {
    pub(crate) endpoint: String,
    keys: Keys,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Keys {
    p256dh: String,
    auth: String,
}

impl Subscription {
    pub(crate) fn valid(&self) -> bool {
        self.builder().is_some()
    }

    fn builder(&self) -> Option<WebPushBuilder> {
        if self.endpoint.len() > 4096 || self.keys.p256dh.len() != 87 || self.keys.auth.len() != 22
        {
            return None;
        }
        let endpoint = url::Url::parse(&self.endpoint).ok()?;
        let host = endpoint.host_str()?;
        // The server performs an outbound request. Restrict it to supported
        // providers and disable redirects on the HTTP client.
        let chromium_staging = host == "jmt17.google.com"
            && endpoint.query().is_none()
            && endpoint
                .path()
                .strip_prefix("/fcm/send/")
                .is_some_and(|token| !token.is_empty() && !token.contains('/'));
        if endpoint.scheme() != "https"
            || endpoint.port().is_some()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
            || !(matches!(
                host,
                "fcm.googleapis.com" | "updates.push.services.mozilla.com"
            ) || host.ends_with(".push.apple.com")
                || chromium_staging)
        {
            return None;
        }
        let public_key = URL_SAFE_NO_PAD.decode(&self.keys.p256dh).ok()?;
        let auth: [u8; 16] = URL_SAFE_NO_PAD
            .decode(&self.keys.auth)
            .ok()?
            .try_into()
            .ok()?;
        Some(WebPushBuilder::new(
            endpoint.as_str().parse().ok()?,
            PublicKey::from_sec1_bytes(&public_key).ok()?,
            Auth::from(auth),
        ))
    }
}

pub(crate) struct Event {
    pub(crate) conversation: String,
    pub(crate) private: bool,
    pub(crate) id: String,
    pub(crate) author: String,
    pub(crate) author_name: String,
    pub(crate) reply_author: Option<String>,
    pub(crate) text: String,
    pub(crate) media: std::sync::OnceLock<bool>,
    pub(crate) sequence: u64,
    pub(crate) mentions: BTreeSet<String>,
    pub(crate) mention_all: bool,
    pub(crate) bot_author: bool,
    pub(crate) bot_mention: bool,
    pub(crate) queued: Instant,
}
impl Event {
    pub(crate) fn expired(&self) -> bool {
        self.queued.elapsed() >= TTL
    }

    pub(crate) fn preview(&self) -> String {
        let mut text = self.text.trim().chars();
        let mut preview: String = text.by_ref().take(200).collect();
        if text.next().is_some() {
            preview.push('…');
        }
        preview
    }
}

#[derive(Clone)]
pub(crate) struct Runtime {
    pub(crate) public_key: Option<String>,
    live: tokio::sync::broadcast::Sender<Arc<Event>>,
    queue: mpsc::Sender<Arc<Event>>,
    sockets: Arc<Mutex<BTreeMap<String, usize>>>,
}
impl Runtime {
    pub(crate) fn new(public_key: Option<String>) -> (Self, mpsc::Receiver<Arc<Event>>) {
        let (queue, events) = mpsc::channel(256);
        (
            Self {
                public_key,
                live: tokio::sync::broadcast::channel(128).0,
                queue,
                sockets: Default::default(),
            },
            events,
        )
    }

    pub(crate) fn enqueue(&self, event: Event) {
        let event = Arc::new(event);
        let _ = self.live.send(event.clone());
        if self.public_key.is_some()
            && let Err(mpsc::error::TrySendError::Full(_)) = self.queue.try_send(event)
        {
            tracing::warn!("Web Push queue full");
        }
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Arc<Event>> {
        self.live.subscribe()
    }

    pub(crate) fn open_socket(&self, digest: String) -> Option<Socket> {
        *self.sockets.lock().ok()?.entry(digest.clone()).or_default() += 1;
        Some(Socket {
            sockets: self.sockets.clone(),
            digest,
        })
    }

    pub(crate) fn connected(&self, digest: &str) -> bool {
        self.sockets
            .lock()
            .map_or(true, |sockets| sockets.contains_key(digest))
    }
}

pub(crate) struct Socket {
    sockets: Arc<Mutex<BTreeMap<String, usize>>>,
    digest: String,
}
impl Drop for Socket {
    fn drop(&mut self) {
        if let Ok(mut sockets) = self.sockets.lock()
            && let Some(count) = sockets.get_mut(&self.digest)
        {
            *count -= 1;
            if *count == 0 {
                sockets.remove(&self.digest);
            }
        }
    }
}

pub(crate) struct Sender {
    client: reqwest::Client,
    key: ES256KeyPair,
    subject: String,
    authorization: Mutex<BTreeMap<String, (Instant, header::HeaderValue)>>,
}

impl Sender {
    pub(crate) fn from_environment() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let private = match std::env::var("CHAT_VAPID_PRIVATE_KEY") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let key = URL_SAFE_NO_PAD
            .decode(private)
            .ok()
            .filter(|key| key.len() == 32)
            .and_then(|key| ES256KeyPair::from_bytes(&key).ok())
            .ok_or("CHAT_VAPID_PRIVATE_KEY must be a base64url-encoded P-256 private scalar.")?;
        let subject = std::env::var("CHAT_VAPID_SUBJECT")?;
        let contact = url::Url::parse(&subject)?;
        if !(contact.scheme() == "mailto" && contact.path().contains('@')
            || contact.scheme() == "https"
                && contact.host_str().is_some_and(|host| host != "localhost"))
        {
            return Err(
                "CHAT_VAPID_SUBJECT must be a mailto address or a public HTTPS URL.".into(),
            );
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .build()?;
        Ok(Some(Self {
            client,
            key,
            subject,
            authorization: Default::default(),
        }))
    }

    pub(crate) fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.public_key().public_key().to_bytes_uncompressed())
    }

    fn authorization(&self, endpoint: &Uri) -> Option<header::HeaderValue> {
        // Endpoints are HTTPS on the default port, so the host identifies the audience.
        let host = endpoint.host()?;
        let mut cache = self.authorization.lock().ok()?;
        if let Some((created, authorization)) = cache.get(host)
            && created.elapsed() < VAPID_REFRESH
        {
            return Some(authorization.clone());
        }
        cache.retain(|_, (created, _)| created.elapsed() < VAPID_REFRESH);
        if cache.len() >= 64 {
            return None;
        }
        // APNs expects the same token to be reused for at least an hour.
        let signature =
            VapidSignature::sign(endpoint, VAPID_VALIDITY, &self.subject, &self.key).ok()?;
        let mut value = header::HeaderValue::from(signature);
        value.set_sensitive(true);
        cache.insert(host.to_owned(), (Instant::now(), value.clone()));
        Some(value)
    }

    fn request(
        &self,
        builder: WebPushBuilder,
        event: &Event,
        view: String,
    ) -> Option<Request<Vec<u8>>> {
        let remaining = TTL.saturating_sub(event.queued.elapsed());
        if remaining.as_secs() == 0 {
            return None;
        }
        let title = if event.private {
            event.author_name.clone()
        } else {
            format!("{} · #{view}", event.author_name)
        };
        let payload = serde_json::json!({
            "id": event.id,
            "view": view,
            "title": title,
            "body": event.preview(),
        })
        .to_string();
        let mut request = builder.with_valid_duration(remaining).build(payload).ok()?;
        let authorization = self.authorization(request.uri())?;
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, authorization);
        Some(request)
    }

    async fn send(&self, subscription: &Subscription, event: &Event, view: String) -> bool {
        let Some(builder) = subscription.builder() else {
            return true;
        };
        let Some(request) = self.request(builder, event, view) else {
            tracing::warn!("Could not prepare a Web Push notification");
            return false;
        };
        let (parts, body) = request.into_parts();
        match self
            .client
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .body(body)
            .send()
            .await
        {
            Ok(response) => match response.status().as_u16() {
                404 | 410 => true,
                200..=299 => false,
                status => {
                    tracing::warn!(status, "Web Push provider rejected a notification");
                    false
                }
            },
            Err(_) => {
                tracing::warn!("Web Push provider request failed");
                false
            }
        }
    }
}

pub(crate) fn start(app: App, sender: Sender, mut events: mpsc::Receiver<Arc<Event>>) {
    tokio::spawn(async move {
        loop {
            let notified = app.shutdown.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let event = tokio::select! {
                _ = &mut notified => break,
                event = events.recv() => event,
            };
            let Some(event) = event else { break };
            if event.expired() {
                continue;
            }
            let Ok(sessions) = app
                .inspect_engine(|engine, _| Ok::<_, EngineError>(engine.push_sessions()))
                .await
            else {
                continue;
            };
            tokio::select! {
                _ = &mut notified => break,
                _ = stream::iter(sessions)
                    .for_each_concurrent(8, |digest| deliver(&app, &sender, event.clone(), digest)) => {}
            }
        }
    });
}

async fn delivery_target(
    app: &App,
    event: &Arc<Event>,
    digest: &str,
) -> Option<(Subscription, String)> {
    let (delivery, history) = app
        .inspect_engine({
            let event = event.clone();
            let digest = digest.to_owned();
            move |engine, _| {
                Ok::<_, EngineError>(
                    engine
                        .push_delivery(&digest, &event)
                        .map(|delivery| (delivery, engine.history_store().clone())),
                )
            }
        })
        .await
        .ok()??;
    // History can change while the event waits for the sender. Check the
    // indexed message sequence in storage before handing content to a provider.
    history
        .contains_sequence_async(&event.conversation, event.sequence)
        .await
        .ok()?
        .then_some(delivery)
}

async fn deliver(app: &App, sender: &Sender, event: Arc<Event>, digest: String) {
    let Some((subscription, view)) = delivery_target(app, &event, &digest).await else {
        return;
    };
    if sender.send(&subscription, &event, view).await
        && let Err(error) = crate::web::notifications::clear_gone(app, digest, subscription).await
    {
        tracing::warn!(%error, "Could not remove an expired Web Push subscription");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use web_push_native::p256::{SecretKey, elliptic_curve::sec1::ToEncodedPoint};

    #[tokio::test]
    async fn queued_push_is_dropped_after_retraction() {
        use crate::engine::{Engine, session_digest};

        let app =
            crate::web::media::tests::app(Engine::open(std::path::Path::new(":memory:")).unwrap());
        let (runtime, mut events) = Runtime::new(Some("test-key".into()));
        let digest = session_digest("bob");
        {
            let mut engine = app.engine().unwrap();
            engine.push = Some(runtime);
            engine
                .execute(Some("alice"), None, "/room new lobby")
                .unwrap();
            engine
                .execute(Some("alice"), None, "/room add bob #lobby")
                .unwrap();
        }
        let subscription = Subscription {
            endpoint: "https://web.push.apple.com/test".into(),
            keys: Keys {
                p256dh: String::new(),
                auth: String::new(),
            },
        };
        let pending = app
            .engine_read()
            .unwrap()
            .prepare_push_subscription(&digest, Some(subscription), None)
            .unwrap();
        let pending = pending.save().await.unwrap();
        {
            let mut engine = app.engine().unwrap();
            engine.publish_sessions(pending).unwrap();
            engine
                .execute(Some("alice"), Some("lobby"), "hello @bob")
                .unwrap();
        }
        let event = events.try_recv().unwrap();
        assert!(delivery_target(&app, &event, &digest).await.is_some());
        app.engine()
            .unwrap()
            .execute(
                Some("alice"),
                Some("lobby"),
                &format!("/retract {}", event.id),
            )
            .unwrap();
        assert!(delivery_target(&app, &event, &digest).await.is_none());
    }

    #[test]
    fn web_push_request_encrypts_payload_and_reuses_vapid_for_the_same_provider() {
        let sender = Sender {
            client: reqwest::Client::new(),
            key: ES256KeyPair::from_bytes(&[1; 32]).unwrap(),
            subject: "mailto:operator@example.com".into(),
            authorization: Default::default(),
        };
        let private = SecretKey::from_slice(&[2; 32]).unwrap();
        let ua_auth = [3; 16];
        let mut subscription = Subscription {
            endpoint: "https://web.push.apple.com/test".into(),
            keys: Keys {
                p256dh: URL_SAFE_NO_PAD.encode(private.public_key().to_encoded_point(false)),
                auth: URL_SAFE_NO_PAD.encode(ua_auth),
            },
        };
        let event = Event {
            conversation: "room-id".into(),
            private: false,
            id: "message-id".into(),
            author: "author-id".into(),
            author_name: "author".into(),
            reply_author: None,
            text: format!("Hello there! {}", "🦀".repeat(250)),
            media: Default::default(),
            sequence: 1,
            mentions: BTreeSet::new(),
            mention_all: false,
            bot_author: false,
            bot_mention: false,
            queued: Instant::now(),
        };
        let first = sender
            .request(subscription.builder().unwrap(), &event, "lobby".into())
            .unwrap();
        let auth = first.headers()[header::AUTHORIZATION].clone();
        assert!(
            auth.to_str()
                .unwrap()
                .ends_with(&format!(", k={}", sender.public_key()))
        );
        assert_eq!(first.headers()[header::CONTENT_ENCODING], "aes128gcm");
        let payload: serde_json::Value = serde_json::from_slice(
            &web_push_native::decrypt(first.into_body(), &private, &Auth::from(ua_auth)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            payload,
            serde_json::json!({
                "id": "message-id",
                "view": "lobby",
                "title": "author · #lobby",
                "body": format!("Hello there! {}…", "🦀".repeat(187)),
            })
        );

        subscription.endpoint = "https://web.push.apple.com/another-endpoint".into();
        let same = sender
            .request(subscription.builder().unwrap(), &event, "other".into())
            .unwrap();
        assert_eq!(same.headers()[header::AUTHORIZATION], auth);
        subscription.endpoint = "https://fcm.googleapis.com/test".into();
        let other = sender
            .request(subscription.builder().unwrap(), &event, "lobby".into())
            .unwrap();
        assert_ne!(other.headers()[header::AUTHORIZATION], auth);
    }
}
