//! Persistence of the last successfully logged-in username, gated by
//! `remember_last_user` in the config.

use std::path::Path;

use crate::config::Config;

pub fn load_last_user(cfg: &Config) -> Option<String> {
    if !cfg.remember_last_user {
        return None;
    }
    std::fs::read_to_string(cfg.last_user_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn save_last_user(cfg: &Config, username: &str) {
    if !cfg.remember_last_user {
        return;
    }
    let path = cfg.last_user_file();
    if let Some(parent) = Path::new(&path).parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            log::warn!("could not create state dir {}: {err}", parent.display());
            return;
        }
    }
    if let Err(err) = std::fs::write(&path, username) {
        log::warn!("could not persist last user to {}: {err}", path.display());
    }
}
