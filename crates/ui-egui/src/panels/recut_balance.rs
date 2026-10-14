//! RECut: the Balance Multi-Camera Sound dialog (`recut.balanceMulticam`, RECUT.md).
//!
//! Opening it measures the multi-camera source's audio tracks in the background
//! ([`filmcraft_engine::recut_balance::analyze`]); each track then shows its camera, its loudness,
//! whether it is an OBS mix of other tracks, and a role the user can change. Apply runs the engine
//! command with those roles and the measured loudness (nothing is measured twice).
//!
//! The dialog lives in egui's temporary memory, not in `UiState`, so RECut adds no field to
//! FilmCraft's UI state. Its texts are in French (RECut's language), not in the catalogs.
//!
//! Automation ids: `balance.<track>.<role>` (role buttons), `balance.voice`, `balance.offset`,
//! `balance.compress`, `balance.limit`, `balance.cancel`, `balance.ok`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align2, Color32, RichText};
use filmcraft_engine::recut_balance::{self, BALANCE, Role, TrackInfo};
use filmcraft_project::ItemId;
use serde_json::{Value, json};

use crate::FilmcraftApp;

type Analysis = Arc<Mutex<Option<Result<Vec<TrackInfo>, String>>>>;

#[derive(Clone)]
struct Draft {
    source: ItemId,
    name: String,
    analysis: Analysis,
    rows: Vec<(TrackInfo, Role)>,
    voice: f64,
    offset: f64,
    compress: bool,
    limit: bool,
    error: String,
}

fn draft_id() -> egui::Id {
    egui::Id::new("recut-balance-dialog")
}

fn role_label(r: Role) -> &'static str {
    match r {
        Role::Voice => "Voix",
        Role::Game => "Jeu",
        Role::Mixed => "Voix + jeu",
        Role::Mute => "Coupée",
        Role::Keep => "Inchangée",
    }
}

/// Clip ▸ Balance Multi-Camera Sound… without params opens the dialog. None for other ids.
pub fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != BALANCE || !params.as_object().is_none_or(|m| m.is_empty()) {
        return None;
    }
    Some(open(app, ctx))
}

fn open(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    let source = recut_balance::target_source(&app.session, &Value::Null).ok_or("select a multi-camera clip")?;
    let project = app.session.project.clone();
    let seq = project.sequence(source).ok_or("no such sequence")?.clone();
    let name = project.item(source).map(|i| i.name.clone()).unwrap_or_default();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let analysis: Analysis = Arc::new(Mutex::new(None));
    let out = analysis.clone();
    let run = move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| recut_balance::analyze(&seq, &provider)))
            .map_err(|_| "l'analyse des pistes a échoué".to_string());
        *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(run);
    #[cfg(target_arch = "wasm32")]
    run();
    let draft = Draft {
        source,
        name,
        analysis,
        rows: Vec::new(),
        voice: recut_balance::DEFAULT_VOICE_LUFS,
        offset: recut_balance::DEFAULT_GAME_OFFSET_DB,
        compress: true,
        limit: true,
        error: String::new(),
    };
    ctx.data_mut(|d| d.insert_temp(draft_id(), draft));
    Ok(json!({"dialog": "balance", "sequence": source.0}))
}

/// Draw the dialog when it is open.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = ctx.data(|m| m.get_temp::<Draft>(draft_id())) else { return };
    if d.rows.is_empty() && d.error.is_empty() {
        let done = d.analysis.lock().unwrap_or_else(PoisonError::into_inner).take();
        match done {
            Some(Ok(rows)) => d.rows = rows.into_iter().map(|i| (i.clone(), i.suggested)).collect(),
            Some(Err(e)) => d.error = e,
            None => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }
    let accent = app.tokens.accent;
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    let mut push = |id: String, r: &egui::Response, label: &str| elems.push((id, r.rect, label.to_string()));
    let mut action: Option<&str> = None;
    let ready = !d.rows.is_empty();
    egui::Window::new("Équilibrer le son multicam")
        .id(egui::Id::new("recut-balance"))
        .collapsible(false)
        .resizable(false)
        .default_width(620.0)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.label(RichText::new(&d.name).strong());
            ui.label(
                RichText::new("Chaque piste est mesurée en LUFS puis calée sur sa cible : la voix au même niveau sur toutes les POV, le jeu en dessous.")
                    .weak()
                    .small(),
            );
            ui.add_space(6.0);
            if !d.error.is_empty() {
                ui.colored_label(Color32::from_rgb(0xE0, 0x50, 0x50), &d.error);
            } else if !ready {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Analyse des pistes audio…");
                });
            } else {
                egui::Grid::new("recut-balance-grid").striped(true).num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                    for h in ["Caméra", "Piste", "Niveau", "Rôle"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    let names: Vec<(u64, String)> = d.rows.iter().map(|(i, _)| (i.track, i.name.clone())).collect();
                    for (info, role) in &mut d.rows {
                        ui.label(if info.camera.is_some() { info.camera_name.as_str() } else { "Son seul" });
                        ui.label(&info.name);
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(info.lufs.map_or_else(|| "vide".to_string(), |l| format!("{l:.1} LUFS"))).extend());
                            if !info.mix_of.is_empty() {
                                let parts: Vec<&str> = info.mix_of.iter().filter_map(|t| names.iter().find(|n| n.0 == *t).map(|n| n.1.as_str())).collect();
                                ui.label(RichText::new(format!("mixage OBS de {}", parts.join(" + "))).weak().small());
                            }
                        });
                        ui.horizontal(|ui| {
                            for r in Role::ALL {
                                let resp = ui.selectable_value(role, r, role_label(r));
                                push(format!("balance.{}.{}", info.track, r.name()), &resp, role_label(r));
                            }
                        });
                        ui.end_row();
                    }
                });
            }
            ui.separator();
            egui::Grid::new("recut-balance-settings").num_columns(2).show(ui, |ui| {
                ui.label("Niveau de la voix");
                let r = ui.add(egui::DragValue::new(&mut d.voice).range(-40.0..=-6.0).speed(0.2).suffix(" LUFS"));
                push("balance.voice".into(), &r, "voice");
                ui.end_row();
                ui.label("Jeu sous la voix");
                let r = ui.add(egui::DragValue::new(&mut d.offset).range(0.0..=30.0).speed(0.2).suffix(" dB"));
                push("balance.offset".into(), &r, "offset");
                ui.end_row();
            });
            let r = ui.checkbox(&mut d.compress, "Compresseur sur chaque piste (niveau plus régulier)");
            push("balance.compress".into(), &r, "compress");
            let r = ui.checkbox(&mut d.limit, "Limiteur à -1 dB sur le mixage");
            push("balance.limit".into(), &r, "limit");
            ui.label(RichText::new("À l'export, activez la normalisation de la sonie à -14 LUFS pour YouTube.").weak().small());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let r = ui.button("Annuler");
                push("balance.cancel".into(), &r, "Annuler");
                if r.clicked() {
                    action = Some("cancel");
                }
                let r = ui.add_enabled(ready, egui::Button::new(RichText::new("Appliquer").color(Color32::WHITE)).fill(accent));
                push("balance.ok".into(), &r, "Appliquer");
                if r.clicked() {
                    action = Some("ok");
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if crate::widgets::escape_closes(ctx) {
        action = Some("cancel");
    }
    match action {
        Some("cancel") => {
            ctx.data_mut(|m| m.remove::<Draft>(draft_id()));
            return;
        }
        Some("ok") => {
            let roles: Vec<Value> = d.rows.iter().map(|(i, r)| json!({"track": i.track, "role": r.name(), "lufs": i.lufs})).collect();
            let p = json!({"sequence": d.source.0, "roles": roles, "voiceLufs": d.voice, "gameOffset": d.offset, "compress": d.compress, "limit": d.limit});
            match app.session.execute(BALANCE, p) {
                Ok(v) => {
                    let n = v["tracks"].as_array().map_or(0, |t| t.iter().filter(|x| x["gainDb"].is_number()).count());
                    app.ui.status = format!("Son multicam équilibré : {n} piste(s) calée(s)");
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
