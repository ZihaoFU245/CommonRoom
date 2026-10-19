# Media upload development

Implemented on the media branch; deployment checks remain below.

- [x] Start manually supplied `tmp/rustfs` using `auto/start-s3.sh`, without downloads.
- [x] Confine launcher writes to `tmp/` with a clean environment and OS filesystem policy.
- [x] Use provider-neutral `s3` connection configuration and environment-only credentials.
- [x] Keep `rust-s3`; support AWS/R2/RustFS endpoint, region and addressing settings.
- [x] Use retention prefixes, avoiding required object tags unsupported by R2.
- [x] Add default room/private uploads with per-user room revocation and typed `w:media.upload` grants with typed size/age constraints and delegation limits.
- [x] Configure chat size, quotas, pending lifetime, allocation rate and cleanup scheduling.
- [x] Reserve verified unsent uploads until transactional message publication.
- [x] Show a separate removable attachment preview/card; generate Markdown at submission.
- [x] Support text plus media, media-only sends and a first private message containing media.
- [x] Default media lifetime to seven days; allow explicit -1 retention grants.
- [x] Enforce age from upload verification, without extending it on send/edit/reuse.
- [x] Clean expired referenced objects, pending objects and staging copies with durable claims.
- [x] Preserve expired-reference tombstones, transactional rollback and v13 -> v14 migration.
- [x] Verify real RustFS upload/completion/download/reference behavior locally.
- [x] Add AVIF, native audio/video, and optional sandboxed HTML/SVG file previews.
- [x] Add AWS/R2/RustFS addressing/signing tests and media policy/lifecycle regression tests.

Deployment checks requiring the actual environment:

- [ ] Verify real AWS/R2 account credentials, bucket CORS, signed download overrides and ingress limits.
- [ ] Provision desired prefix lifecycle rules in the selected provider; exclude permanent media.
- [ ] Verify production HTTPS/CSP/base paths and actual bucket lifecycle/version cleanup.

Browser sends retain the existing uncertain-acknowledgement behavior: inspect
history before retrying; no automatic resend occurs. Durable browser message
idempotency would be a separate protocol change. Multipart/resume, scanning,
thumbnails and a dedicated expired-attachment placeholder are not included.

See [Media uploads](docs/media-uploads.md) for configuration, policy semantics,
local setup, supported operations and verification commands.
