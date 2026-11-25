//! Watch folders: a recipe applied to every picture that appears in a folder.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use pixelit_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::clock;
use crate::paths::Paths;
use crate::scan::is_image;
use crate::store;

/// Version of `watch.json`.
pub const WATCH_VERSION: u32 = 1;

/// How often pending files are looked at.
const TICK: Duration = Duration::from_millis(250);

/// Outputs remembered so that they are not processed again.
const OUTPUTS_REMEMBERED: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchRule {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub recursive: bool,
    /// Recipe applied to new files.
    pub recipe_id: String,
    pub enabled: bool,
    /// Quiet time before a new file is taken, in milliseconds: a file being
    /// copied keeps changing and is left alone until it settles.
    pub settle_ms: u32,
}

impl Default for WatchRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            folder: String::new(),
            recursive: false,
            recipe_id: String::new(),
            enabled: true,
            settle_ms: 1500,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WatchRules {
    pub rules: Vec<WatchRule>,
}

impl WatchRules {
    pub fn load(paths: &Paths) -> Self {
        store::load_or_default(&paths.watch_rules(), WATCH_VERSION, store::no_migration)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        store::save_json(&paths.watch_rules(), WATCH_VERSION, self)
    }

    /// Adds a rule or replaces the one with the same id.
    pub fn upsert(&mut self, mut rule: WatchRule) -> WatchRule {
        if rule.id.is_empty() {
            rule.id = format!(
                "w{:x}{:06x}",
                clock::unix_now(),
                fastrand::u32(..0x00FF_FFFF)
            );
        }
        rule.settle_ms = rule.settle_ms.clamp(200, 600_000);
        match self.rules.iter_mut().find(|known| known.id == rule.id) {
            Some(known) => *known = rule.clone(),
            None => self.rules.push(rule.clone()),
        }
        rule
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|rule| rule.id != id);
        self.rules.len() != before
    }
}

/// Called with a rule and a new file of its folder. Returns the files it
/// wrote, which the watcher then ignores.
pub type Handler = Box<dyn Fn(&WatchRule, &Path) -> Vec<PathBuf> + Send>;

struct Pending {
    rule: usize,
    last_event: Instant,
    size: u64,
}

/// Running watchers. Dropping the value stops them.
pub struct WatchService {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    outputs: Arc<Mutex<HashSet<PathBuf>>>,
    _watcher: RecommendedWatcher,
}

fn rule_of(rules: &[WatchRule], path: &Path) -> Option<usize> {
    rules.iter().position(|rule| {
        let folder = Path::new(&rule.folder);
        if rule.recursive {
            path.starts_with(folder)
        } else {
            path.parent() == Some(folder)
        }
    })
}

/// Whether the file can be opened for reading and has stopped growing.
fn settled(path: &Path, known_size: u64) -> Option<bool> {
    let size = fs::metadata(path).ok()?.len();
    if size == 0 || size != known_size {
        return Some(false);
    }
    Some(fs::File::open(path).is_ok())
}

impl WatchService {
    /// Starts watching the folders of the enabled rules.
    pub fn start(rules: &[WatchRule], handler: Handler) -> Result<Self> {
        let rules: Vec<WatchRule> = rules
            .iter()
            .filter(|rule| rule.enabled && Path::new(&rule.folder).is_dir())
            .cloned()
            .collect();
        let failed = |error: notify::Error| Error::new("watchFailed").detail(error);
        let (events, incoming) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(events).map_err(failed)?;
        for rule in &rules {
            let mode = if rule.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher
                .watch(Path::new(&rule.folder), mode)
                .map_err(|error| failed(error).with("path", &rule.folder))?;
        }
        let outputs = Arc::new(Mutex::new(HashSet::new()));
        let (stop, stopped) = mpsc::channel::<()>();
        let thread_outputs = Arc::clone(&outputs);
        let thread = thread::Builder::new()
            .name(String::from("watch-folders"))
            .spawn(move || {
                let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
                loop {
                    match stopped.try_recv() {
                        Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    match incoming.recv_timeout(TICK) {
                        Ok(Ok(event)) => note_event(&rules, &thread_outputs, &mut pending, &event),
                        Ok(Err(error)) => tracing::warn!(%error, "watch error"),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    dispatch(&rules, &thread_outputs, &mut pending, &handler);
                }
            })
            .map_err(|error| Error::new("watchFailed").detail(error))?;
        Ok(Self {
            stop,
            thread: Some(thread),
            outputs,
            _watcher: watcher,
        })
    }

    /// Tells the watcher that the application wrote this file itself.
    pub fn ignore(&self, path: &Path) {
        remember(&self.outputs, path.to_path_buf());
    }
}

impl Drop for WatchService {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn remember(outputs: &Mutex<HashSet<PathBuf>>, path: PathBuf) {
    let mut outputs = outputs.lock().unwrap_or_else(PoisonError::into_inner);
    if outputs.len() >= OUTPUTS_REMEMBERED {
        outputs.clear();
    }
    outputs.insert(path);
}

fn note_event(
    rules: &[WatchRule],
    outputs: &Mutex<HashSet<PathBuf>>,
    pending: &mut HashMap<PathBuf, Pending>,
    event: &Event,
) {
    if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
        return;
    }
    for path in &event.paths {
        if !is_image(path) {
            continue;
        }
        if outputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(path)
        {
            continue;
        }
        let Some(rule) = rule_of(rules, path) else {
            continue;
        };
        let size = fs::metadata(path).map_or(0, |metadata| metadata.len());
        pending.insert(
            path.clone(),
            Pending {
                rule,
                last_event: Instant::now(),
                size,
            },
        );
    }
}

fn dispatch(
    rules: &[WatchRule],
    outputs: &Mutex<HashSet<PathBuf>>,
    pending: &mut HashMap<PathBuf, Pending>,
    handler: &Handler,
) {
    let now = Instant::now();
    let mut ready = Vec::new();
    pending.retain(|path, entry| {
        let rule = &rules[entry.rule];
        let quiet = Duration::from_millis(u64::from(rule.settle_ms));
        if now.duration_since(entry.last_event) < quiet {
            return true;
        }
        match settled(path, entry.size) {
            // The file is gone.
            None => false,
            Some(true) => {
                ready.push((path.clone(), entry.rule));
                false
            }
            Some(false) => {
                // Still being written: wait another quiet period.
                entry.size = fs::metadata(path).map_or(0, |metadata| metadata.len());
                entry.last_event = now;
                true
            }
        }
    });
    for (path, rule) in ready {
        // The source itself is remembered too: a recipe that rewrites the
        // file in place must not start an endless loop.
        remember(outputs, path.clone());
        for output in handler(&rules[rule], &path) {
            remember(outputs, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_stored() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let mut rules = WatchRules::load(&paths);
        let rule = rules.upsert(WatchRule {
            name: "Inbox".into(),
            folder: "C:/inbox".into(),
            settle_ms: 1,
            ..WatchRule::default()
        });
        assert!(!rule.id.is_empty());
        assert_eq!(rule.settle_ms, 200);
        rules.save(&paths).unwrap();
        assert_eq!(
            WatchRules::load(&paths).rules.as_slice(),
            std::slice::from_ref(&rule)
        );
        assert!(rules.remove(&rule.id));
    }

    #[test]
    fn rule_matches_its_folder() {
        let rules = [
            WatchRule {
                folder: "/a".into(),
                recursive: false,
                ..WatchRule::default()
            },
            WatchRule {
                folder: "/b".into(),
                recursive: true,
                ..WatchRule::default()
            },
        ];
        assert_eq!(rule_of(&rules, Path::new("/a/x.png")), Some(0));
        assert_eq!(rule_of(&rules, Path::new("/a/deep/x.png")), None);
        assert_eq!(rule_of(&rules, Path::new("/b/deep/x.png")), Some(1));
        assert_eq!(rule_of(&rules, Path::new("/c/x.png")), None);
    }

    #[test]
    fn new_file_reaches_the_handler() {
        let folder = tempfile::tempdir().unwrap();
        let watched = folder.path().canonicalize().unwrap();
        let rule = WatchRule {
            id: "w1".into(),
            folder: watched.display().to_string(),
            settle_ms: 200,
            ..WatchRule::default()
        };
        let (sender, receiver) = mpsc::channel();
        let service = WatchService::start(
            &[rule],
            Box::new(move |rule, path| {
                let _ = sender.send((rule.id.clone(), path.to_path_buf()));
                Vec::new()
            }),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        fs::write(watched.join("notes.txt"), b"text").unwrap();
        fs::write(watched.join("new.png"), b"picture bytes").unwrap();
        let (id, path) = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(id, "w1");
        assert_eq!(path.file_name().unwrap(), "new.png");
        // The text file is never reported.
        assert!(receiver.recv_timeout(Duration::from_millis(600)).is_err());
        drop(service);
    }
}
