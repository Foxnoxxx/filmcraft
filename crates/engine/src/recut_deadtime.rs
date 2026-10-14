//! RECut: dead time (`recut.markDeadTime`, `recut.removeDeadTime`, RECUT.md).
//!
//! A *dead time* is a stretch where nobody talks. It is found on the voice tracks of the
//! multi-camera source behind the edit (each player's microphone), or on the audio tracks of the
//! active sequence when it holds no multi-camera clip:
//!
//! 1. every track is decoded at 16 kHz, high-passed (rumble, desk knocks) and measured in 20 ms
//!    hops ([`measure`]);
//! 2. a hop is *speech* on a track when it is at most `rangeDb` (20 dB) under the track's voice
//!    level (98th percentile of the hops above digital silence) and 6 dB over its noise floor (the
//!    commonest level among its quieter half; ignored on a track gated to digital silence by OBS's
//!    noise gate or suppression). A track with less than 9 dB between floor and voice has no speech
//!    to find: a steady loud sound (a game track) talks all along, so it never marks anything dead;
//!    a quiet one (a microphone nobody used) is silent. Blips shorter than 0.12 s are dropped and
//!    gaps shorter than 0.3 s inside speech are closed ([`activity`]). On Rémi's OBS recordings
//!    (2026-05-20) the voice level is around −35 dBFS, pauses 10–20 dB under it and the room noise
//!    around −60 dBFS;
//! 3. a stretch with no speech on any chosen track, at least `minSeconds` long, is dead time, minus
//!    a margin after the speech before it and before the speech after it ([`dead_ranges`]).
//!
//! Mark Dead Time lays one range marker named "Temps mort" over each dead time *in the edit*
//! (through the multi-camera clips, so a re-cut edit is followed) and replaces the markers of an
//! earlier run. The user deletes the markers of the stretches to keep, then Remove Dead Time
//! ripple-deletes what the remaining markers cover, on every unlocked track. Each is one undo step.

use filmcraft_project::{ItemId, Label, Marker, MarkerId, MarkerKind, Sequence, Track};
use filmcraft_render::SourceProvider;
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, f64_p, has_seq, item_p};
use crate::recut_balance::Role;
use crate::{EngineError, Result, Session};

pub const MARK: &str = "recut.markDeadTime";
pub const REMOVE: &str = "recut.removeDeadTime";
/// The name of the markers Mark Dead Time lays (and Remove Dead Time acts on).
pub const MARKER_NAME: &str = "Temps mort";

/// Analysis sample rate and hop (20 ms).
const RATE: u32 = 16_000;
const HOP: usize = 320;
pub const HOPS_PER_SECOND: i64 = 50;
/// Hops decoded at a time (10 s).
const CHUNK_HOPS: usize = 500;
/// Longest analysis: 24 hours.
const MAX_HOPS: usize = 24 * 3600 * HOPS_PER_SECOND as usize;
/// Below: digital silence.
const SILENT_DB: f32 = -90.0;
/// Below: never speech.
const FLOOR_DB: f32 = -70.0;
/// How far over the noise floor speech must be.
const OVER_NOISE_DB: f32 = 6.0;
/// Under this between floor and voice, a track has no speech to find.
const MIN_DYNAMICS_DB: f32 = 9.0;
/// A steady track louder than this talks all along; a quieter one is silent.
const STEADY_LOUD_DB: f32 = -45.0;
/// Speech gaps closed and blips dropped (hops).
const CLOSE_GAP: usize = 15;
const MIN_BURST: usize = 6;
/// Share of digitally silent hops above which a track counts as gated.
const GATED_SHARE: f64 = 0.2;
/// High-pass corner of the analysis (Hz).
const HIGHPASS_HZ: f32 = 120.0;

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: MARK,
            label: "Mark Dead Time…",
            menu: &["Sequence"],
            shortcut: None,
            params: r#"{"source":id?,"tracks":[id]?,"minSeconds":f64?,"rangeDb":f64?,"keepAfter":f64?,"keepBefore":f64?,"ranges":[[ticks,ticks]]?}"#,
            enabled: has_seq,
            run: mark,
            journal: true,
        },
        CommandSpec {
            id: REMOVE,
            label: "Remove Marked Dead Time",
            menu: &["Sequence"],
            shortcut: None,
            params: "{}",
            enabled: has_dead_markers,
            run: remove,
            journal: true,
        },
    ]
}

/// Detection settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Shortest silence that is dead time (seconds).
    pub min_seconds: f64,
    /// How far under its voice level a track still talks (dB).
    pub range_db: f64,
    /// Kept after the speech before a dead time, and before the speech after it (seconds).
    pub keep_after: f64,
    pub keep_before: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { min_seconds: 3.0, range_db: 20.0, keep_after: 0.5, keep_before: 0.3 }
    }
}

impl Settings {
    /// The settings in `p` (missing, non-finite or out-of-range values fall back or are clamped).
    pub fn from_params(p: &Value) -> Settings {
        let d = Settings::default();
        let get = |k: &str, def: f64, lo: f64, hi: f64| f64_p(p, k).filter(|v| v.is_finite()).unwrap_or(def).clamp(lo, hi);
        Settings {
            min_seconds: get("minSeconds", d.min_seconds, 0.5, 600.0),
            range_db: get("rangeDb", d.range_db, 6.0, 40.0),
            keep_after: get("keepAfter", d.keep_after, 0.0, 5.0),
            keep_before: get("keepBefore", d.keep_before, 0.0, 5.0),
        }
    }
}

/// The 20 ms levels of one track.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackLevels {
    pub track: u64,
    pub name: String,
    /// The camera the track belongs to ("" outside a multi-camera source).
    pub camera: String,
    /// Chosen by default (see [`default_tracks`]).
    pub default: bool,
    /// Level of each hop in dBFS.
    pub db: Vec<f32>,
    /// Noise floor and voice level (dBFS), and whether the track is gated to digital silence.
    /// `None`: the track is silent.
    pub stats: Option<(f32, f32, bool)>,
}

/// The levels of the playing audio tracks of a sequence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    pub tracks: Vec<TrackLevels>,
    /// Hops analysed (the sequence's length).
    pub hops: usize,
}

/// One multi-camera clip of the edit: where its source time lands in the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub source: Tick,
    pub edit: Tick,
    pub duration: Tick,
}

/// Start time of hop `h`.
pub fn hop_tick(h: usize) -> Tick {
    Tick((h as i64).saturating_mul(TICKS_PER_SECOND / HOPS_PER_SECOND))
}

fn playing(t: &Track) -> bool {
    !t.muted && t.items.iter().any(|i| i.enabled)
}

/// The sequence analysed for the edit: the multi-camera source of the clip given as `clip` (or the
/// selected one, or the one at the playhead), else the active sequence itself. `source` names it
/// outright.
pub fn analysed_sequence(s: &Session, p: &Value) -> Option<ItemId> {
    if let Some(id) = item_p(p, "source") {
        return Some(id).filter(|i| s.project.sequence(*i).is_some());
    }
    if let Some((_, src)) = crate::recut_overlays::target(s, p) {
        return Some(src);
    }
    s.state.active_sequence
}

/// Where `analysed`'s time lands in the active sequence: the whole sequence when it is the active
/// one, else its enabled multi-camera clips (at normal speed) on the first video track holding any.
pub fn spans(s: &Session, analysed: ItemId) -> Vec<Span> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    if s.state.active_sequence == Some(analysed) {
        return vec![Span { source: Tick::ZERO, edit: Tick::ZERO, duration: q.duration() }];
    }
    let is_mc = |it: &filmcraft_project::TrackItem| it.item == analysed && it.multicam.is_some_and(|m| m.enabled);
    let Some(track) = q.video_tracks.iter().find(|t| t.items.iter().any(is_mc)) else { return Vec::new() };
    let mut out: Vec<Span> = track
        .items
        .iter()
        .filter(|it| is_mc(it) && (it.speed - 1.0).abs() < 1e-9 && !it.reverse)
        .map(|it| Span { source: it.source_in, edit: it.start, duration: it.duration })
        .collect();
    out.sort_by_key(|sp| sp.edit);
    out
}

/// The tracks chosen by default: per camera, its voice tracks (balanced as voice: a high-pass), else
/// its one playing track, else the tracks the balance analysis takes for voice; outside a
/// multi-camera source, every playing track.
pub fn default_tracks(source: &Sequence, sources: &dyn SourceProvider) -> Vec<u64> {
    let Some(mc) = &source.multicam else { return source.audio_tracks.iter().filter(|t| playing(t)).map(|t| t.id.0).collect() };
    let mut out = Vec::new();
    let mut infos: Option<Vec<crate::recut_balance::TrackInfo>> = None;
    for cam in &mc.cameras {
        let tracks: Vec<&Track> = source.audio_tracks.iter().filter(|t| cam.audio_tracks.contains(&t.id) && playing(t)).collect();
        let voiced: Vec<u64> = tracks.iter().filter(|t| t.effects.iter().any(|e| e.enabled && e.effect == "highpass")).map(|t| t.id.0).collect();
        if !voiced.is_empty() {
            out.extend(voiced);
        } else if tracks.len() == 1 {
            out.extend(tracks.iter().map(|t| t.id.0));
        } else if !tracks.is_empty() {
            let infos = infos.get_or_insert_with(|| crate::recut_balance::analyze(source, sources));
            for t in &tracks {
                let role = infos.iter().find(|i| i.track == t.id.0).map_or(Role::Keep, |i| i.suggested);
                if matches!(role, Role::Voice | Role::Mixed) || (cam.video_track.is_none() && role == Role::Keep) {
                    out.push(t.id.0);
                }
            }
        }
    }
    out
}

/// Measure the playing audio tracks of `source` (see the module docs), several tracks at a time.
/// `progress` gets the share done and returns false to stop (then None).
pub fn measure(source: &Sequence, sources: &dyn SourceProvider, progress: &mut dyn FnMut(f32) -> bool) -> Option<Levels> {
    let defaults = default_tracks(source, sources);
    let cams = source.multicam.as_ref();
    let hops = usize::try_from(source.duration().to_units_floor(HOPS_PER_SECOND)).unwrap_or(0).min(MAX_HOPS);
    let tracks: Vec<&Track> = source.audio_tracks.iter().filter(|t| playing(t)).collect();
    let levels = levels_of(&tracks, hops, sources, progress)?;
    let out = tracks
        .iter()
        .zip(levels)
        .map(|(t, db)| TrackLevels {
            track: t.id.0,
            name: t.name.clone(),
            camera: cams.and_then(|m| m.cameras.iter().find(|c| c.audio_tracks.contains(&t.id))).map(|c| c.name.clone()).unwrap_or_default(),
            default: defaults.contains(&t.id.0),
            stats: stats(&db),
            db,
        })
        .collect();
    Some(Levels { tracks: out, hops })
}

/// The hop levels of each track: one worker per track (as many as the CPU has, less one).
#[cfg(not(target_arch = "wasm32"))]
fn levels_of(tracks: &[&Track], hops: usize, sources: &dyn SourceProvider, progress: &mut dyn FnMut(f32) -> bool) -> Option<Vec<Vec<f32>>> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};
    let chunks = hops.div_ceil(CHUNK_HOPS).max(1);
    let total = (chunks * tracks.len()).max(1);
    let (next, done, exited, stop) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicBool::new(false));
    let results: Mutex<Vec<Option<Vec<f32>>>> = Mutex::new(vec![None; tracks.len()]);
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1)).clamp(1, tracks.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(t) = tracks.get(k) else { break };
                    let tick = || {
                        done.fetch_add(1, Ordering::Relaxed);
                        !stop.load(Ordering::Relaxed)
                    };
                    // a track that cannot be decoded counts as silent, it never stops the others
                    let db = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| track_levels(t, hops, sources, &tick))) {
                        Ok(Some(db)) => db,
                        Ok(None) => break,
                        Err(_) => Vec::new(),
                    };
                    if let Some(slot) = results.lock().unwrap_or_else(PoisonError::into_inner).get_mut(k) {
                        *slot = Some(db);
                    }
                }
                exited.fetch_add(1, Ordering::Relaxed);
            });
        }
        // the caller's thread reports progress until every worker is done (or it is stopped)
        while exited.load(Ordering::Relaxed) < workers {
            if !stop.load(Ordering::Relaxed) && !progress((done.load(Ordering::Relaxed) as f32 / total as f32).min(1.0)) {
                stop.store(true, Ordering::Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    progress(1.0);
    results.into_inner().unwrap_or_else(PoisonError::into_inner).into_iter().collect()
}

/// The hop levels of each track, one after the other (no threads on the web).
#[cfg(target_arch = "wasm32")]
fn levels_of(tracks: &[&Track], hops: usize, sources: &dyn SourceProvider, progress: &mut dyn FnMut(f32) -> bool) -> Option<Vec<Vec<f32>>> {
    let chunks = hops.div_ceil(CHUNK_HOPS).max(1);
    let total = (chunks * tracks.len()).max(1);
    let done = std::cell::Cell::new(0usize);
    let progress = std::cell::RefCell::new(progress);
    let mut out = Vec::new();
    for t in tracks {
        let tick = || {
            done.set(done.get() + 1);
            let mut p = progress.borrow_mut();
            (*p)(done.get() as f32 / total as f32)
        };
        out.push(track_levels(t, hops, sources, &tick)?);
    }
    Some(out)
}

/// The 20 ms levels of one track, decoded 10 s at a time; `tick` is called after each chunk and
/// returns false to stop (then None).
fn track_levels(t: &Track, hops: usize, sources: &dyn SourceProvider, tick: &dyn Fn() -> bool) -> Option<Vec<f32>> {
    let mut db = Vec::with_capacity(hops);
    // one-pole high-pass: y[n] = a·(y[n-1] + x[n] − x[n-1])
    let a = (-2.0 * std::f32::consts::PI * HIGHPASS_HZ / RATE as f32).exp();
    let (mut x1, mut y1) = (0.0f32, 0.0f32);
    for c in 0..hops.div_ceil(CHUNK_HOPS) {
        let h0 = c * CHUNK_HOPS;
        let nh = CHUNK_HOPS.min(hops.saturating_sub(h0));
        let n = nh * HOP;
        let a0 = (h0 * HOP) as i64;
        let mut mono = vec![0.0f32; n];
        for it in t.items.iter().filter(|i| i.enabled && i.start.to_units_floor(RATE as i64) < a0 + n as i64 && i.end().to_units_floor(RATE as i64) > a0) {
            let Some([l, r]) = filmcraft_render::audio::clip_signal(it, a0, n, RATE, sources) else { continue };
            for ((m, x), y) in mono.iter_mut().zip(&l).zip(&r) {
                *m += 0.5 * (x + y);
            }
        }
        for v in &mut mono {
            let y = a * (y1 + *v - x1);
            x1 = *v;
            y1 = if y.is_finite() { y } else { 0.0 };
            *v = y1;
        }
        db.extend(mono.chunks(HOP).map(|h| {
            let e = h.iter().map(|v| v * v).sum::<f32>() / h.len().max(1) as f32;
            10.0 * (e + 1e-13).log10()
        }));
        if !tick() {
            return None;
        }
    }
    Some(db)
}

/// Noise floor (the commonest level, in 2 dB steps, among the quieter half of the hops above
/// digital silence), voice level (their 98th percentile) and whether the track is gated. None when
/// (almost) nothing is heard.
fn stats(db: &[f32]) -> Option<(f32, f32, bool)> {
    // on at most 200 000 hops, evenly picked
    let step = (db.len() / 200_000).max(1);
    let all: Vec<f32> = db.iter().step_by(step).copied().collect();
    let mut heard: Vec<f32> = all.iter().copied().filter(|v| *v > SILENT_DB).collect();
    if heard.len() < MIN_BURST {
        return None;
    }
    heard.sort_by(f32::total_cmp);
    let q = |p: f64| heard.get(((heard.len() - 1) as f64 * p) as usize).copied().unwrap_or(FLOOR_DB);
    let voice = q(0.98);
    let mut bins = [0usize; 45];
    for v in heard.iter().take(heard.len().div_ceil(2)) {
        let k = ((v - SILENT_DB) / 2.0).floor();
        if k >= 0.0
            && let Some(b) = bins.get_mut(k as usize)
        {
            *b += 1;
        }
    }
    let smooth = |k: usize| (k.saturating_sub(1)..=k + 1).filter_map(|j| bins.get(j)).sum::<usize>();
    let mode = (0..bins.len()).max_by_key(|k| (smooth(*k), std::cmp::Reverse(*k))).unwrap_or(0);
    let floor = SILENT_DB + 2.0 * mode as f32 + 1.0;
    let gated = 1.0 - heard.len() as f64 / all.len().max(1) as f64 > GATED_SHARE;
    Some((floor, voice, gated))
}

/// The speech threshold of a track (dBFS) for a range under its voice. None for a silent track.
pub fn threshold(t: &TrackLevels, range_db: f64) -> Option<f32> {
    let (floor, voice, gated) = t.stats?;
    let low = voice - range_db as f32;
    let th = if gated {
        low
    } else if voice - floor < MIN_DYNAMICS_DB {
        // a steady sound: loud talks all along, quiet is silent
        if floor > STEADY_LOUD_DB { FLOOR_DB } else { return None }
    } else {
        low.max(floor + OVER_NOISE_DB)
    };
    Some(th.max(FLOOR_DB))
}

/// Which hops of a track are speech (see the module docs).
pub fn activity(t: &TrackLevels, range_db: f64) -> Vec<bool> {
    let Some(th) = threshold(t, range_db) else { return vec![false; t.db.len()] };
    let mut act: Vec<bool> = t.db.iter().map(|v| *v > th).collect();
    // drop blips, then close short gaps
    fill_runs(&mut act, true, MIN_BURST, false);
    fill_runs(&mut act, false, CLOSE_GAP, true);
    act
}

/// Flip the runs of `value` shorter than `shorter` (with `inner_only`, only runs between two
/// runs of the other value: a gap is closed between speech, never at the ends).
fn fill_runs(act: &mut [bool], value: bool, shorter: usize, inner_only: bool) {
    let n = act.len();
    let mut i = 0;
    while i < n {
        if act.get(i) != Some(&value) {
            i += 1;
            continue;
        }
        let mut j = i;
        while act.get(j) == Some(&value) {
            j += 1;
        }
        let inner = i > 0 && j < n;
        if j - i < shorter && (!inner_only || inner) {
            for v in act.iter_mut().take(j).skip(i) {
                *v = !value;
            }
        }
        i = j;
    }
}

/// Share of the hops where the track talks.
pub fn speaking_share(act: &[bool]) -> f64 {
    if act.is_empty() { 0.0 } else { act.iter().filter(|v| **v).count() as f64 / act.len() as f64 }
}

/// The dead times (analysed sequence time) when nobody talks on the tracks whose activity is in
/// `acts` (`hops` long each).
pub fn dead_ranges(acts: &[Vec<bool>], hops: usize, st: &Settings) -> Vec<TimeRange> {
    let min = (st.min_seconds * HOPS_PER_SECOND as f64).round().max(1.0) as usize;
    let after = (st.keep_after * HOPS_PER_SECOND as f64).round() as usize;
    let before = (st.keep_before * HOPS_PER_SECOND as f64).round() as usize;
    let talking = |h: usize| acts.iter().any(|a| a.get(h).copied().unwrap_or(false));
    let mut out = Vec::new();
    let mut h = 0;
    while h < hops {
        if talking(h) {
            h += 1;
            continue;
        }
        let start = h;
        while h < hops && !talking(h) {
            h += 1;
        }
        if h - start < min {
            continue;
        }
        let a = if start > 0 { start + after } else { start };
        let b = if h < hops { h.saturating_sub(before) } else { h };
        if b > a {
            out.push(TimeRange::from_bounds(hop_tick(a), hop_tick(b)));
        }
    }
    out
}

/// The dead times of `levels` on `tracks` with `st`.
pub fn dead_ranges_of(levels: &Levels, tracks: &[u64], st: &Settings) -> Vec<TimeRange> {
    let acts: Vec<Vec<bool>> = levels.tracks.iter().filter(|t| tracks.contains(&t.track)).map(|t| activity(t, st.range_db)).collect();
    dead_ranges(&acts, levels.hops, st)
}

/// Map analysed-time ranges onto the edit through `spans`, joined where they touch and rounded
/// inward to whole frames; pieces shorter than half a second are dropped.
pub fn to_edit(spans: &[Span], ranges: &[TimeRange], rate: FrameRate) -> Vec<TimeRange> {
    let mut pieces: Vec<(Tick, Tick)> = Vec::new();
    for r in ranges {
        for sp in spans {
            let a = r.start.max(sp.source);
            let b = r.end().min(sp.source + sp.duration);
            if b > a {
                pieces.push((sp.edit + (a - sp.source), sp.edit + (b - sp.source)));
            }
        }
    }
    pieces.sort();
    let frame = rate.frame_duration();
    let mut joined: Vec<(Tick, Tick)> = Vec::new();
    for (a, b) in pieces {
        match joined.last_mut() {
            Some(last) if a <= last.1 + frame => last.1 = last.1.max(b),
            _ => joined.push((a, b)),
        }
    }
    let min = Tick(TICKS_PER_SECOND / 2).max(frame);
    joined
        .into_iter()
        .filter_map(|(a, b)| {
            let a = if rate.snap(a) < a { rate.tick_of(rate.frame_at(a).saturating_add(1)) } else { a };
            let b = rate.snap(b);
            (b - a >= min).then(|| TimeRange::from_bounds(a, b))
        })
        .collect()
}

fn is_dead_marker(m: &Marker) -> bool {
    m.name == MARKER_NAME && m.duration > Tick::ZERO
}

fn has_dead_markers(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence")?;
    if q.markers.iter().any(is_dead_marker) { Ok(()) } else { Err("mark the dead time first (Sequence ▸ Mark Dead Time…)".into()) }
}

/// Seconds with a French decimal comma ("5,2 s").
fn seconds_fr(t: Tick) -> String {
    format!("{:.1} s", t.0 as f64 / TICKS_PER_SECOND as f64).replace('.', ",")
}

fn ranges_p(p: &Value, cmd: &str, len: Tick) -> Result<Option<Vec<TimeRange>>> {
    let Some(v) = p.get("ranges") else { return Ok(None) };
    let list = v.as_array().ok_or_else(|| bad(cmd, "`ranges` is a list of [start, end] ticks"))?;
    if list.len() > 1_000_000 {
        return Err(bad(cmd, "too many ranges"));
    }
    let mut out = Vec::with_capacity(list.len());
    for r in list {
        let pair = r.as_array().filter(|a| a.len() == 2);
        let (Some(a), Some(b)) = (pair.and_then(|a| a.first()).and_then(Value::as_i64), pair.and_then(|a| a.get(1)).and_then(Value::as_i64)) else {
            return Err(bad(cmd, "each range is [start, end] in ticks"));
        };
        let (a, b) = (Tick(a.max(0)).min(len), Tick(b.max(0)).min(len));
        if b > a {
            out.push(TimeRange::from_bounds(a, b));
        }
    }
    Ok(Some(out))
}

fn mark(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = MARK;
    let analysed = analysed_sequence(s, p).ok_or_else(|| bad(cmd, "open a sequence (or select a multi-camera clip)"))?;
    let source = s.project.sequence(analysed).ok_or_else(|| bad(cmd, "no such sequence"))?.clone();
    let spans = spans(s, analysed);
    if spans.is_empty() {
        return Err(bad(cmd, "the active sequence holds no multi-camera clip of that source"));
    }
    let st = Settings::from_params(p);
    let ranges = match ranges_p(p, cmd, source.duration())? {
        Some(r) => r,
        None => {
            let provider = s.media.provider(s.project.clone(), s.services.clone());
            let levels = measure(&source, &provider, &mut |_| true).unwrap_or_default();
            let tracks: Vec<u64> = match p.get("tracks").and_then(Value::as_array) {
                Some(list) => {
                    let mut v = Vec::new();
                    for t in list {
                        let id = t.as_u64().ok_or_else(|| bad(cmd, "`tracks` is a list of audio track ids"))?;
                        if !levels.tracks.iter().any(|l| l.track == id) {
                            return Err(bad(cmd, format!("track {id} is not a playing audio track of the analysed sequence")));
                        }
                        v.push(id);
                    }
                    v
                }
                None => levels.tracks.iter().filter(|t| t.default).map(|t| t.track).collect(),
            };
            if tracks.is_empty() {
                return Err(bad(cmd, "no voice track to listen to"));
            }
            dead_ranges_of(&levels, &tracks, &st)
        }
    };
    let rate = s.sequence_rate();
    let edit = to_edit(&spans, &ranges, rate);
    let total: Tick = edit.iter().map(|r| r.duration).fold(Tick::ZERO, |a, b| a + b);
    let laid = edit.clone();
    let replaced = s.edit_sequence("Mark Dead Time", move |q, ctx, _| {
        let before = q.markers.len();
        q.markers.retain(|m| !is_dead_marker(m));
        let replaced = before - q.markers.len();
        for r in laid {
            let id = MarkerId(ctx.alloc());
            let comment = format!("Silence de {}", seconds_fr(r.duration));
            q.markers.push(Marker {
                id,
                start: r.start,
                duration: r.duration,
                name: MARKER_NAME.into(),
                comment,
                kind: MarkerKind::Comment,
                color: Label::Rose,
            });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(replaced)
    })?;
    Ok(json!({
        "source": analysed.0,
        "markers": edit.len(),
        "replaced": replaced,
        "seconds": total.0 as f64 / TICKS_PER_SECOND as f64,
        "ranges": edit.iter().map(|r| [r.start.0, r.end().0]).collect::<Vec<_>>(),
    }))
}

fn remove(s: &mut Session, _: &Value) -> Result<Value> {
    has_dead_markers(s).map_err(|e| bad(REMOVE, e))?;
    let (n, total) = s.edit_sequence("Remove Dead Time", |q, ctx, _| {
        let ranges: Vec<TimeRange> = q.markers.iter().filter(|m| is_dead_marker(m)).map(|m| TimeRange::new(m.start, m.duration)).collect();
        q.markers.retain(|m| !is_dead_marker(m));
        let n = ranges.len();
        let total = filmcraft_edit::transcript::ripple_delete_ranges(q, ranges, ctx);
        q.check().map_err(EngineError::Other)?;
        Ok((n, total))
    })?;
    Ok(json!({"removed": n, "seconds": total.0 as f64 / TICKS_PER_SECOND as f64}))
}
