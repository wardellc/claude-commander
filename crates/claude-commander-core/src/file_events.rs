//! Filesystem invalidations. Watch directories so atomic replacement is visible.
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::watch;

pub struct FileEvents {
    _watcher: RecommendedWatcher,
    rx: watch::Receiver<u64>,
    failed: Arc<AtomicBool>,
}

impl FileEvents {
    pub fn watch(path: &Path, recursive: bool) -> notify::Result<Self> {
        Self::watch_filtered(path, recursive, |_| true)
    }

    pub fn watch_filtered(
        path: &Path,
        recursive: bool,
        accept: impl Fn(&Path) -> bool + Send + 'static,
    ) -> notify::Result<Self> {
        Self::watch_paths(&[path.to_path_buf()], recursive, accept)
    }

    /// State stores are flat siblings of worktrees; never recursively register
    /// the data tree merely to observe these three small directories.
    pub fn state(data_dir: &Path) -> notify::Result<Self> {
        let paths = [
            data_dir.to_path_buf(),
            data_dir.join("comments"),
            data_dir.join("reviewed"),
        ];
        for path in &paths {
            std::fs::create_dir_all(path)?;
        }
        Self::watch_paths(&paths, false, |path| {
            path.file_name().is_some_and(|name| name == "state.json")
                || path
                    .parent()
                    .and_then(|p| p.file_name())
                    .is_some_and(|name| name == "comments" || name == "reviewed")
                    && path.extension().is_some_and(|ext| ext == "json")
        })
    }

    fn watch_paths(
        paths: &[PathBuf],
        recursive: bool,
        accept: impl Fn(&Path) -> bool + Send + 'static,
    ) -> notify::Result<Self> {
        let roots = paths.to_vec();
        let (tx, rx) = watch::channel(0_u64);
        let failed = Arc::new(AtomicBool::new(false));
        let callback_failed = failed.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let relevant = match event {
                    Ok(event) => {
                        // A directory watch may no longer cover its replacement.
                        // Wake the consumer and restore reconciliation on that path.
                        let replaced_root =
                            matches!(
                                event.kind,
                                notify::EventKind::Remove(_)
                                    | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                            ) && event.paths.iter().any(|path| roots.contains(path));
                        if replaced_root {
                            callback_failed.store(true, Ordering::Relaxed);
                        }
                        replaced_root
                            || !matches!(event.kind, notify::EventKind::Access(_))
                                && event.paths.iter().any(|path| accept(path))
                    }
                    Err(_) => {
                        callback_failed.store(true, Ordering::Relaxed);
                        true
                    }
                };
                if relevant {
                    tx.send_modify(|g| *g = g.wrapping_add(1));
                }
            })?;
        for path in paths {
            watcher.watch(
                path,
                if recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )?;
        }
        Ok(Self {
            _watcher: watcher,
            rx,
            failed,
        })
    }

    /// Consume invalidations synchronously when checking a cached resource.
    pub fn take_changed(&mut self) -> bool {
        let changed = self.rx.has_changed().unwrap_or(true);
        self.rx.borrow_and_update();
        changed
    }
    pub fn healthy(&self) -> bool {
        !self.failed.load(Ordering::Relaxed)
    }

    /// Watch the worktree plus linked-worktree refs/index and shared refs.
    pub fn git(path: &Path) -> notify::Result<Self> {
        let mut events = Self::watch(path, true)?;
        let dotgit = path.join(".git");
        let gitdir: PathBuf = match std::fs::read_to_string(&dotgit) {
            Ok(text) => path.join(text.trim().strip_prefix("gitdir: ").unwrap_or(text.trim())),
            Err(_) => dotgit,
        };
        if gitdir.is_dir() {
            events
                ._watcher
                .watch(&gitdir, RecursiveMode::NonRecursive)?;
            let common = std::fs::read_to_string(gitdir.join("commondir"))
                .map(|p| gitdir.join(p.trim()))
                .unwrap_or(gitdir);
            events
                ._watcher
                .watch(&common, RecursiveMode::NonRecursive)?;
            if common.join("refs").is_dir() {
                events
                    ._watcher
                    .watch(&common.join("refs"), RecursiveMode::Recursive)?;
            }
        }
        Ok(events)
    }

    pub async fn changed(&mut self) -> bool {
        self.rx.changed().await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn state_watch_covers_new_comment_dirs_and_atomic_replacements() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut events = FileEvents::state(dir.path()).unwrap();
        for relative in [
            "state.json",
            "comments/session.json",
            "reviewed/session.json",
        ] {
            let target = dir.path().join(relative);
            let temporary = target.with_extension("tmp");
            std::fs::write(&temporary, "updated").unwrap();
            std::fs::rename(&temporary, &target).unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(2), events.changed())
                    .await
                    .unwrap()
            );
        }
        let worktree = dir.path().join("worktrees/session");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join("source.rs"), "unrelated").unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), events.changed())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn replacing_state_child_directory_restores_reconciliation() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut events = FileEvents::state(dir.path()).unwrap();
        std::fs::rename(dir.path().join("comments"), dir.path().join("old-comments")).unwrap();
        std::fs::create_dir(dir.path().join("comments")).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), events.changed())
                .await
                .unwrap()
        );
        assert!(!events.healthy());
    }

    #[tokio::test]
    async fn git_watch_observes_a_real_lockfile() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("Cargo.lock");
        std::fs::write(&target, "old").unwrap();
        let mut events = FileEvents::git(dir.path()).unwrap();
        std::fs::write(&target, "new").unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), events.changed())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn observes_atomic_replacement_and_in_place_edits() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("state.json");
        std::fs::write(&target, "old").unwrap();
        let mut events = FileEvents::watch(dir.path(), true).unwrap();
        let temp = dir.path().join("replacement.tmp");
        std::fs::write(&temp, "new").unwrap();
        std::fs::rename(temp, &target).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), events.changed())
                .await
                .unwrap()
        );
        std::fs::write(target, "changed in place").unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), events.changed())
                .await
                .unwrap()
        );
    }
}
