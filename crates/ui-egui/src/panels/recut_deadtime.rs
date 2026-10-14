//! RECut: the Mark Dead Time dialog (`recut.markDeadTime`, RECUT.md).
//!
//! Opening it decodes the voice tracks of the analysed sequence once, in the background, with a
//! progress bar ([`filmcraft_engine::recut_deadtime::measure`]). After that every setting and every
//! track ticked recomputes the dead times at once: the dialog lists them (a click puts the playhead
//! on one) and Lay Markers runs the engine command with those ranges (nothing is decoded twice).
//! Remove Marked Dead Time is a plain command; here it only gets a status-bar line.
//!
//! The dialog lives in egui's temporary memory, not in `UiState`, so RECut adds no field to
//! FilmCraft's UI state. Its texts are in French (RECut's language), not in the catalogs.
//!
//! Automation ids: `deadtime.track.<id>` (checkboxes), `deadtime.min`, `deadtime.range`,
//! `deadtime.after`, `deadtime.before`, `deadtime.range.<n>`, `deadtime.stop`, `deadtime.cancel`,
//! `deadtime.ok`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align2, Color32, RichText};
use filmcraft_engine::recut_deadtime::{self, Levels, MARK, REMOVE, Settings};
use filmcraft_project::ItemId;
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Default)]
struct Job {
    progress: f32,
    stop: bool,
    done: Option<Result<Levels, String>>,
}

/// What the current settings give.
struct Computed {
    shares: Vec<(u64, f64)>,
    /// In the analysed sequence's time (what the command takes) and in the edit's.
    ranges: Vec<TimeRange>,
    edit: Vec<TimeRange>,
}

#[derive(Clone)]
struct Draft {
    source: ItemId,
    name: String,
    job: Arc<Mutex<Job>>,
    levels: Option<Arc<Levels>>,
    chosen: Vec<u64>,
    settings: Settings,
    cache: Option<(Settings, Vec<u64>, Arc<Computed>)>,
    error: String,
}

fn draft_id() -> egui::Id {
    egui::Id::new("recut-deadtime-dialog")
}

/// "1 h 02 min 05 s", "4 min 32 s", "7,4 s".
pub fn duration_fr(t: Tick) -> String {
    let s = t.0.max(0) as f64 / TICKS_PER_SECOND as f64;
    if s < 60.0 {
        return format!("{s:.1} s").replace('.', ",");
    }
    let s = s.round() as i64;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h} h {m:02} min {sec:02} s") } else { format!("{m} min {sec:02} s") }
}

/// "1:02:05" / "4:32".
fn clock(t: Tick) -> String {
    let s = (t.0.max(0) / TICKS_PER_SECOND).max(0);
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

/// Sequence ▸ Mark Dead Time… without params opens the dialog; Remove Marked Dead Time runs and
/// says what it did. None for other ids.
pub fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let bare = params.as_object().is_none_or(|m| m.is_empty());
    if id == REMOVE && bare {
        let r = app.session.execute(REMOVE, json!({})).map_err(|e| e.to_string());
        app.ui.status = match &r {
            Ok(v) => {
                let t = Tick((v["seconds"].as_f64().unwrap_or(0.0) * TICKS_PER_SECOND as f64) as i64);
                format!("Temps morts supprimés : {} passage(s), {}", v["removed"].as_u64().unwrap_or(0), duration_fr(t))
            }
            Err(e) => e.clone(),
        };
        return Some(r);
    }
    if id != MARK || !bare {
        return None;
    }
    let r = open(app, ctx);
    if let Err(e) = &r {
        app.ui.status = e.clone();
    }
    Some(r)
}

fn open(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    let source = recut_deadtime::analysed_sequence(&app.session, &Value::Null).ok_or("ouvrez une séquence")?;
    if recut_deadtime::spans(&app.session, source).is_empty() {
        return Err("la séquence active ne contient aucun clip multicam de cette source".into());
    }
    let project = app.session.project.clone();
    let seq = project.sequence(source).ok_or("no such sequence")?.clone();
    let name = project.item(source).map(|i| i.name.clone()).unwrap_or_default();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let job = Arc::new(Mutex::new(Job::default()));
    let shared = job.clone();
    let repaint = ctx.clone();
    let run = move || {
        let mut progress = |f: f32| {
            let mut j = shared.lock().unwrap_or_else(PoisonError::into_inner);
            j.progress = f;
            repaint.request_repaint();
            !j.stop
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| recut_deadtime::measure(&seq, &provider, &mut progress)));
        let done = match r {
            Ok(Some(levels)) => Ok(levels),
            Ok(None) => Err("analyse arrêtée".to_string()),
            Err(_) => Err("l'analyse des pistes a échoué".to_string()),
        };
        shared.lock().unwrap_or_else(PoisonError::into_inner).done = Some(done);
        repaint.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(run);
    #[cfg(target_arch = "wasm32")]
    run();
    let draft = Draft { source, name, job, levels: None, chosen: Vec::new(), settings: Settings::default(), cache: None, error: String::new() };
    ctx.data_mut(|d| d.insert_temp(draft_id(), draft));
    Ok(json!({"dialog": "deadTime", "source": source.0}))
}

fn compute(app: &FilmcraftApp, d: &Draft, levels: &Levels) -> Computed {
    let mut acts = Vec::new();
    let mut shares = Vec::new();
    for t in &levels.tracks {
        let act = recut_deadtime::activity(t, d.settings.range_db);
        shares.push((t.track, recut_deadtime::speaking_share(&act)));
        if d.chosen.contains(&t.track) {
            acts.push(act);
        }
    }
    let ranges = if acts.is_empty() { Vec::new() } else { recut_deadtime::dead_ranges(&acts, levels.hops, &d.settings) };
    let spans = recut_deadtime::spans(&app.session, d.source);
    let edit = recut_deadtime::to_edit(&spans, &ranges, app.session.sequence_rate());
    Computed { shares, ranges, edit }
}

/// Draw the dialog when it is open.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = ctx.data(|m| m.get_temp::<Draft>(draft_id())) else { return };
    let mut progress = 0.0f32;
    if d.levels.is_none() && d.error.is_empty() {
        let mut j = d.job.lock().unwrap_or_else(PoisonError::into_inner);
        progress = j.progress;
        match j.done.take() {
            Some(Ok(levels)) => {
                d.chosen = levels.tracks.iter().filter(|t| t.default).map(|t| t.track).collect();
                d.levels = Some(Arc::new(levels));
            }
            Some(Err(e)) => d.error = e,
            None => {}
        }
    }
    // recompute when a setting or a ticked track changed
    let mut computed: Option<Arc<Computed>> = None;
    if let Some(levels) = d.levels.clone() {
        let fresh = d.cache.as_ref().is_some_and(|(st, ch, _)| *st == d.settings && *ch == d.chosen);
        if !fresh {
            d.cache = Some((d.settings, d.chosen.clone(), Arc::new(compute(app, &d, &levels))));
        }
        computed = d.cache.as_ref().map(|c| c.2.clone());
    }
    let accent = app.tokens.accent;
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    let mut push = |id: String, r: &egui::Response, label: &str| elems.push((id, r.rect, label.to_string()));
    let mut action: Option<&str> = None;
    let mut seek: Option<Tick> = None;
    let ready = computed.is_some();
    egui::Window::new("Temps morts")
        .id(egui::Id::new("recut-deadtime"))
        .collapsible(false)
        .resizable(false)
        .default_width(560.0)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.label(RichText::new(&d.name).strong());
            ui.label(
                RichText::new(
                    "RECut écoute les pistes de voix et repère les passages où personne ne parle. Chacun reçoit un marqueur rose « Temps mort » : \
                     supprimez ceux des passages à garder, puis Séquence ▸ Supprimer les temps morts marqués.",
                )
                .weak()
                .small(),
            );
            ui.add_space(6.0);
            if !d.error.is_empty() {
                ui.colored_label(Color32::from_rgb(0xE0, 0x50, 0x50), &d.error);
            }
            let n = computed.as_ref().map_or(0, |c| c.edit.len());
            match (d.levels.clone(), computed.clone()) {
                (Some(levels), Some(c)) => {
                    ui.label(RichText::new("Pistes écoutées").strong());
                    egui::Grid::new("recut-deadtime-tracks").striped(true).num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
                        for t in &levels.tracks {
                            let mut on = d.chosen.contains(&t.track);
                            let label = if t.camera.is_empty() { t.name.clone() } else { format!("{} · {}", t.camera, t.name) };
                            let r = ui.checkbox(&mut on, label.as_str());
                            push(format!("deadtime.track.{}", t.track), &r, &label);
                            if r.changed() {
                                if on {
                                    d.chosen.push(t.track);
                                } else {
                                    d.chosen.retain(|x| *x != t.track);
                                }
                            }
                            let share = c.shares.iter().find(|s| s.0 == t.track).map_or(0.0, |s| s.1);
                            ui.label(if t.stats.is_none() { "silencieuse".to_string() } else { format!("son actif {:.0} % du temps", share * 100.0) });
                            if share > 0.9 && on {
                                ui.label(RichText::new("bruit continu ? décochez-la").small().color(Color32::from_rgb(0xE0, 0xA0, 0x40)));
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                        }
                    });
                    ui.separator();
                    egui::Grid::new("recut-deadtime-settings").num_columns(2).show(ui, |ui| {
                        let mut row = |ui: &mut egui::Ui, label: &str, id: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, suffix: &str| {
                            ui.label(label);
                            let r = ui.add(egui::DragValue::new(v).range(range).speed(0.05).max_decimals(1).suffix(suffix));
                            push(id.into(), &r, label);
                            ui.end_row();
                        };
                        row(ui, "Silence d'au moins", "deadtime.min", &mut d.settings.min_seconds, 0.5..=600.0, " s");
                        row(ui, "Parole : jusqu'à", "deadtime.range", &mut d.settings.range_db, 6.0..=40.0, " dB sous la voix");
                        row(ui, "Garder après la parole", "deadtime.after", &mut d.settings.keep_after, 0.0..=5.0, " s");
                        row(ui, "Garder avant la parole", "deadtime.before", &mut d.settings.keep_before, 0.0..=5.0, " s");
                    });
                    ui.separator();
                    let total: Tick = c.edit.iter().map(|r| r.duration).fold(Tick::ZERO, |a, b| a + b);
                    let length = app.session.active_sequence().map_or(Tick::ZERO, |q| q.duration());
                    let pct = if length > Tick::ZERO { 100.0 * total.0 as f64 / length.0 as f64 } else { 0.0 };
                    ui.label(
                        RichText::new(format!("{} temps mort(s) · {} sur {} ({pct:.0} %)", c.edit.len(), duration_fr(total), duration_fr(length))).strong(),
                    );
                    if d.chosen.is_empty() {
                        ui.label(RichText::new("Cochez au moins une piste de voix.").weak());
                    }
                    egui::ScrollArea::vertical().max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
                        for (k, r) in c.edit.iter().enumerate() {
                            let text = format!("▶  {} → {}  ·  {}", clock(r.start), clock(r.end()), duration_fr(r.duration));
                            let resp = ui.add(egui::Button::new(text).frame(false));
                            push(format!("deadtime.range.{k}"), &resp, "temps mort");
                            if resp.clicked() {
                                seek = Some(r.start);
                            }
                        }
                    });
                }
                _ if d.error.is_empty() => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Écoute des pistes audio… {:.0} %", progress * 100.0));
                    });
                    ui.add(egui::ProgressBar::new(progress).desired_width(520.0));
                    let r = ui.button("Arrêter l'analyse");
                    push("deadtime.stop".into(), &r, "Arrêter");
                    if r.clicked() {
                        d.job.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
                    }
                }
                _ => {}
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let r = ui.button("Annuler");
                push("deadtime.cancel".into(), &r, "Annuler");
                if r.clicked() {
                    action = Some("cancel");
                }
                let label = RichText::new(format!("Poser les marqueurs ({n})")).color(Color32::WHITE);
                let r = ui.add_enabled(ready && n > 0, egui::Button::new(label).fill(accent));
                push("deadtime.ok".into(), &r, "Poser les marqueurs");
                if r.clicked() {
                    action = Some("ok");
                }
            });
        });
    if let Some(t) = seek {
        app.session.set_playhead(t);
    }
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if crate::widgets::escape_closes(ctx) {
        action = Some("cancel");
    }
    match action {
        Some("cancel") => {
            d.job.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
            ctx.data_mut(|m| m.remove::<Draft>(draft_id()));
            return;
        }
        Some("ok") => {
            let ranges: Vec<[i64; 2]> = computed.as_ref().map(|c| c.ranges.iter().map(|r| [r.start.0, r.end().0]).collect()).unwrap_or_default();
            let p = json!({"source": d.source.0, "ranges": ranges});
            match app.session.execute(MARK, p) {
                Ok(v) => {
                    let t = Tick((v["seconds"].as_f64().unwrap_or(0.0) * TICKS_PER_SECOND as f64) as i64);
                    app.ui.status = format!(
                        "Temps morts marqués : {} ({}). Supprimez les marqueurs des passages à garder, puis Séquence ▸ Supprimer les temps morts marqués.",
                        v["markers"].as_u64().unwrap_or(0),
                        duration_fr(t)
                    );
                    ctx.data_mut(|m| m.remove::<Draft>(draft_id()));
                    return;
                }
                Err(e) => d.error = e.to_string(),
            }
        }
        _ => {}
    }
    ctx.data_mut(|m| m.insert_temp(draft_id(), d));
}
