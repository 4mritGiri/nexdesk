use nexdesk_core::profiles::{Profile, Store};
use nexdesk_session::SessionManager;

pub struct ManagerState {
    pub store: Option<Store>,
    pub profiles: Vec<Profile>,
    pub selected: Option<usize>,
    pub sessions: SessionManager,
    pub password: String,
    pub status: String,
}

impl ManagerState {
    pub fn load(engine_path: std::path::PathBuf) -> Self {
        let store = nexdesk_core::profiles::default_store_dir().map(Store::new);
        let profiles = store.as_ref().map(Store::list).unwrap_or_default();
        Self { store, profiles, selected: None, sessions: SessionManager::new(engine_path), password: String::new(), status: String::new() }
    }
}
