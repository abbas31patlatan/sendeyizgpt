use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

#[derive(Default)]
pub struct ModelWatcher {
    watcher: Option<RecommendedWatcher>,
    roots: BTreeSet<PathBuf>,
}

impl ModelWatcher {
    pub fn sync(&mut self, app: &AppHandle, roots: BTreeSet<PathBuf>) -> Result<(), String> {
        if self.watcher.is_none() {
            let app = app.clone();
            let mut last = Instant::now() - Duration::from_secs(2);
            self.watcher = Some(
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    let changed = event.is_ok_and(|event| {
                        matches!(
                            event.kind,
                            notify::EventKind::Create(_)
                                | notify::EventKind::Modify(_)
                                | notify::EventKind::Remove(_)
                        ) && event.paths.iter().any(|path| {
                            path.extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
                                || path.extension().is_none()
                        })
                    });
                    if changed && last.elapsed() >= Duration::from_secs(1) {
                        last = Instant::now();
                        let _ = app.emit("aegis://models-changed", ());
                    }
                })
                .map_err(|error| error.to_string())?,
            );
        }
        let watcher = self
            .watcher
            .as_mut()
            .ok_or_else(|| "watcher unavailable".to_owned())?;
        for root in self.roots.difference(&roots) {
            let _ = watcher.unwatch(root);
        }
        self.roots.retain(|root| roots.contains(root));
        for root in roots {
            if !self.roots.contains(&root) {
                watcher
                    .watch(&root, RecursiveMode::Recursive)
                    .map_err(|error| error.to_string())?;
                self.roots.insert(root);
            }
        }
        Ok(())
    }
}
