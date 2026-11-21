# Commonroom 0.3.2

- See time to first chat, image, sticker, acknowledged message, and ready
  attachment in `/system metrics`. The report breaks down client failures by
  device class, browser engine, and app mode.
- Compare conditional `200` and `304` API latency, see browser cache reuse,
  and read server uptime as a duration.
- Configure browser metrics sampling with
  `server.client_metrics_sample_percent`. Production defaults to 5%, and
  debug builds sample all authenticated browser pages by default.
- Use the per-browser **Always send with Enter** preference. This release
  also improves iOS Safari keyboard and touch layouts and fills the
  swipe-to-reply arrow when the gesture reaches its trigger threshold.
- Update Rust and web dependencies.

Upgrades from 0.3.1 can use existing data directly. Deploy the updated server
and web bundle together to enable configurable client metrics. See
[Data](docs/data.md) for compatibility and [Deployment](docs/deployment.md)
for sampling configuration.
