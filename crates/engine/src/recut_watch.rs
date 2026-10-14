//! RECut: the watch folder (File ▸ Watch Folder…, RECUT.md).
//!
//! RECut keeps an eye on one folder (OBS's recording folder, or where transfers are unzipped) and
//! imports each new media file once it is complete: its size and date have not changed for
//! [`QUIET`], over two looks at least. A recording still being written, a download or an unzip in
//! progress keeps changing, so it waits. Files already there when watching starts are left alone,
//! unless asked; a file already in the project is never imported twice.
//!
//! Each file goes to a bin named after its first subfolder under the watched folder (a transfer's
//! folder, a project's folder), else "Rushs <date>" for an OBS recording ("2026-05-20 15-40-35.mp4")
//! or "Rushs".
//!
//! The setting is kept in `recut-watch.json` in RECut's data folder. The app scans in the
//! background ([`Watcher::poll`] only reads folder listings) and imports on its own thread.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use filmcraft_project::{MediaRef, Project};
use serde::{Deserialize, Serialize};

/// How long a file must stay unchanged before it is imported.
pub const QUIET: Duration = Duration::from_secs(10);
/// How often the app looks.
pub const POLL: Duration = Duration::from_secs(3);
/// Deepest subfolder looked into, and most entries listed per look.
const MAX_DEPTH: usize = 6;
const MAX_ENTRIES: usize = 50_000;
/// The file holding the setting, in RECut's data folder.
pub const CONFIG_FILE: &str = "recut-watch.json";

/// Media RECut imports from the watched folder.
pub const MEDIA_EXTENSIONS: &[&str] =
    &["mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "mts", "m2ts", "ts", "wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "png", "jpg", "jpeg"];

/// The watch folder setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchConfig {
    pub dir: String,
    pub enabled: bool,
    /// Also watch the subfolders.
    pub recursive: bool,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self { dir: String::new(), enabled: false, recursive: true }
    }
}

/// Where the setting is kept.
pub fn config_path() -> Option<PathBuf> {
    crate::autosave::default_data_dir().map(|d| d.join(CONFIG_FILE))
}

/// The setting saved at `path` (the default when there is none, or it is unreadable).
pub fn load_config(path: &Path) -> WatchConfig {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Save the setting at `path`.
pub fn save_config(path: &Path, c: &WatchConfig) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Whether RECut imports `path` (by its extension; hidden and temporary files are skipped).
pub fn is_media(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.starts_with('.') || name.starts_with("~$") {
        return false;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    MEDIA_EXTENSIONS.contains(&ext.as_str())
}

/// The recording time OBS puts in its file names ("2026-05-20 15-40-35.mp4", also with `_` or `T`
/// between date and time, or a prefix): (date "2026-05-20", seconds since midnight).
pub fn obs_time(name: &str) -> Option<(String, u32)> {
    let b = name.as_bytes();
    let digits = |s: usize, n: usize| -> Option<u32> {
        let part = b.get(s..s + n)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(part).ok()?.parse().ok()
    };
    // "YYYY-MM-DD hh-mm-ss": 19 bytes
    for s in 0..b.len().saturating_sub(18) {
        let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) =
            (digits(s, 4), digits(s + 5, 2), digits(s + 8, 2), digits(s + 11, 2), digits(s + 14, 2), digits(s + 17, 2))
        else {
            continue;
        };
        let sep = |k: usize, ok: &[u8]| b.get(s + k).is_some_and(|c| ok.contains(c));
        if sep(4, b"-")
            && sep(7, b"-")
            && sep(10, b" _T")
            && sep(13, b"-.:")
            && sep(16, b"-.:")
            && (1..=12).contains(&mo)
            && (1..=31).contains(&d)
            && h < 24
            && mi < 60
            && se < 60
        {
            return Some((format!("{y:04}-{mo:02}-{d:02}"), h * 3600 + mi * 60 + se));
        }
    }
    None
}

/// The bin a file from the watched folder goes to (see the module docs).
pub fn bin_for(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let mut parts = rel.components();
    if let (Some(first), Some(_)) = (parts.next(), parts.next()) {
        return first.as_os_str().to_string_lossy().into_owned();
    }
    match file.file_name().and_then(|n| n.to_str()).and_then(obs_time) {
        Some((date, _)) => format!("Rushs {date}"),
        None => "Rushs".into(),
    }
}

/// Whether the project already holds the media file `path`.
pub fn in_project(project: &Project, path: &Path) -> bool {
    project.items.values().any(|i| i.as_media().is_some_and(|m| matches!(&m.media, MediaRef::File { path: p } if Path::new(p) == path)))
}

#[derive(Clone, Copy, Debug)]
struct Seen {
    size: u64,
    modified: Option<SystemTime>,
    /// When the size or date last changed (or the file appeared).
    since: SystemTime,
    looks: u32,
    done: bool,
}

/// What the watcher found in one look.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Poll {
    /// Complete files to import now.
    pub ready: Vec<PathBuf>,
    /// New files still being written.
    pub waiting: usize,
}

/// Remembers the files of the watched folder between looks.
#[derive(Clone, Debug, Default)]
pub struct Watcher {
    seen: BTreeMap<PathBuf, Seen>,
    primed: bool,
}

impl Watcher {
    /// Look at `root` once (see the module docs): a file is ready once unchanged for `quiet` (normally
    /// [`QUIET`]) over two looks. On the first look, the files already there count as done unless
    /// `import_existing`.
    pub fn poll(&mut self, root: &Path, recursive: bool, import_existing: bool, now: SystemTime, quiet: Duration) -> Poll {
        let files = list(root, recursive);
        let mut out = Poll::default();
        for (path, size, modified) in &files {
            let Some(s) = self.seen.get_mut(path) else {
                let done = !self.primed && !import_existing;
                self.seen.insert(path.clone(), Seen { size: *size, modified: *modified, since: now, looks: 1, done });
                if !done {
                    out.waiting += 1;
                }
                continue;
            };
            if s.done {
                continue;
            }
            s.looks = s.looks.saturating_add(1);
            if s.size != *size || s.modified != *modified {
                (s.size, s.modified, s.since) = (*size, *modified, now);
                out.waiting += 1;
                continue;
            }
            // a date in the future (clock skew) does not hold a file back for ever
            let calm = |t: SystemTime| now.duration_since(t).map_or(true, |d| d >= quiet);
            if *size > 0 && s.looks >= 2 && calm(s.since) && modified.is_none_or(|m| m > now || calm(m)) {
                s.done = true;
                out.ready.push(path.clone());
            } else {
                out.waiting += 1;
            }
        }
        // a file that went away (an OBS remux replaces its .mkv) is forgotten
        let present: BTreeSet<&PathBuf> = files.iter().map(|f| &f.0).collect();
        self.seen.retain(|p, _| present.contains(p));
        self.primed = true;
        out
    }
}

/// The media files under `root`: (path, size, date).
fn list(root: &Path, recursive: bool) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut entries = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            entries += 1;
            if entries > MAX_ENTRIES {
                return out;
            }
            let Ok(ft) = e.file_type() else { continue };
            let path = e.path();
            if ft.is_dir() {
                if recursive && depth < MAX_DEPTH && !e.file_name().to_string_lossy().starts_with('.') {
                    stack.push((path, depth + 1));
                }
            } else if ft.is_file() && is_media(&path) {
                let Ok(meta) = e.metadata() else { continue };
                out.push((path, meta.len(), meta.modified().ok()));
            }
        }
    }
    out.sort();
    out
}
