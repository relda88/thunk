use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::app::paths::AppPaths;
use crate::app::AppContext;
use crate::core::config::Config;
use crate::tui::worker::{run_worker, WorkerCmd, WorkerReply};

pub(crate) struct BackendHandle {
    pub cmd_tx: mpsc::Sender<WorkerCmd>,
    pub reply_rx: mpsc::Receiver<WorkerReply>,
}

pub(crate) fn spawn_backend(app: AppContext, config: &Config, paths: &AppPaths) -> BackendHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let (reply_tx, reply_rx) = mpsc::channel::<WorkerReply>();
    thread::spawn(move || run_worker(app, cmd_rx, reply_tx));

    {
        let watcher_cmd_tx = cmd_tx.clone();
        let watch_root = paths.project_root.clone();
        thread::spawn(move || {
            let (watcher_tx, watcher_rx) = mpsc::channel::<notify::Result<notify::Event>>();
            let mut watcher =
                match notify::RecommendedWatcher::new(watcher_tx, notify::Config::default()) {
                    Ok(w) => w,
                    Err(_) => return,
                };
            use notify::Watcher as _;
            if watcher
                .watch(&watch_root, notify::RecursiveMode::Recursive)
                .is_err()
            {
                return;
            }
            for res in watcher_rx {
                match res {
                    Ok(event) => {
                        for path in event.paths {
                            if should_rebuild(&path, &watch_root) {
                                if watcher_cmd_tx.send(WorkerCmd::RebuildFile(path)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(_) => return,
                }
            }
        });
    }

    {
        let proactive_cmd_tx = cmd_tx.clone();
        let proactive_enabled = config.proactive.enabled;
        let interval_secs: u64 = config.proactive.interval_secs;
        if proactive_enabled {
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(interval_secs));
                let _ = proactive_cmd_tx.send(WorkerCmd::ProactiveScan);
            });
        }
    }

    BackendHandle { cmd_tx, reply_rx }
}

fn should_rebuild(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.extension().map_or(false, |e| e == "rs")
        && path.starts_with(root)
        && !path
            .components()
            .any(|c| c.as_os_str() == "target" || c.as_os_str() == ".git")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn should_rebuild_filter() {
        let root = PathBuf::from("/proj");

        assert!(super::should_rebuild(&root.join("src/lib.rs"), &root));
        assert!(super::should_rebuild(&root.join("src/tui/app.rs"), &root));
        assert!(!super::should_rebuild(
            &root.join("target/debug/build/foo.rs"),
            &root
        ));
        assert!(!super::should_rebuild(
            &root.join(".git/hooks/post-commit"),
            &root
        ));
        assert!(!super::should_rebuild(&root.join("src/main.toml"), &root));
        assert!(!super::should_rebuild(
            &PathBuf::from("/other/src/lib.rs"),
            &root
        ));
    }
}
