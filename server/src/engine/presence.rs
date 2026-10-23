//! Connection counts and the shared active-account directory are transient state.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct Projection {
    version: (u64, u64),
    names: Arc<Vec<String>>,
    frame: Arc<str>,
}
#[derive(Default)]
pub(super) struct Presence {
    connections: BTreeMap<String, usize>,
    pub(super) revision: u64,
    projection: Mutex<Option<Projection>>,
}
impl Presence {
    pub(super) fn connect(&mut self, name: &str) -> bool {
        let count = self.connections.entry(name.into()).or_default();
        *count += 1;
        let changed = *count == 1;
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }
    pub(super) fn disconnect(&mut self, name: &str) -> bool {
        if let Some(count) = self.connections.get_mut(name) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.connections.remove(name);
                self.revision = self.revision.wrapping_add(1);
                return true;
            }
        }
        false
    }
    pub(super) fn count(&self, name: &str) -> usize {
        self.connections.get(name).copied().unwrap_or(0)
    }
    fn projection(&self, revision: u64, active: impl Fn(&str) -> bool) -> Projection {
        let version = (revision, self.revision);
        if let Ok(cache) = self.projection.lock()
            && let Some(projection) = cache
                .as_ref()
                .filter(|projection| projection.version == version)
        {
            return projection.clone();
        }
        let names: Vec<_> = self
            .connections
            .keys()
            .filter(|name| active(name))
            .cloned()
            .collect();
        let frame = serde_json::json!({"kind":"presence","online":names})
            .to_string()
            .into();
        let projection = Projection {
            version,
            names: Arc::new(names),
            frame,
        };
        // This is a derived cache. A poisoned cache is bypassed; authority and
        // the active directory are recomputed from the guarded engine state.
        if let Ok(mut cache) = self.projection.lock() {
            *cache = Some(projection.clone());
        }
        projection
    }
    pub(super) fn online(&self, revision: u64, active: impl Fn(&str) -> bool) -> Vec<String> {
        self.projection(revision, active).names.as_ref().clone()
    }
    pub(super) fn frame(&self, revision: u64, active: impl Fn(&str) -> bool) -> Arc<str> {
        self.projection(revision, active).frame
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn projections_share_frames_and_invalidate_on_visible_connections_or_account_changes() {
        let mut presence = Presence::default();
        assert!(presence.connect("alice"));
        assert!(presence.connect("bob"));
        let first = presence.frame(1, |_| true);
        assert!(Arc::ptr_eq(&first, &presence.frame(1, |_| true)));
        assert!(!presence.connect("alice"));
        assert_eq!(presence.count("alice"), 2);
        assert!(!presence.disconnect("alice"));
        assert!(Arc::ptr_eq(&first, &presence.frame(1, |_| true)));
        let filtered = presence.frame(2, |name| name != "bob");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&filtered).unwrap()["online"],
            serde_json::json!(["alice"])
        );
        assert!(!Arc::ptr_eq(&first, &filtered));
        assert!(presence.disconnect("alice"));
        assert_eq!(
            presence.online(2, |name| name != "bob"),
            Vec::<String>::new()
        );
        assert!(!presence.disconnect("alice"));
        assert!(presence.disconnect("bob"));
    }
}
