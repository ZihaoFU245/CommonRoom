//! Transient bot activity and socket-owned command discovery.
#[derive(Clone)]
pub(crate) struct Hub {
    pub activities: tokio::sync::broadcast::Sender<serde_json::Value>,
    pub commands: super::discovery::Hub,
}
impl Default for Hub {
    fn default() -> Self {
        Self {
            activities: tokio::sync::broadcast::channel(64).0,
            commands: Default::default(),
        }
    }
}
