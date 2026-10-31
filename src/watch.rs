use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    thread,
    time::{Duration, SystemTime},
};

const INTERVAL: Duration = Duration::from_millis(250);
type Snapshot = BTreeMap<PathBuf, Option<(u64, SystemTime)>>;

pub fn run(input: &Path) -> Result<()> {
    let input = input
        .canonicalize()
        .with_context(|| format!("cannot open {}", input.display()))?;
    anyhow::ensure!(input.is_dir(), "{} is not a folder", input.display());
    let output = input.join("_site");
    crate::check_output(&output)?;
    let address = listen_address(std::env::var("MEMOC_BIND").ok().as_deref())?;
    let listener = TcpListener::bind(address)
        .with_context(|| format!("cannot serve at {address}; check that the port is available"))?;
    let mut watcher = Watcher::new(&input)?;
    let gate = Arc::new(RwLock::new(()));
    rebuild(&input, &gate);
    let server_gate = Arc::clone(&gate);
    thread::spawn(move || crate::server::serve(listener, output, server_gate));
    println!(
        "Watching {}\nhttp://{address}/\nRefresh the browser after edits. Press Ctrl+C to stop.",
        input.display()
    );
    let mut previous_error = None;
    loop {
        thread::sleep(INTERVAL);
        match watcher.ready(&input) {
            Ok(ready) => {
                previous_error = None;
                if ready {
                    rebuild(&input, &gate);
                }
            }
            Err(error) => {
                let message = format!("{error:#}");
                if previous_error.as_ref() != Some(&message) {
                    eprintln!("memoc: {message}; still watching");
                    previous_error = Some(message);
                }
            }
        }
    }
}

fn listen_address(value: Option<&str>) -> Result<SocketAddr> {
    value
        .unwrap_or("127.0.0.1:4000")
        .parse()
        .context("MEMOC_BIND must be an IP address and port, for example 0.0.0.0:4000")
}

fn rebuild(input: &Path, gate: &RwLock<()>) {
    // Requests cannot observe the short gap while the output directory is replaced.
    let _guard = gate.write().unwrap();
    if let Err(error) = crate::build(input) {
        eprintln!(
            "memoc: {error:#}\nBuild failed; keeping the previous site and watching for changes."
        );
    }
}

struct Watcher {
    observed: Snapshot,
    pending: bool,
}

impl Watcher {
    fn new(input: &Path) -> Result<Self> {
        Ok(Self {
            observed: snapshot(input)?,
            pending: false,
        })
    }

    fn ready(&mut self, input: &Path) -> Result<bool> {
        let current = snapshot(input)?;
        if current != self.observed {
            self.observed = current;
            self.pending = true;
            return Ok(false);
        }
        // Wait for one quiet interval, coalescing editor save/rename bursts.
        Ok(std::mem::take(&mut self.pending))
    }
}

fn snapshot(input: &Path) -> Result<Snapshot> {
    fn visit(folder: &Path, entries: &mut Snapshot) -> Result<()> {
        for entry in
            fs::read_dir(folder).with_context(|| format!("cannot watch {}", folder.display()))?
        {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(['.', '_']) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                // Directory timestamps would trigger rebuilds for ignored output and editor files.
                entries.insert(path.clone(), None);
                visit(&path, entries)?;
            } else if kind.is_file() {
                let metadata = entry.metadata()?;
                entries.insert(path, Some((metadata.len(), metadata.modified()?)));
            }
        }
        Ok(())
    }
    let mut entries = BTreeMap::new();
    visit(input, &mut entries)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Folder;

    #[test]
    fn local_runs_use_loopback_and_containers_can_bind_all_interfaces() {
        assert_eq!(listen_address(None).unwrap().to_string(), "127.0.0.1:4000");
        assert_eq!(
            listen_address(Some("0.0.0.0:4000")).unwrap().to_string(),
            "0.0.0.0:4000"
        );
        assert!(listen_address(Some("invalid")).is_err());
    }

    #[test]
    fn coalesces_changes_and_detects_create_edit_rename_delete_and_assets() {
        let input = Folder::new();
        input.write("README.md", "# Home");
        let mut watcher = Watcher::new(&input.0).unwrap();
        assert!(!watcher.ready(&input.0).unwrap());
        input.write("page.md", "# Page");
        assert!(!watcher.ready(&input.0).unwrap());
        input.write("page.md", "# Updated page");
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
        assert!(!watcher.ready(&input.0).unwrap());
        fs::rename(input.0.join("page.md"), input.0.join("renamed.md")).unwrap();
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
        fs::remove_file(input.0.join("renamed.md")).unwrap();
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
        input.write("images/photo.png", [0, 1, 2]);
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
        fs::create_dir(input.0.join("empty")).unwrap();
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
    }

    #[test]
    fn ignores_build_outputs_hidden_files_and_symlinks() {
        let input = Folder::new();
        input.write("README.md", "# Home");
        let mut watcher = Watcher::new(&input.0).unwrap();
        crate::build(&input.0).unwrap();
        input.write(".git/config", "ignored");
        input.write("_draft/page.md", "ignored");
        input.write("_memoc-build/partial", "ignored");
        #[cfg(unix)]
        std::os::unix::fs::symlink(input.0.join("README.md"), input.0.join("link.md")).unwrap();
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(!watcher.ready(&input.0).unwrap());
    }

    #[test]
    fn recovers_after_failed_initial_build_and_later_bad_edits() {
        let input = Folder::new();
        input.write("README.md", "# Broken\n[Missing](missing.md)");
        let mut watcher = Watcher::new(&input.0).unwrap();
        let gate = RwLock::new(());
        rebuild(&input.0, &gate);
        assert!(!input.0.join("_site").exists());
        input.write("missing.md", "# Now present");
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
        rebuild(&input.0, &gate);
        let before = fs::read(input.0.join("_site/index.html")).unwrap();
        input.write("README.md", "# Broken again\n[Absent](absent.md)");
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
        rebuild(&input.0, &gate);
        assert_eq!(fs::read(input.0.join("_site/index.html")).unwrap(), before);
        assert!(!watcher.ready(&input.0).unwrap());
        input.write("README.md", "# Recovered");
        assert!(!watcher.ready(&input.0).unwrap());
        assert!(watcher.ready(&input.0).unwrap());
        rebuild(&input.0, &gate);
        assert!(
            fs::read_to_string(input.0.join("_site/index.html"))
                .unwrap()
                .contains("Recovered")
        );
    }
}
