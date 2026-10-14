//! RECut: Balance Multi-Camera Sound (`recut.balanceMulticam`, RECUT.md).
//!
//! Recordings made for one video rarely agree on level: microphones around -38 LUFS, game sound
//! anywhere from -15 to -26 LUFS. And OBS writes a **mix track** (track 1 = the sum of the others)
//! next to the separate tracks, so a camera that keeps all its tracks plays everything twice.
//! Peak normalisation of the multi-camera clip cannot fix either: it moves the whole mix at once.
//!
//! Here every audio track of the multi-camera source is measured (BS.1770 integrated loudness on
//! samples spread over the recording, without its clip gain), mix tracks are found by least squares
//! (a track that the camera's other tracks add up to), and each track gets a role:
//!
//! | role | does |
//! |---|---|
//! | `voice` | clip gain to the voice target; high-pass 80 Hz, compressor 3:1 |
//! | `game` | clip gain to the voice target minus the game offset; compressor 2.5:1 |
//! | `mixed` | voice and game in one track: halfway between the two; compressor 2.5:1 |
//! | `mute` | track muted (mix tracks and empty tracks by default) |
//! | `keep` | left alone (sound of its own, linked to no camera, by default) |
//!
//! and a hard limiter at -1 dB on the source's Mix. RECut manages the first high-pass,
//! single-band compressor and hard limiter of each track (and of the Mix): they are updated, not
//! stacked, when the command runs again. Everything is one undo step.

use filmcraft_audio_dsp::LoudnessMeter;
use filmcraft_project::{EffectInstance, ItemId, ParamValue, Project, Sequence, TrackId, TrackItem, find_effect};
use filmcraft_render::SourceProvider;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, item_p};
use crate::{EngineError, Result, Session};

pub const BALANCE: &str = "recut.balanceMulticam";

/// Measurement windows per track, and their length.
const WINDOWS: i64 = 12;
const WINDOW_S: i64 = 6;
/// Every n-th sample is kept for mix detection (a linear decimation: a sum stays a sum).
const DECIMATE: usize = 6;
/// A track the camera's other tracks explain this well is a mix of them.
const MIX_EXPLAINED: f64 = 0.95;
/// Below this a track counts as empty.
const SILENT_LUFS: f64 = -60.0;
/// Clip gain limits.
const MAX_GAIN_DB: f64 = 40.0;

pub const DEFAULT_VOICE_LUFS: f64 = -20.0;
pub const DEFAULT_GAME_OFFSET_DB: f64 = 8.0;

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: BALANCE,
        label: "Balance Multi-Camera Sound…",
        menu: &["Clip"],
        shortcut: None,
        params: r#"{"sequence":id?,"analyze":bool?,"roles":[{"track":id,"role":"voice|game|mixed|mute|keep","lufs":f64?}]?,"voiceLufs":f64?,"gameOffset":f64?,"compress":bool?,"limit":bool?}"#,
        enabled: has_multicam_target,
        run: balance,
        journal: true,
    }]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Voice,
    Game,
    Mixed,
    Mute,
    Keep,
}

impl Role {
    pub fn from_name(s: &str) -> Option<Role> {
        Some(match s.to_ascii_lowercase().as_str() {
            "voice" | "voix" => Role::Voice,
            "game" | "jeu" => Role::Game,
            "mixed" | "mix" => Role::Mixed,
            "mute" | "off" => Role::Mute,
            "keep" => Role::Keep,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Role::Voice => "voice",
            Role::Game => "game",
            Role::Mixed => "mixed",
            Role::Mute => "mute",
            Role::Keep => "keep",
        }
    }
    pub const ALL: [Role; 5] = [Role::Voice, Role::Game, Role::Mixed, Role::Mute, Role::Keep];
}

/// What the analysis found about one audio track of the source.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub track: u64,
    pub name: String,
    /// Camera (angle) the track belongs to; None = sound of its own.
    pub camera: Option<usize>,
    pub camera_name: String,
    /// Integrated loudness without clip gain (None = silent).
    pub lufs: Option<f64>,
    /// The tracks this one is the sum of (an OBS mix track), empty otherwise.
    pub mix_of: Vec<u64>,
    pub suggested: Role,
}

/// The multi-camera source a command acts on: `sequence`, else a selected multi-camera clip, else
/// the one at the playhead.
pub fn target_source(s: &Session, p: &Value) -> Option<ItemId> {
    let is_mc = |id: ItemId| s.project.sequence(id).is_some_and(|q| q.multicam.is_some());
    if let Some(id) = item_p(p, "sequence") {
        return Some(id).filter(|i| is_mc(*i));
    }
    let q = s.active_sequence()?;
    let selected = s.state.selection.iter().filter_map(|c| q.find_item(*c)).map(|(_, it)| it.item).find(|i| is_mc(*i));
    selected.or_else(|| crate::multicam::multicam_clip_at(s, s.playhead()).map(|c| c.1.item))
}

fn has_multicam_target(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if target_source(s, &Value::Null).is_some() { Ok(()) } else { Err("select a multi-camera clip".into()) }
}

/// Measure every audio track of `source` and suggest a role for each.
pub fn analyze(source: &Sequence, sources: &dyn SourceProvider) -> Vec<TrackInfo> {
    let sr = source.settings.sample_rate.clamp(8_000, 192_000);
    let dur = source.duration();
    let mc = source.cameras();
    let window = Tick(WINDOW_S * TICKS_PER_SECOND).min(Tick(dur.0 / WINDOWS.max(1)).max(Tick(TICKS_PER_SECOND / 10)));
    let starts: Vec<Tick> = (0..WINDOWS).map(|k| Tick(((dur.0 as i128) * (2 * k as i128 + 1) / (2 * WINDOWS as i128)) as i64) - Tick(window.0 / 2)).collect();
    let mut infos = Vec::new();
    let mut signals: Vec<Vec<f32>> = Vec::new();
    for t in &source.audio_tracks {
        let camera = mc.cameras.iter().position(|c| c.audio_tracks.contains(&t.id));
        let camera_name = camera.and_then(|c| mc.cameras.get(c)).map(|c| c.name.clone()).unwrap_or_default();
        let (lufs, decimated) = measure(t.items.as_slice(), &starts, window, sr, sources);
        let lufs = lufs.filter(|l| l.is_finite() && *l > SILENT_LUFS);
        infos.push(TrackInfo { track: t.id.0, name: t.name.clone(), camera, camera_name, lufs, mix_of: Vec::new(), suggested: Role::Keep });
        signals.push(decimated);
    }
    // per camera with a picture: mix tracks, then roles
    for (ci, cam) in mc.cameras.iter().enumerate() {
        let idx: Vec<usize> = infos.iter().enumerate().filter(|(_, i)| i.camera == Some(ci)).map(|(k, _)| k).collect();
        if cam.video_track.is_none() {
            continue;
        }
        let mut muted: Vec<usize> = Vec::new();
        for &k in &idx {
            if infos.get(k).is_some_and(|i| i.lufs.is_none()) {
                muted.push(k);
            }
        }
        for &k in &idx {
            if muted.contains(&k) {
                continue;
            }
            let others: Vec<usize> = idx.iter().copied().filter(|o| *o != k && !muted.contains(o)).collect();
            let (Some(y), false) = (signals.get(k), others.is_empty()) else { continue };
            let xs: Vec<&[f32]> = others.iter().filter_map(|o| signals.get(*o).map(Vec::as_slice)).collect();
            if let Some((explained, coef)) = least_squares(y, &xs)
                && explained >= MIX_EXPLAINED
            {
                let parts: Vec<u64> = others.iter().zip(&coef).filter(|(_, c)| c.abs() > 0.1).filter_map(|(o, _)| infos.get(*o).map(|i| i.track)).collect();
                if let Some(i) = infos.get_mut(k) {
                    i.mix_of = parts;
                }
                muted.push(k);
            }
        }
        let playing: Vec<usize> = idx.iter().copied().filter(|k| !muted.contains(k)).collect();
        let quietest = playing.iter().copied().min_by(|a, b| {
            let l = |k: &usize| infos.get(*k).and_then(|i| i.lufs).unwrap_or(0.0);
            l(a).total_cmp(&l(b))
        });
        for &k in &idx {
            let role = if muted.contains(&k) {
                Role::Mute
            } else if playing.len() == 1 {
                Role::Mixed
            } else if Some(k) == quietest {
                Role::Voice
            } else {
                Role::Game
            };
            if let Some(i) = infos.get_mut(k) {
                i.suggested = role;
            }
        }
    }
    infos
}

/// Integrated loudness (without clip gain) of a track over the windows, and its decimated mono
/// signal for mix detection.
fn measure(items: &[TrackItem], starts: &[Tick], window: Tick, sr: u32, sources: &dyn SourceProvider) -> (Option<f64>, Vec<f32>) {
    // a mono track is metered as one channel, as BS.1770 meters a mono programme (Essential Sound's
    // Auto-Match does the same): metering its two identical sides would read 3 LU too loud
    let mono_track = items.iter().find_map(|i| filmcraft_render::audio::clip_is_mono(i, sources)).unwrap_or(false);
    let mut meter = LoudnessMeter::new(sr as f64, if mono_track { 1 } else { 2 });
    let mut heard = false;
    let mut dec = Vec::new();
    let n = window.to_units_floor(sr as i64).clamp(1, 60 * 192_000) as usize;
    for st in starts {
        let a0 = st.to_units_floor(sr as i64);
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        for it in items.iter().filter(|i| i.enabled && i.start.to_units_floor(sr as i64) < a0 + n as i64 && i.end().to_units_floor(sr as i64) > a0) {
            let Some([cl, cr]) = filmcraft_render::audio::clip_signal(it, a0, n, sr, sources) else { continue };
            // measured without the clip gain, so the result does not depend on an earlier balance
            let undo = filmcraft_render::audio::db_to_gain(-it.gain_db);
            let mono = filmcraft_render::audio::clip_is_mono(it, sources).unwrap_or(false);
            for (k, (a, b)) in cl.iter().zip(&cr).enumerate() {
                let (a, b) = if mono { (0.5 * (a + b), 0.5 * (a + b)) } else { (*a, *b) };
                if let (Some(x), Some(y)) = (l.get_mut(k), r.get_mut(k)) {
                    *x += a * undo;
                    *y += b * undo;
                }
            }
            heard = true;
        }
        if mono_track {
            let m: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
            meter.process(&[&m]);
        } else {
            meter.process(&[&l, &r]);
        }
        dec.extend(l.iter().zip(&r).step_by(DECIMATE).map(|(a, b)| 0.5 * (a + b)));
    }
    (heard.then(|| meter.integrated()), dec)
}

/// Fit `y ≈ Σ c·x` by least squares: (share of y's energy explained, coefficients). None when the
/// system is degenerate.
fn least_squares(y: &[f32], xs: &[&[f32]]) -> Option<(f64, Vec<f64>)> {
    let m = xs.len();
    if m == 0 || m > 32 {
        return None;
    }
    let len = xs.iter().map(|x| x.len()).chain(std::iter::once(y.len())).min()?;
    let mut a = vec![vec![0.0f64; m + 1]; m];
    for i in 0..m {
        for j in 0..m {
            let (xi, xj) = (xs.get(i)?, xs.get(j)?);
            a[i][j] = (0..len).map(|t| f64::from(xi[t]) * f64::from(xj[t])).sum();
        }
        let xi = xs.get(i)?;
        a[i][m] = (0..len).map(|t| f64::from(xi[t]) * f64::from(y[t])).sum();
    }
    // Gaussian elimination with partial pivoting
    for col in 0..m {
        let piv = (col..m).max_by(|p, q| a[*p][col].abs().total_cmp(&a[*q][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        for row in 0..m {
            if row != col {
                let f = a[row][col] / a[col][col];
                for k in col..=m {
                    a[row][k] -= f * a[col][k];
                }
            }
        }
    }
    let coef: Vec<f64> = (0..m).map(|i| a[i][m] / a[i][i]).collect();
    let energy: f64 = y[..len].iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    if energy <= 1e-12 {
        return None;
    }
    let residual: f64 = (0..len)
        .map(|t| {
            let fit: f64 = xs.iter().zip(&coef).map(|(x, c)| c * f64::from(x[t])).sum();
            let e = f64::from(y[t]) - fit;
            e * e
        })
        .sum();
    Some(((1.0 - residual / energy).clamp(0.0, 1.0), coef))
}

/// Set (or add first) the effect `id` among `effects` to `params`.
fn manage(effects: &mut Vec<EffectInstance>, id: &str, params: &[(&str, f64)], at_front: bool) {
    let pos = effects.iter().position(|e| e.effect == id);
    let pos = match pos {
        Some(p) => p,
        None => {
            let Some(def) = find_effect(id) else { return };
            if at_front {
                effects.insert(0, def.instance());
                0
            } else {
                effects.push(def.instance());
                effects.len() - 1
            }
        }
    };
    if let Some(e) = effects.get_mut(pos) {
        e.enabled = true;
        for (k, v) in params {
            if let Some(p) = e.params.get_mut(*k) {
                p.value = ParamValue::Float(*v);
                p.keyframes.clear();
            }
        }
    }
}

fn balance(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = BALANCE;
    let src = target_source(s, p).ok_or_else(|| bad(cmd, "select a multi-camera clip (or pass `sequence`)"))?;
    let source = s.project.sequence(src).ok_or_else(|| bad(cmd, "no such sequence"))?.clone();
    let voice = f64_p(p, "voiceLufs").filter(|v| v.is_finite()).unwrap_or(DEFAULT_VOICE_LUFS).clamp(-40.0, -6.0);
    let offset = f64_p(p, "gameOffset").filter(|v| v.is_finite()).unwrap_or(DEFAULT_GAME_OFFSET_DB).clamp(0.0, 30.0);
    let compress = bool_p(p, "compress").unwrap_or(true);
    let limit = bool_p(p, "limit").unwrap_or(true);
    // requested roles (and loudness already measured by the caller)
    let mut wanted: Vec<(TrackId, Role, Option<f64>)> = Vec::new();
    if let Some(list) = p.get("roles").and_then(Value::as_array) {
        for r in list {
            let track = r.get("track").and_then(Value::as_u64).map(TrackId).ok_or_else(|| bad(cmd, "each role needs `track`"))?;
            if !source.audio_tracks.iter().any(|t| t.id == track) {
                return Err(bad(cmd, format!("track {} is not an audio track of the source", track.0)));
            }
            let name = r.get("role").and_then(Value::as_str).unwrap_or_default();
            let role = Role::from_name(name).ok_or_else(|| bad(cmd, format!("unknown role `{name}` (voice, game, mixed, mute, keep)")))?;
            let lufs = r.get("lufs").and_then(Value::as_f64).filter(|l| l.is_finite() && (-120.0..=20.0).contains(l));
            wanted.push((track, role, lufs));
        }
    }
    let analyze_only = bool_p(p, "analyze").unwrap_or(false);
    let need_measure = analyze_only || source.audio_tracks.iter().any(|t| wanted.iter().find(|w| w.0 == t.id).is_none_or(|w| w.2.is_none()));
    let infos = if need_measure {
        let provider = s.media.provider(s.project.clone(), s.services.clone());
        analyze(&source, &provider)
    } else {
        Vec::new()
    };
    let settings = json!({"voiceLufs": voice, "gameOffset": offset, "compress": compress, "limit": limit});
    if analyze_only {
        return Ok(json!({"sequence": src.0, "tracks": infos, "settings": settings}));
    }
    // the plan: role and loudness of every track
    let plan: Vec<(TrackId, Role, Option<f64>)> = source
        .audio_tracks
        .iter()
        .map(|t| {
            let info = infos.iter().find(|i| i.track == t.id.0);
            match wanted.iter().find(|w| w.0 == t.id) {
                Some((_, role, lufs)) => (t.id, *role, lufs.or_else(|| info.and_then(|i| i.lufs))),
                None => (t.id, info.map_or(Role::Keep, |i| i.suggested), info.and_then(|i| i.lufs)),
            }
        })
        .collect();
    let target = |role: Role| match role {
        Role::Voice => Some(voice),
        Role::Game => Some(voice - offset),
        Role::Mixed => Some(voice - offset / 2.0),
        Role::Mute | Role::Keep => None,
    };
    let mut report = Vec::new();
    s.edit("Balance Multi-Camera Sound", |pr: &mut Project, _| {
        let q = pr.sequence_mut(src).ok_or(EngineError::NoSequence)?;
        for (track, role, lufs) in &plan {
            let Some(t) = q.audio_tracks.iter_mut().find(|t| t.id == *track) else { continue };
            match role {
                Role::Keep => {}
                Role::Mute => t.muted = true,
                _ => {
                    t.muted = false;
                    let gain = match (target(*role), lufs) {
                        (Some(tg), Some(l)) => Some((tg - l).clamp(-MAX_GAIN_DB, MAX_GAIN_DB)),
                        _ => None,
                    };
                    if let Some(g) = gain {
                        for it in &mut t.items {
                            it.gain_db = g;
                        }
                    }
                    if *role == Role::Voice {
                        manage(&mut t.effects, "highpass", &[("cutoff", 80.0)], true);
                    }
                    if compress {
                        let tg = target(*role).unwrap_or(voice);
                        let (ratio, attack, release) = if *role == Role::Voice { (3.0, 10.0, 150.0) } else { (2.5, 20.0, 200.0) };
                        manage(
                            &mut t.effects,
                            "single_band_compressor",
                            &[("threshold", (tg - 6.0).clamp(-60.0, 0.0)), ("ratio", ratio), ("attack", attack), ("release", release), ("output", 0.0)],
                            false,
                        );
                    }
                    report.push(json!({"track": track.0, "role": role.name(), "lufs": lufs, "gainDb": gain}));
                    continue;
                }
            }
            report.push(json!({"track": track.0, "role": role.name()}));
        }
        if limit {
            manage(&mut q.master_effects, "hard_limiter", &[("max", -1.0), ("boost", 0.0)], false);
        }
        q.check().map_err(EngineError::Other)?;
        Ok(())
    })?;
    Ok(json!({"sequence": src.0, "tracks": report, "settings": settings}))
}
