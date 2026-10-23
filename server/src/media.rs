//! S3 transport only. Authorization and SQL belong to the engine.
mod types;
use s3::{Bucket, Region, creds::Credentials};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

/// Client display hints only. Never use these headers for authorization, type,
/// object identity, expiry, quota, or response headers.
pub(crate) fn hint_headers(
    hints: &std::collections::BTreeMap<String, String>,
) -> Result<axum::http::HeaderMap, String> {
    let mut headers = axum::http::HeaderMap::new();
    if hints.len() > 4 {
        return Err("At most four upload hints are allowed.".into());
    }
    for (key, value) in hints {
        if !["filename", "content-type", "label", "client"].contains(&key.as_str())
            || value.is_empty()
            || value.len() > 256
            || !value.bytes().all(|c| (32..=126).contains(&c))
            || value.trim() != value
        {
            return Err("Upload hints require known keys and 1–256 printable ASCII bytes without surrounding spaces.".into());
        }
        let name =
            axum::http::HeaderName::from_bytes(format!("x-amz-meta-client-{key}").as_bytes())
                .map_err(|_| "Invalid hint name.")?;
        let value = axum::http::HeaderValue::from_str(value).map_err(|_| "Invalid hint value.")?;
        headers.insert(name, value);
    }
    Ok(headers)
}

pub(crate) struct MediaStorage {
    pub bucket: Box<Bucket>,
    pub public: Box<Bucket>,
    pub public_origin: String,
    // Locks exist only while requests are active; unrelated objects never serialize.
    locks: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    jobs: tokio::sync::Semaphore,
    probe_gate: tokio::sync::Mutex<()>,
    probe_result: Mutex<Option<(std::time::Instant, Result<(), String>)>>,
}

#[derive(Debug)]
pub(crate) enum StorageError {
    Invalid(&'static str),
    Unavailable(&'static str),
}
impl StorageError {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Invalid(message) | Self::Unavailable(message) => message,
        }
    }
}

impl MediaStorage {
    pub(crate) fn lock_media(&self, id: &str) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
        let mut locks = self.locks.lock().map_err(|_| "Media locks unavailable.")?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        let lock = locks.get(id).and_then(Weak::upgrade).unwrap_or_else(|| {
            let lock = Arc::new(tokio::sync::Mutex::new(()));
            locks.insert(id.into(), Arc::downgrade(&lock));
            lock
        });
        lock.try_lock_owned()
            .map_err(|_| "This attachment is busy; retry shortly.".into())
    }
    fn work(&self) -> Result<tokio::sync::SemaphorePermit<'_>, StorageError> {
        self.jobs
            .try_acquire()
            .map_err(|_| StorageError::Unavailable("Media storage busy; retry shortly."))
    }
    pub fn from_config(
        production: bool,
        saved: Option<&crate::config::S3Config>,
    ) -> Result<Option<Arc<Self>>, String> {
        let endpoint = std::env::var("CHAT_S3_ENDPOINT")
            .ok()
            .or_else(|| saved.map(|c| c.endpoint.clone()));
        let Some(endpoint) = endpoint else {
            return Ok(None);
        };
        let required = |name| std::env::var(name).map_err(|_| format!("Missing {name}."));
        let public = std::env::var("CHAT_S3_PUBLIC_ENDPOINT")
            .ok()
            .or_else(|| saved.and_then(|c| c.public_endpoint.clone()))
            .unwrap_or_else(|| endpoint.clone());
        // Parse with the same URL implementation used by the transport.
        let public_url = url::Url::parse(&public).map_err(|_| "Invalid media public endpoint.")?;
        if !matches!(public_url.scheme(), "https" | "http")
            || public_url.host_str().is_none()
            || !public_url.username().is_empty()
            || public_url.password().is_some()
            || public_url.query().is_some()
            || public_url.fragment().is_some()
            || public_url.path() != "/"
            || (production && public_url.scheme() != "https")
        {
            return Err(
                "Media public endpoint must be an HTTPS origin (HTTP allowed in development)."
                    .into(),
            );
        }
        let access = required("CHAT_S3_ACCESS_KEY")?;
        let secret = required("CHAT_S3_SECRET_KEY")?;
        let name = std::env::var("CHAT_S3_BUCKET")
            .ok()
            .or_else(|| saved.map(|c| c.bucket.clone()))
            .ok_or("Missing CHAT_S3_BUCKET or s3.bucket.")?;
        if name.is_empty() || name.contains(['/', '?', '#']) {
            return Err("Invalid S3 bucket.".into());
        }
        let region = std::env::var("CHAT_S3_REGION")
            .ok()
            .or_else(|| saved.map(|c| c.region.clone()))
            .unwrap_or_else(|| "us-east-1".into());
        let token = std::env::var("CHAT_S3_SESSION_TOKEN").ok();
        let path_style = match std::env::var("CHAT_S3_PATH_STYLE").ok().as_deref() {
            Some("true") => true,
            Some("false") => false,
            Some(_) => return Err("CHAT_S3_PATH_STYLE must be true or false.".into()),
            None => saved.is_none_or(|c| c.path_style),
        };
        let internal = url::Url::parse(&endpoint).map_err(|_| "Invalid S3 endpoint.")?;
        if !matches!(internal.scheme(), "http" | "https")
            || internal.host_str().is_none()
            || internal.path() != "/"
            || !internal.username().is_empty()
            || internal.password().is_some()
            || internal.query().is_some()
            || internal.fragment().is_some()
        {
            return Err("S3 endpoint must be an HTTP/HTTPS origin.".into());
        }
        let make = |endpoint: String| {
            let credentials =
                Credentials::new(Some(&access), Some(&secret), None, token.as_deref(), None)
                    .map_err(|_| "Invalid storage credentials.".to_string())?;
            s3_bucket(&name, &region, endpoint, path_style, credentials)
        };
        let bucket = make(endpoint)?;
        let public = make(public)?;
        // Virtual-host addressing changes the browser origin to bucket.endpoint.
        let public_origin = bucket_origin(&public)?;
        Ok(Some(Arc::new(Self {
            bucket,
            public,
            public_origin,
            locks: Mutex::new(HashMap::new()),
            jobs: tokio::sync::Semaphore::new(8),
            probe_gate: tokio::sync::Mutex::new(()),
            probe_result: Mutex::new(None),
        })))
    }

    /// Probe with an object HEAD, requiring no bucket-list permission or writes.
    /// A missing object (404), or a denied missing-object HEAD (403), still
    /// proves the S3 endpoint is reachable. PUT authorization is enforced by S3.
    pub async fn check_reachable(&self) -> Result<(), String> {
        let requested = std::time::Instant::now();
        let _probe = self.probe_gate.lock().await;
        if let Some((finished, result)) = &*self
            .probe_result
            .lock()
            .map_err(|_| "Storage health unavailable.")?
            && (*finished >= requested
                || (result.is_err() && finished.elapsed() < Duration::from_secs(1)))
        {
            return result.clone();
        }
        let _job = self.work().map_err(|e| e.message().to_string())?;
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            self.bucket.head_object("uploads/.availability"),
        )
        .await;
        let result = match result {
            Ok(Ok((_, 200 | 403 | 404))) => Ok(()),
            _ => {
                tracing::warn!("S3 reachability probe failed");
                Err("Upload rejected: media storage is unavailable. Try again later.".into())
            }
        };
        *self
            .probe_result
            .lock()
            .map_err(|_| "Storage health unavailable.")? =
            Some((std::time::Instant::now(), result.clone()));
        result
    }

    #[cfg(test)]
    pub async fn upload_url(&self, id: &str, size: u64) -> Result<String, s3::error::S3Error> {
        self.upload_url_with_hints(id, size, axum::http::HeaderMap::new())
            .await
    }
    pub async fn upload_url_with_hints(
        &self,
        id: &str,
        size: u64,
        mut headers: axum::http::HeaderMap,
    ) -> Result<String, s3::error::S3Error> {
        headers.insert(
            "content-type",
            axum::http::HeaderValue::from_static("application/octet-stream"),
        );
        if let Ok(value) = size.to_string().parse() {
            headers.insert("content-length", value);
        }
        self.public
            .presign_put(format!("uploads/{id}"), 300, Some(headers), None)
            .await
    }

    pub async fn finalize(
        &self,
        media: &mut crate::engine::media::Media,
    ) -> Result<(), StorageError> {
        use StorageError::{Invalid, Unavailable};
        if media.size == 0 {
            return Err(Invalid("Invalid upload size."));
        }
        let _job = self.work()?;
        let start = std::time::Instant::now();
        let source = format!("uploads/{}", media.id);
        let destination = media.final_key();
        let failure = |operation: &'static str, cause: s3::error::S3Error| {
            tracing::warn!(media_id = %media.id, operation, error = %cause, "S3 completion failed");
            Unavailable("Media storage unavailable; retry completion.")
        };
        let (head, status) = self
            .bucket
            .head_object(&source)
            .await
            .map_err(|e| failure("head", e))?;
        if status == 404 {
            return Err(Invalid("Uploaded file not found."));
        }
        if status != 200 {
            return Err(Unavailable("Cannot inspect upload; retry completion."));
        }
        if head.content_length.and_then(|n| u64::try_from(n).ok()) != Some(media.size) {
            return Err(Invalid("Uploaded file size does not match the request."));
        }
        let mut copy = self.bucket.clone();
        copy.extra_headers =
            hint_headers(&media.hints).map_err(|_| Invalid("Invalid upload hints."))?;
        copy.extra_headers.insert(
            "x-amz-metadata-directive",
            axum::http::HeaderValue::from_static("REPLACE"),
        );
        copy.extra_headers.insert(
            "content-type",
            axum::http::HeaderValue::from_static("application/octet-stream"),
        );
        let status = copy
            .copy_object_internal(&source, &destination)
            .await
            .map_err(|e| failure("copy", e))?;
        if status != 200 {
            return Err(Unavailable("Cannot finalize upload; retry completion."));
        }
        let end = media.size.min(4096) - 1;
        let expected_prefix =
            usize::try_from(end + 1).map_err(|_| Invalid("Invalid upload size."))?;
        // Use the same pooled client, but inspect headers and bound streaming
        // before buffering. A provider ignoring Range must not buffer a huge file.
        let url = self
            .bucket
            .presign_get(&destination, 60, None)
            .await
            .map_err(|e| failure("range signing", e))?;
        let mut response = self.bucket.http_client().get(url).header("Range",format!("bytes=0-{end}"))
            .timeout(Duration::from_secs(30)).send().await.map_err(|cause| {
                tracing::warn!(media_id = %media.id, error = %cause.without_url(), "S3 range request failed");
                Unavailable("Cannot inspect final file; retry completion.")
            })?;
        let total = match response.status().as_u16() {
            206 => response
                .headers()
                .get("content-range")
                .and_then(|header| header.to_str().ok())
                .and_then(|value| value.strip_prefix(&format!("bytes 0-{end}/")))
                .and_then(|size| size.parse::<u64>().ok()),
            200 if media.size <= 4096 => response.content_length(),
            _ => {
                return Err(Unavailable(
                    "Storage must support byte ranges; retry completion.",
                ));
            }
        };
        if total != Some(media.size) {
            return Err(Invalid("Final file size does not match the request."));
        }
        let mut prefix = Vec::with_capacity(expected_prefix);
        while let Some(chunk) = response.chunk().await.map_err(|cause| {
            tracing::warn!(media_id = %media.id, error = %cause.without_url(), "S3 prefix read failed");
            Unavailable("Cannot read final file; retry completion.")
        })? {
            if prefix.len().saturating_add(chunk.len()) > expected_prefix {
                return Err(Unavailable("Storage returned more bytes than the requested range."));
            }
            prefix.extend_from_slice(&chunk);
        }
        if prefix.len() != expected_prefix {
            return Err(Invalid("Final file size does not match the request."));
        }
        media.content_type = types::content_type(&prefix).into();
        media.object_key = destination;
        media.ready = true;
        tracing::debug!(media_id = %media.id, elapsed_ms = start.elapsed().as_millis(), s3_calls = 3, "S3 completion verified");
        Ok(())
    }

    pub async fn download_url(
        &self,
        media: &crate::engine::media::Media,
        force_download: bool,
    ) -> Result<String, s3::error::S3Error> {
        let mut query = HashMap::new();
        query.insert("response-content-type".into(), media.content_type.clone());
        let disposition = if !force_download
            && (matches!(
                media.content_type.as_str(),
                "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif"
            ) || media.content_type.starts_with("audio/")
                || media.content_type.starts_with("video/"))
        {
            "inline".into()
        } else {
            format!(
                "attachment; filename=\"download\"; filename*=UTF-8''{}",
                url::form_urlencoded::byte_serialize(media.filename.as_bytes())
                    .collect::<String>()
                    .replace('+', "%20")
            )
        };
        query.insert("response-content-disposition".into(), disposition);
        query.insert("response-cache-control".into(), "private, no-store".into());
        self.public
            .presign_get(
                &media.object_key,
                media
                    .expires
                    .map_or(60, |t| {
                        t.saturating_sub(crate::engine::media::timestamp()).min(60)
                    })
                    .max(1)
                    .try_into()
                    .map_err(|_| {
                        s3::error::S3Error::Io(std::io::Error::other("Invalid download lifetime"))
                    })?,
                Some(query),
            )
            .await
    }

    pub async fn delete_keys(
        &self,
        keys: &std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        let objects = keys
            .iter()
            .cloned()
            .map(s3::serde_types::ObjectIdentifier::new)
            .collect::<Vec<_>>();
        let result = {
            let _job = self.work().map_err(|e| e.message().to_string())?;
            self.bucket.delete_objects(objects).await
        };
        match result {
            Ok(result)
                if result.errors.is_empty()
                    && keys
                        .iter()
                        .all(|key| result.deleted.iter().any(|deleted| &deleted.key == key)) =>
            {
                Ok(())
            }
            Err(s3::error::S3Error::HttpFailWithBody(405 | 501, _)) => {
                for key in keys {
                    self.delete(key).await?;
                }
                Ok(())
            }
            Ok(result) => {
                tracing::warn!(
                    errors = result.errors.len(),
                    "S3 batch deletion incomplete; retaining quota"
                );
                Err("Storage deletion incomplete.".into())
            }
            Err(cause) => {
                tracing::warn!(error = %cause, "S3 batch deletion failed");
                Err("Storage deletion failed.".into())
            }
        }
    }
    pub async fn delete(&self, key: &str) -> Result<(), String> {
        let _job = self.work().map_err(|e| e.message().to_string())?;
        let response = self.bucket.delete_object(key).await.map_err(|cause| {
            tracing::warn!(key, error = %cause, "S3 deletion failed");
            "Storage deletion failed."
        })?;
        if !matches!(response.status_code(), 200 | 204 | 404) {
            return Err("Storage deletion failed.".into());
        }
        Ok(())
    }
}

/// Used only by the debug launcher; never provisions a production bucket.
pub(crate) async fn setup_local_s3() -> Result<(), String> {
    let endpoint = std::env::var("CHAT_S3_ENDPOINT").map_err(|_| "Missing CHAT_S3_ENDPOINT.")?;
    let url = url::Url::parse(&endpoint).map_err(|_| "Invalid local S3 endpoint.")?;
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
        return Err("Local S3 setup requires an HTTP endpoint on 127.0.0.1.".into());
    }
    if std::env::var("CHAT_S3_PATH_STYLE").as_deref() != Ok("true") {
        return Err("Local S3 setup requires path-style addressing.".into());
    }
    let storage =
        MediaStorage::from_config(false, None)?.ok_or("Missing local S3 configuration.")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        if !storage.bucket.exists().await.map_err(|e| format!("Cannot inspect local S3 buckets: {e}"))? {
            let response = Bucket::create_with_path_style(
                &storage.bucket.name(),
                storage.bucket.region.clone(),
                storage.bucket.credentials().await.map_err(|_| "Invalid local S3 credentials.")?,
                s3::BucketConfiguration::default(),
            ).await.map_err(|e| format!("Cannot create local S3 bucket: {e}"))?;
            if response.response_code != 200 {
                return Err(format!("Local bucket creation returned {}.", response.response_code));
            }
        }
        let cors = s3::serde_types::CorsConfiguration::new(vec![s3::serde_types::CorsRule::new(
            Some(vec!["content-type".into(), "x-amz-meta-client-filename".into(), "x-amz-meta-client-content-type".into(), "x-amz-meta-client-label".into(), "x-amz-meta-client-client".into()]),
            vec!["PUT".into(), "GET".into(), "HEAD".into()],
            vec!["http://127.0.0.1:5173".into(), "http://localhost:5173".into()],
            Some(vec!["ETag".into()]),
            None,
            Some(300),
        )]);
        let response = storage.bucket.put_bucket_cors("", &cors).await
            .map_err(|e| format!("Cannot configure local S3 CORS: {e}"))?;
        if response.status_code() != 200 {
            return Err(format!("Local S3 CORS setup returned {}.", response.status_code()));
        }
        tracing::info!(bucket = %storage.bucket.name(), endpoint, "Local media bucket and browser CORS ready");
        Ok(())
    }).await.map_err(|_| "Local S3 setup timed out. Is auto/start-s3.sh running?".to_string())?
}

fn bucket_origin(bucket: &Bucket) -> Result<String, String> {
    Ok(url::Url::parse(&bucket.url())
        .map_err(|_| "Invalid public bucket URL.")?
        .origin()
        .ascii_serialization())
}

fn s3_bucket(
    name: &str,
    region: &str,
    endpoint: String,
    path_style: bool,
    credentials: Credentials,
) -> Result<Box<Bucket>, String> {
    Bucket::new(
        name,
        Region::Custom {
            region: region.into(),
            endpoint,
        },
        credentials,
    )
    .and_then(|b| {
        let b = if path_style { b.with_path_style() } else { b };
        b.with_request_timeout(Duration::from_secs(30))
    })
    .map_err(|_| "Invalid storage configuration.".into())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::{Engine, media::Media};

    pub(crate) fn test_storage(endpoint: &str) -> Arc<MediaStorage> {
        let credentials = Credentials::new(Some("test"), Some("secret"), None, None, None).unwrap();
        let bucket = s3_bucket("private", "us-east-1", endpoint.into(), true, credentials).unwrap();
        Arc::new(MediaStorage {
            public: bucket.clone(),
            bucket,
            public_origin: endpoint.into(),
            locks: Mutex::new(HashMap::new()),
            jobs: tokio::sync::Semaphore::new(8),
            probe_gate: tokio::sync::Mutex::new(()),
            probe_result: Mutex::new(None),
        })
    }
    #[test]
    fn upload_hints_reject_unknown_headers_controls_unicode_and_excessive_values() {
        use std::collections::BTreeMap;
        for (key, value) in [
            ("uploader", "trusted"),
            ("label", "bad\r\ninjected: true"),
            ("filename", "你好.png"),
            ("client", " padded "),
            ("client", ""),
        ] {
            assert!(hint_headers(&BTreeMap::from([(key.into(), value.into())])).is_err());
        }
        assert!(hint_headers(&BTreeMap::from([("label".into(), "x".repeat(257))])).is_err());
        let headers = hint_headers(&BTreeMap::from([
            ("filename".into(), "%E4%BD%A0.png".into()),
            ("content-type".into(), "text/html".into()),
        ]))
        .unwrap();
        assert_eq!(headers["x-amz-meta-client-filename"], "%E4%BD%A0.png");
        assert!(!headers.contains_key("content-type"));
    }
    #[tokio::test]
    async fn upload_hints_are_signed_and_finalization_replaces_untrusted_staging_metadata() {
        use std::collections::BTreeMap;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let storage = test_storage(&format!("http://{}", listener.local_addr().unwrap()));
        let hints = BTreeMap::from([
            ("label".into(), "inspection-only".into()),
            ("content-type".into(), "text/html".into()),
        ]);
        let url = storage
            .upload_url_with_hints("object", 8, hint_headers(&hints).unwrap())
            .await
            .unwrap();
        let url = url::Url::parse(&url).unwrap();
        let signed = url
            .query_pairs()
            .find(|(k, _)| k == "X-Amz-SignedHeaders")
            .unwrap()
            .1
            .into_owned();
        assert!(signed.contains("x-amz-meta-client-label"));
        assert!(signed.contains("x-amz-meta-client-content-type"));
        let server = tokio::spawn(async move {
            for step in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let size = socket.read(&mut chunk).await.unwrap();
                    assert!(size > 0);
                    bytes.extend_from_slice(&chunk[..size]);
                    if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
                let response = match step {
                    0 => {
                        assert!(request.starts_with("head "));
                        "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nx-amz-meta-client-label: tampered\r\nx-amz-meta-trusted-owner: forged\r\nConnection: close\r\n\r\n".as_bytes().to_vec()
                    }
                    1 => {
                        assert!(request.starts_with("put /private/media/7d/"));
                        assert!(request.contains("x-amz-metadata-directive: replace"));
                        assert!(request.contains("x-amz-meta-client-label: inspection-only"));
                        assert!(request.contains("x-amz-meta-client-content-type: text/html"));
                        assert!(
                            !request.contains("tampered") && !request.contains("trusted-owner")
                        );
                        let body = "<CopyObjectResult><ETag>test</ETag></CopyObjectResult>";
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
                    }
                    _ => {
                        assert!(request.starts_with("get "));
                        b"HTTP/1.1 206 Partial Content\r\nContent-Length: 8\r\nContent-Range: bytes 0-7/8\r\nConnection: close\r\n\r\n\x89PNG\r\n\x1a\n".to_vec()
                    }
                };
                socket.write_all(&response).await.unwrap();
            }
        });
        let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
        engine
            .provision("alice", "hash".into(), true, false)
            .unwrap();
        engine
            .execute(Some("alice"), None, "/room new #files")
            .unwrap();
        let mut media = engine
            .allocate_media_with_hints("alice", &engine.data.rooms["files"].id, "file", 8, hints)
            .unwrap();
        storage.finalize(&mut media).await.unwrap();
        assert_eq!(media.content_type, "image/png");
        assert_eq!(media.hints["content-type"], "text/html");
        server.await.unwrap();
    }
    #[test]
    fn object_locks_and_storage_capacity_are_independent_and_bounded() {
        let storage = test_storage("http://127.0.0.1:1");
        let first = storage.lock_media("first").unwrap();
        assert!(storage.lock_media("first").is_err());
        assert!(storage.lock_media("second").is_ok());
        let permits: Vec<_> = (0..8).map(|_| storage.work().unwrap()).collect();
        assert!(storage.work().is_err());
        drop(permits);
        drop(first);
        assert!(storage.lock_media("first").is_ok());
        assert!(storage.work().is_ok());
        assert!(storage.locks.lock().unwrap().len() <= 1);
    }
    #[tokio::test]
    async fn completion_classifies_storage_failure_and_invalid_size_without_underflow() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, expected_invalid) in [(200, true), (404, true), (503, false)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let storage = test_storage(&format!("http://{}", listener.local_addr().unwrap()));
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut input = [0; 4096];
                let count = socket.read(&mut input).await.unwrap();
                assert!(count > 0);
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 4\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
            engine
                .provision("alice", "hash".into(), true, false)
                .unwrap();
            engine
                .execute(Some("alice"), None, "/new room files")
                .unwrap();
            let mut media = engine
                .allocate_media("alice", &engine.data.rooms["files"].id, "file", 1)
                .unwrap();
            let error = storage.finalize(&mut media).await.unwrap_err();
            assert_eq!(matches!(error, StorageError::Invalid(_)), expected_invalid);
            server.await.unwrap();
            media.size = 0;
            assert!(matches!(
                storage.finalize(&mut media).await,
                Err(StorageError::Invalid(_))
            ));
        }
    }
    #[tokio::test]
    async fn signed_download_filename_preserves_quotes_spaces_unicode_and_plus() {
        let storage = test_storage("http://127.0.0.1:1");
        let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
        engine
            .provision("alice", "hash".into(), true, false)
            .unwrap();
        engine
            .execute(Some("alice"), None, "/new room files")
            .unwrap();
        let media = engine
            .allocate_media(
                "alice",
                &engine.data.rooms["files"].id,
                "hello \"你好+\".txt",
                1,
            )
            .unwrap();
        let url = url::Url::parse(&storage.download_url(&media, true).await.unwrap()).unwrap();
        let query: HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            query["response-content-disposition"],
            "attachment; filename=\"download\"; filename*=UTF-8''hello%20%22%E4%BD%A0%E5%A5%BD%2B%22.txt"
        );
    }

    #[tokio::test]
    async fn batch_delete_checks_each_key_including_partial_failures() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (xml, success) in [
            (
                "<DeleteResult><Deleted><Key>one</Key></Deleted><Deleted><Key>two</Key></Deleted></DeleteResult>",
                true,
            ),
            (
                "<DeleteResult><Deleted><Key>one</Key></Deleted><Error><Key>two</Key><Code>AccessDenied</Code><Message>Denied</Message></Error></DeleteResult>",
                false,
            ),
            ("<DeleteResult/>", false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let storage = test_storage(&format!("http://{}", listener.local_addr().unwrap()));
            let response = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|value| value.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",xml.len()).as_bytes()).await.unwrap();
            });
            assert_eq!(
                storage
                    .delete_keys(&std::collections::BTreeSet::from([
                        "one".into(),
                        "two".into()
                    ]))
                    .await
                    .is_ok(),
                success
            );
            response.await.unwrap();
        }
    }
    #[tokio::test]
    async fn overlapping_probes_share_work_but_later_requests_probe_again() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let storage = test_storage(&format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(
                    b"HTTP/1.1 404 Missing\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let (one, two) = tokio::join!(storage.check_reachable(), storage.check_reachable());
        assert!(one.is_ok() && two.is_ok());
        server.await.unwrap();
        assert!(storage.check_reachable().await.is_err());
    }

    #[tokio::test]
    async fn failed_probes_back_off_briefly_and_recover_without_caching_success() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let storage = test_storage(&format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            for status in [503, 404] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            }
        });
        assert!(storage.check_reachable().await.is_err());
        let first = storage.probe_result.lock().unwrap().as_ref().unwrap().0;
        assert!(storage.check_reachable().await.is_err());
        assert_eq!(
            storage.probe_result.lock().unwrap().as_ref().unwrap().0,
            first
        );
        storage.probe_result.lock().unwrap().as_mut().unwrap().0 -= Duration::from_secs(2);
        assert!(storage.check_reachable().await.is_ok());
        server.await.unwrap();
        // Successful health is not cached after the server goes away.
        assert!(storage.check_reachable().await.is_err());
    }
    #[tokio::test]
    async fn reachability_accepts_private_bucket_responses_but_rejects_outages() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for status in [200, 403, 404, 503] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let response = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..n])
                        .starts_with("HEAD /private/uploads/.availability")
                );
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let credentials =
                Credentials::new(Some("test"), Some("secret"), None, None, None).unwrap();
            let bucket = s3_bucket(
                "private",
                "us-east-1",
                format!("http://{address}"),
                true,
                credentials,
            )
            .unwrap();
            let storage = MediaStorage {
                public: bucket.clone(),
                bucket,
                public_origin: format!("http://{address}"),
                locks: Mutex::new(HashMap::new()),
                jobs: tokio::sync::Semaphore::new(8),
                probe_gate: tokio::sync::Mutex::new(()),
                probe_result: Mutex::new(None),
            };
            assert_eq!(storage.check_reachable().await.is_ok(), status != 503);
            response.await.unwrap();
            assert!(
                storage
                    .check_reachable()
                    .await
                    .unwrap_err()
                    .starts_with("Upload rejected:")
            );
        }
    }

    #[tokio::test]
    async fn provider_addressing_and_signing_remain_s3_compatible() {
        for (endpoint, region, path_style, expected) in [
            (
                "http://127.0.0.1:19000",
                "us-east-1",
                true,
                "http://127.0.0.1:19000/private",
            ),
            (
                "https://s3.eu-west-1.amazonaws.com",
                "eu-west-1",
                false,
                "https://private.s3.eu-west-1.amazonaws.com",
            ),
            (
                "https://account.r2.cloudflarestorage.com",
                "auto",
                true,
                "https://account.r2.cloudflarestorage.com/private",
            ),
        ] {
            let credentials = Credentials::new(
                Some("test-access"),
                Some("test-secret"),
                None,
                Some("test-session"),
                None,
            )
            .unwrap();
            let bucket =
                s3_bucket("private", region, endpoint.into(), path_style, credentials).unwrap();
            assert_eq!(bucket.url(), expected);
            assert_eq!(
                bucket_origin(&bucket).unwrap(),
                url::Url::parse(expected)
                    .unwrap()
                    .origin()
                    .ascii_serialization()
            );
            let storage = MediaStorage {
                public: bucket.clone(),
                bucket,
                public_origin: endpoint.into(),
                locks: Mutex::new(HashMap::new()),
                jobs: tokio::sync::Semaphore::new(8),
                probe_gate: tokio::sync::Mutex::new(()),
                probe_result: Mutex::new(None),
            };
            let signed = storage.upload_url("test-object", 123).await.unwrap();
            let url = url::Url::parse(&signed).unwrap();
            let query: HashMap<_, _> = url.query_pairs().collect();
            assert!(query["X-Amz-Credential"].contains(&format!("/{region}/s3/aws4_request")));
            assert_eq!(query["X-Amz-Security-Token"], "test-session");
            assert!(query["X-Amz-SignedHeaders"].contains("content-length"));
            assert!(query["X-Amz-SignedHeaders"].contains("content-type"));
            assert!(!signed.contains("x-amz-tagging"));
        }
    }

    #[tokio::test]
    #[ignore = "requires local RustFS and CHAT_S3_* settings; creates a dedicated test bucket"]
    async fn rustfs_round_trip() {
        let configured = MediaStorage::from_config(false, None).unwrap().unwrap();
        let keep = std::env::var("CHAT_S3_TEST_KEEP_BUCKET").as_deref() == Ok("1");
        let name = if keep {
            configured.bucket.name()
        } else {
            format!("commonroom-test-{}", uuid::Uuid::new_v4().simple())
        };
        let response = Bucket::create_with_path_style(
            &name,
            configured.bucket.region.clone(),
            configured.bucket.credentials().await.unwrap(),
            s3::BucketConfiguration::default(),
        )
        .await
        .unwrap();
        assert_eq!(response.response_code, 200);
        let bucket = response.bucket;
        let storage = MediaStorage {
            public: bucket.clone(),
            bucket,
            public_origin: configured.public_origin.clone(),
            locks: Mutex::new(HashMap::new()),
            jobs: tokio::sync::Semaphore::new(8),
            probe_gate: tokio::sync::Mutex::new(()),
            probe_result: Mutex::new(None),
        };
        let cors = s3::serde_types::CorsConfiguration::new(vec![s3::serde_types::CorsRule::new(
            Some(vec![
                "content-type".into(),
                "x-amz-meta-client-filename".into(),
                "x-amz-meta-client-content-type".into(),
                "x-amz-meta-client-label".into(),
                "x-amz-meta-client-client".into(),
            ]),
            vec!["PUT".into(), "GET".into(), "HEAD".into()],
            vec!["http://127.0.0.1:5173".into()],
            Some(vec!["ETag".into()]),
            None,
            Some(300),
        )]);
        assert_eq!(
            storage
                .bucket
                .put_bucket_cors("", &cors)
                .await
                .unwrap()
                .status_code(),
            200
        );
        let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
        engine
            .provision("alice", "hash".into(), true, false)
            .unwrap();
        engine
            .execute(Some("alice"), None, "/new room files")
            .unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nhello".to_vec();
        let mut media: Media = engine
            .allocate_media(
                "alice",
                &engine.data.rooms["files"].id,
                "photo.png",
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap();
        let url = storage.upload_url(&media.id, media.size).await.unwrap();
        let client = storage.public.http_client();
        let preflight = client
            .request(axum::http::Method::OPTIONS, &url)
            .header("Origin", "http://127.0.0.1:5173")
            .header("Access-Control-Request-Method", "PUT")
            .header("Access-Control-Request-Headers", "content-type")
            .send()
            .await
            .unwrap();
        assert!(preflight.status().is_success(), "{}", preflight.status());
        assert_eq!(
            preflight
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "http://127.0.0.1:5173"
        );
        // Public and unsigned reads must not expose the pending object.
        let unsigned = url.split('?').next().unwrap();
        assert!(
            !client
                .get(unsigned)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        let uploaded = client
            .put(&url)
            .header("Content-Type", "application/octet-stream")
            .body(bytes.clone())
            .send()
            .await
            .unwrap();
        assert!(
            uploaded.status().is_success(),
            "{}",
            uploaded.text().await.unwrap()
        );
        storage.finalize(&mut media).await.unwrap();
        assert_eq!(media.content_type, "image/png");
        engine.finish_media("alice", &media).unwrap();
        assert!(engine.readable_media("alice", &media.id).is_err());
        engine
            .execute_formatted(
                Some("alice"),
                Some("files"),
                &format!(
                    "![photo](http://127.0.0.1:3000/api/media/{}/file)",
                    media.id
                ),
                true,
            )
            .unwrap();
        let readable = engine.readable_media("alice", &media.id).unwrap();
        let download = storage.download_url(&readable, false).await.unwrap();
        let fetched = client.get(&download).send().await.unwrap();
        assert_eq!(fetched.headers().get("content-type").unwrap(), "image/png");
        assert_eq!(
            fetched.headers().get("content-disposition").unwrap(),
            "inline"
        );
        assert_eq!(fetched.bytes().await.unwrap().as_ref(), bytes);
        // Reusing the upload URL cannot alter the published final object.
        let replacement = vec![b'x'; bytes.len()];
        assert!(
            client
                .put(&url)
                .header("Content-Type", "application/octet-stream")
                .body(replacement)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert_eq!(
            client
                .get(&download)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .as_ref(),
            bytes
        );
        let range = client
            .get(&download)
            .header("Range", "bytes=0-3")
            .send()
            .await
            .unwrap();
        assert_eq!(range.status(), 206);
        assert_eq!(range.bytes().await.unwrap().as_ref(), &bytes[..4]);
        storage
            .delete(&format!("uploads/{}", media.id))
            .await
            .unwrap();
        storage.delete(&media.object_key).await.unwrap();
        if !keep {
            storage.bucket.delete().await.unwrap();
        }
    }
}
