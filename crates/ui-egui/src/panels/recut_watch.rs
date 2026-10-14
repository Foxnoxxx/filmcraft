//! RECut: File ▸ Watch Folder… (`recut.watchFolder`, RECUT.md).
//!
//! The watch service lives in egui's temporary memory (no field added to FilmCraft's UI state):
//! every few seconds a background thread looks at the folder ([`filmcraft_engine::recut_watch`]),
//! and the files it finds complete are imported on the next frame, each into its bin. The dialog
//! sets the folder; its texts are in French (RECut's language), not in the catalogs.
//!
//! `recut.watchFolder` without params opens the dialog; with params it sets the service directly
//! (agents): `dir`, `enabled`, `recursive`, `existing` (also import what is already there, once),
//! and for tests `persist` (false: do not save the setting), `quietSeconds`, `pollSeconds`.
//!
//! Automation ids: `watch.dir`, `watch.choose`, `watch.enabled`, `watch.recursive`,
//! `watch.existing`, `watch.close`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use egui::{Align2, RichText};
use filmcraft_engine::recut_watch::{self, Poll, WatchConfig, Watcher};
use filmcraft_project::BinEntry;
use serde_json::{Value, json};

use crate::FilmcraftApp;

pub const WATCH: &str = "recut.watchFolder";

#[derive(Default)]
struct Shared {
    busy: bool,
    watcher: Watcher,
    result: Option<Poll>,
    /// A folder chosen in the native picker, for the next frame.
    picked: Option<String>,
}

#[derive(Clone)]
struct Service {
    cfg: WatchConfig,
    /// Where the setting is saved (None: not saved).
    path: Option<PathBuf>,
    /// Import what is already there on the next first look.
    existing: bool,
    quiet: Duration,
    poll: f64,
    shared: Arc<Mutex<Shared>>,
    last: f64,
    waiting: usize,
    imported: usize,
    open: bool,
    error: String,
}

impl Service {
    fn load() -> Self {
        let path = recut_watch::config_path();
        let cfg = path.as_deref().map(recut_watch::load_config).unwrap_or_default();
        Service {
            cfg,
            path,
            existing: false,
            quiet: recut_watch::QUIET,
            poll: recut_watch::POLL.as_secs_f64(),
            shared: Arc::default(),
            last: f64::NEG_INFINITY,
            waiting: 0,
            imported: 0,
            open: false,
            error: String::new(),
        }
    }

    /// Start over with a new setting (saved when it has a place).
    fn set(&mut self, cfg: WatchConfig) {
        if cfg != self.cfg {
            self.cfg = cfg;
            self.shared.lock().unwrap_or_else(PoisonError::into_inner).watcher = Watcher::default();
            self.last = f64::NEG_INFINITY;
            self.waiting = 0;
            self.error = match &self.path {
                Some(p) => recut_watch::save_config(p, &self.cfg).err().map(|e| format!("réglage non enregistré : {e}")).unwrap_or_default(),
                None => String::new(),
            };
        }
    }
}

fn service_id() -> egui::Id {
    egui::Id::new("recut-watch-service")
}

fn get(ctx: &egui::Context) -> Service {
    ctx.data_mut(|d| d.get_temp::<Service>(service_id())).unwrap_or_else(Service::load)
}

fn put(ctx: &egui::Context, s: Service) {
    ctx.data_mut(|d| d.insert_temp(service_id(), s));
}

/// File ▸ Watch Folder…: the dialog, or the setting given as params. None for other ids.
pub fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != WATCH {
        return None;
    }
    let mut sv = get(ctx);
    let Some(m) = params.as_object().filter(|m| !m.is_empty()) else {
        sv.open = true;
        put(ctx, sv);
        return Some(Ok(json!({"dialog": "watchFolder"})));
    };
    if m.get("persist").and_then(Value::as_bool) == Some(false) {
        sv.path = None;
    }
    let secs = |k: &str| m.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 600.0));
    if let Some(q) = secs("quietSeconds") {
        sv.quiet = Duration::from_secs_f64(q);
    }
    if let Some(p) = secs("pollSeconds") {
        sv.poll = p.max(0.1);
    }
    let mut cfg = sv.cfg.clone();
    if let Some(d) = m.get("dir").and_then(Value::as_str) {
        cfg.dir = d.to_string();
    }
    if let Some(b) = m.get("recursive").and_then(Value::as_bool) {
        cfg.recursive = b;
    }
    if let Some(b) = m.get("enabled").and_then(Value::as_bool) {
        cfg.enabled = b;
    }
    if cfg.enabled && !Path::new(&cfg.dir).is_dir() {
        let e = format!("dossier introuvable : {}", cfg.dir);
        app.ui.status = e.clone();
        put(ctx, sv);
        return Some(Err(e));
    }
    sv.existing = m.get("existing").and_then(Value::as_bool).unwrap_or(sv.existing);
    sv.set(cfg);
    let r = json!({"dir": sv.cfg.dir, "enabled": sv.cfg.enabled, "recursive": sv.cfg.recursive});
    put(ctx, sv);
    Some(Ok(r))
}

/// Every frame: look at the folder every few seconds (in the background) and import what is ready.
pub fn tick(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let mut sv = get(ctx);
    let (ready, picked) = {
        let mut sh = sv.shared.lock().unwrap_or_else(PoisonError::into_inner);
        (sh.result.take(), sh.picked.take())
    };
    if let Some(dir) = picked {
        let mut cfg = sv.cfg.clone();
        cfg.dir = dir;
        cfg.enabled = true;
        sv.set(cfg);
    }
    if let Some(p) = ready {
        sv.waiting = p.waiting;
        if !p.ready.is_empty() {
            sv.imported += import(app, Path::new(&sv.cfg.dir), p.ready);
        }
    }
    if sv.cfg.enabled && !sv.cfg.dir.is_empty() {
        let now = ctx.input(|i| i.time);
        let busy = sv.shared.lock().unwrap_or_else(PoisonError::into_inner).busy;
        if !busy && now - sv.last >= sv.poll {
            sv.last = now;
            look(&mut sv);
        }
        ctx.request_repaint_after(Duration::from_secs_f64(sv.poll.clamp(0.1, 10.0)));
    }
    put(ctx, sv);
}

/// One look at the folder on a background thread.
#[cfg(not(target_arch = "wasm32"))]
fn look(sv: &mut Service) {
    let shared = sv.shared.clone();
    let (root, recursive, existing, quiet) = (PathBuf::from(&sv.cfg.dir), sv.cfg.recursive, sv.existing, sv.quiet);
    sv.existing = false;
    let mut watcher = {
        let mut sh = shared.lock().unwrap_or_else(PoisonError::into_inner);
        sh.busy = true;
        std::mem::take(&mut sh.watcher)
    };
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| watcher.poll(&root, recursive, existing, std::time::SystemTime::now(), quiet)));
        let mut sh = shared.lock().unwrap_or_else(PoisonError::into_inner);
        sh.watcher = watcher;
        sh.result = r.ok();
        sh.busy = false;
    });
}

#[cfg(target_arch = "wasm32")]
fn look(_: &mut Service) {}

/// Import `files` (those not in the project yet), each into its bin; the number imported.
fn import(app: &mut FilmcraftApp, root: &Path, files: Vec<PathBuf>) -> usize {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for f in files.iter().filter(|f| !recut_watch::in_project(&app.session.project, f)) {
        let bin = recut_watch::bin_for(root, f);
        let path = f.to_string_lossy().into_owned();
        match groups.iter_mut().find(|g| g.0 == bin) {
            Some(g) => g.1.push(path),
            None => groups.push((bin, vec![path])),
        }
    }
    let mut done = 0;
    let mut lines = Vec::new();
    for (bin, paths) in groups {
        let existing = app.session.project.root.children.iter().find_map(|e| match e {
            BinEntry::Bin(b) if b.name == bin => Some(b.id.0),
            _ => None,
        });
        let id = match existing {
            Some(id) => Some(id),
            None => app.session.execute("file.newBin", json!({"name": bin})).ok().and_then(|v| v["bin"].as_u64()),
        };
        let n = paths.len();
        match app.session.execute("file.import", json!({"paths": paths, "bin": id})) {
            Ok(_) => {
                done += n;
                lines.push(format!("{n} dans « {bin} »"));
            }
            Err(e) => lines.push(format!("échec dans « {bin} » : {e}")),
        }
    }
    if !lines.is_empty() {
        app.ui.status = format!("Dossier de veille : rushs importés, {}", lines.join(" ; "));
    }
    done
}

/// Draw the dialog when it is open.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let mut sv = get(ctx);
    if !sv.open {
        return;
    }
    let mut cfg = sv.cfg.clone();
    let mut existing = sv.existing;
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    let mut push = |id: &str, r: &egui::Response, label: &str| elems.push((id.to_string(), r.rect, label.to_string()));
    let (mut close, mut choose) = (false, false);
    egui::Window::new("Dossier de veille")
        .id(egui::Id::new("recut-watch"))
        .collapsible(false)
        .resizable(false)
        .default_width(520.0)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.label(
                RichText::new(
                    "RECut surveille ce dossier (celui d'OBS, ou celui où vous dézippez les POV reçues) et importe chaque nouveau rush \
                     dès qu'il est complet, rangé dans un chutier au nom de son sous-dossier (sinon « Rushs <date> »).",
                )
                .weak()
                .small(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut cfg.dir).hint_text("Aucun dossier choisi").desired_width(380.0));
                push("watch.dir", &r, "dossier");
                let r = ui.button("Choisir…");
                push("watch.choose", &r, "Choisir…");
                choose = r.clicked();
            });
            let r = ui.checkbox(&mut cfg.enabled, "Surveiller ce dossier");
            push("watch.enabled", &r, "Surveiller");
            let r = ui.checkbox(&mut cfg.recursive, "Inclure les sous-dossiers");
            push("watch.recursive", &r, "Sous-dossiers");
            let r = ui.checkbox(&mut existing, "Importer aussi les fichiers déjà présents (une fois)");
            push("watch.existing", &r, "Déjà présents");
            ui.add_space(4.0);
            let state = if !cfg.enabled || cfg.dir.is_empty() {
                "Veille arrêtée.".to_string()
            } else if !Path::new(&cfg.dir).is_dir() {
                "Dossier introuvable.".to_string()
            } else {
                format!("En veille · {} fichier(s) en cours d'écriture · {} importé(s) depuis l'ouverture de RECut", sv.waiting, sv.imported)
            };
            ui.label(RichText::new(state).strong());
            if !sv.error.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(0xE0, 0x50, 0x50), &sv.error);
            }
            ui.add_space(6.0);
            let r = ui.button("Fermer");
            push("watch.close", &r, "Fermer");
            close = r.clicked();
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if crate::widgets::escape_closes(ctx) {
        close = true;
    }
    if existing && !sv.existing {
        // a fresh first look imports what is there
        sv.shared.lock().unwrap_or_else(PoisonError::into_inner).watcher = Watcher::default();
        sv.last = f64::NEG_INFINITY;
    }
    sv.existing = existing;
    sv.set(cfg);
    if choose {
        let shared = sv.shared.clone();
        app.pick_ui(crate::native_dialogs::FileDialog::Folder { at: None }, move |_, paths| {
            if let Some(d) = paths.into_iter().next() {
                shared.lock().unwrap_or_else(PoisonError::into_inner).picked = Some(d);
            }
        });
    }
    if close {
        sv.open = false;
    }
    put(ctx, sv);
}
