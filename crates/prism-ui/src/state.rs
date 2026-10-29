//! Application state shared across the UI.

use prism_core::{InstanceList, Paths};
use std::path::PathBuf;

pub struct App {
    pub paths: Paths,
    pub instances: InstanceList,
    pub selected: Option<String>,
}

impl App {
    pub fn new() -> App {
        let paths = Paths::new();
        if let Err(e) = paths.ensure() {
            tracing::warn!("failed to create data dirs: {e}");
        }
        let instances = InstanceList::load(&paths).unwrap_or_else(|e| {
            tracing::error!("failed to load instances: {e}");
            InstanceList::default()
        });
        App {
            paths,
            instances,
            selected: None,
        }
    }

    pub fn reload_instances(&mut self) {
        self.instances = InstanceList::load(&self.paths).unwrap_or_else(|e| {
            tracing::error!("failed to load instances: {e}");
            InstanceList::default()
        });
    }

    pub fn selected_instance(&self) -> Option<&prism_core::instance::Instance> {
        self.selected.as_deref().and_then(|id| self.instances.get(id))
    }

    /// Status bar description of the selected instance, mirroring
    /// `MainWindow::getStatusbarDescription()`.
    pub fn status_description(&self) -> String {
        match self.selected_instance() {
            Some(inst) => {
                let played = format_play_time(inst.total_time_played);
                format!("Name: {} • Play time: {played}", inst.name)
            }
            None => "No instance selected".to_string(),
        }
    }

    pub fn instance_icon_path(&self, inst: &prism_core::instance::Instance) -> Option<PathBuf> {
        let dir = self.paths.icons_dir();
        for ext in ["png", "svg", "jpg", "jpeg", "webp", "ico"] {
            let p = dir.join(format!("{}.{ext}", inst.icon_key));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

pub fn format_play_time(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}
