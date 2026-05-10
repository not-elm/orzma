//! Tracks which extension owns which activity / pane, plus extension launch_dirs
//! for path-traversal validation in HTTP handlers.

use ozmux_multiplexer::{activity::ActivityId, pane::PaneId};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

#[derive(Clone, Default)]
pub struct ExtensionRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Default)]
struct RegistryInner {
    by_name: HashMap<String, ExtensionInfo>,
    activity_owner: HashMap<ActivityId, String>,
    pane_owner: HashMap<PaneId, String>,
}

#[derive(Clone, Debug)]
pub struct ExtensionInfo {
    pub name: String,
    pub launch_dir: PathBuf,
}

impl ExtensionRegistry {
    pub fn register(&self, name: &str, launch_dir: &Path) {
        let mut g = self.inner.write().expect("registry poisoned");
        g.by_name.insert(
            name.to_string(),
            ExtensionInfo {
                name: name.to_string(),
                launch_dir: launch_dir.to_path_buf(),
            },
        );
    }

    pub fn unregister(&self, name: &str) {
        let mut g = self.inner.write().expect("registry poisoned");
        g.by_name.remove(name);
    }

    pub fn get(&self, name: &str) -> Option<ExtensionInfo> {
        let g = self.inner.read().expect("registry poisoned");
        g.by_name.get(name).cloned()
    }

    pub fn record_activity_owner(&self, activity_id: &ActivityId, name: &str) {
        let mut g = self.inner.write().expect("registry poisoned");
        g.activity_owner.insert(activity_id.clone(), name.to_string());
    }

    pub fn record_pane_owner(&self, pane_id: &PaneId, name: &str) {
        let mut g = self.inner.write().expect("registry poisoned");
        g.pane_owner.insert(pane_id.clone(), name.to_string());
    }

    pub fn activity_owner(&self, activity_id: &ActivityId) -> Option<String> {
        let g = self.inner.read().expect("registry poisoned");
        g.activity_owner.get(activity_id).cloned()
    }

    pub fn pane_owner(&self, pane_id: &PaneId) -> Option<String> {
        let g = self.inner.read().expect("registry poisoned");
        g.pane_owner.get(pane_id).cloned()
    }

    pub fn forget_activity(&self, activity_id: &ActivityId) {
        let mut g = self.inner.write().expect("registry poisoned");
        g.activity_owner.remove(activity_id);
    }

    pub fn forget_pane(&self, pane_id: &PaneId) {
        let mut g = self.inner.write().expect("registry poisoned");
        g.pane_owner.remove(pane_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get_returns_info() {
        let reg = ExtensionRegistry::default();
        reg.register("memo", Path::new("/tmp/memo"));
        let info = reg.get("memo").unwrap();
        assert_eq!(info.name, "memo");
        assert_eq!(info.launch_dir, PathBuf::from("/tmp/memo"));
    }

    #[test]
    fn unregister_removes_entry() {
        let reg = ExtensionRegistry::default();
        reg.register("memo", Path::new("/tmp/memo"));
        reg.unregister("memo");
        assert!(reg.get("memo").is_none());
    }

    #[test]
    fn activity_owner_round_trip() {
        let reg = ExtensionRegistry::default();
        let aid = ActivityId::new();
        reg.record_activity_owner(&aid, "memo");
        assert_eq!(reg.activity_owner(&aid).as_deref(), Some("memo"));
        reg.forget_activity(&aid);
        assert!(reg.activity_owner(&aid).is_none());
    }

    #[test]
    fn pane_owner_round_trip() {
        let reg = ExtensionRegistry::default();
        let pid = PaneId::new();
        reg.record_pane_owner(&pid, "memo");
        assert_eq!(reg.pane_owner(&pid).as_deref(), Some("memo"));
        reg.forget_pane(&pid);
        assert!(reg.pane_owner(&pid).is_none());
    }

    #[test]
    fn registry_is_clone_safe() {
        let reg = ExtensionRegistry::default();
        let cloned = reg.clone();
        reg.register("memo", Path::new("/tmp/memo"));
        assert!(cloned.get("memo").is_some());
    }
}
