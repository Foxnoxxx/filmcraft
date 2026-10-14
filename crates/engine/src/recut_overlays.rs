//! RECut: Apply POV Overlays (`recut.povOverlays`, RECUT.md).
//!
//! In a multi-camera edit, each stretch where one camera is shown (a *segment*: adjacent
//! multi-camera clips showing the same angle) gets that camera's overlay: its name, its player's
//! head… The user dresses one segment per camera by hand, on the video tracks above the
//! multi-camera clips; that first dressed segment is the camera's **model**. The command copies
//! each model onto every other segment of its camera:
//!
//! - a model clip that covers its whole segment covers each segment it is copied to (stretched);
//! - a shorter one keeps its offset from the segment's start and its length (a 3-second pop-up),
//!   cut at the segment's end;
//! - a clip with limited media (a video, a nested sequence) is never made longer than its media.
//!
//! Running it again after re-cutting the edit removes the copies (clips identical to an earlier
//! model clip on the same track: same item, same source In, same effects) and lays them out again.
//! A clip the user changed is no longer identical, so it stays; the models themselves are never
//! changed; and a copy never lands on another clip: that segment is skipped and reported.
//! Everything is one undo step.

use std::collections::BTreeMap;

use filmcraft_project::{ClipId, ItemId, TrackItem};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_seq};
use crate::{EngineError, Result, Session};

pub const OVERLAYS: &str = "recut.povOverlays";

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: OVERLAYS,
        label: "Apply POV Overlays",
        menu: &["Clip"],
        shortcut: None,
        params: r#"{"clip":id?}"#,
        enabled: has_multicam_track,
        run: apply,
        journal: true,
    }]
}

/// One stretch of the edit where one camera is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: Tick,
    pub end: Tick,
    pub angle: u32,
}

/// The multi-camera picture track and source the command works on: the clip given as `clip`, else
/// a selected multi-camera picture clip, else the one at the playhead.
pub(crate) fn target(s: &Session, p: &Value) -> Option<(usize, ItemId)> {
    let q = s.active_sequence()?;
    let is_mc = |it: &TrackItem| it.multicam.is_some_and(|m| m.enabled) && s.project.sequence(it.item).is_some();
    let find = |c: ClipId| q.video_tracks.iter().enumerate().find_map(|(ti, t)| t.item(c).filter(|it| is_mc(it)).map(|it| (ti, it.item)));
    if let Some(c) = p.get("clip").and_then(Value::as_u64) {
        return find(ClipId(c));
    }
    if let Some(found) = s.state.selection.iter().find_map(|c| find(*c)) {
        return Some(found);
    }
    let (track, clip) = crate::multicam::multicam_clip_at(s, s.playhead())?;
    Some((q.video_tracks.iter().position(|t| t.id == track)?, clip.item))
}

fn has_multicam_track(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if target(s, &Value::Null).is_some() { Ok(()) } else { Err("select a multi-camera clip".into()) }
}

/// The segments of the multi-camera clips of `source` on `items` (sorted by time): adjacent clips
/// showing the same angle make one segment.
pub fn segments(items: &[TrackItem], source: ItemId) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut clips: Vec<&TrackItem> = items.iter().filter(|i| i.item == source && i.multicam.is_some_and(|m| m.enabled)).collect();
    clips.sort_by_key(|i| i.start);
    for it in clips {
        let angle = it.multicam.map_or(0, |m| m.angle);
        match out.last_mut() {
            Some(last) if last.end == it.start && last.angle == angle => last.end = it.end(),
            _ => out.push(Segment { start: it.start, end: it.end(), angle }),
        }
    }
    out
}

/// Clips on the overlay tracks are the same overlay when they show the same thing the same way.
fn same_overlay(a: &TrackItem, b: &TrackItem) -> bool {
    a.item == b.item && a.source_in == b.source_in && a.effects == b.effects && a.speed == b.speed && a.reverse == b.reverse
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = OVERLAYS;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let (mc_track, source) = target(s, p).ok_or_else(|| bad(cmd, "select a multi-camera clip"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let rate = q.settings.frame_rate;
    let tol = rate.frame_duration();
    let segs = segments(q.video_tracks.get(mc_track).map(|t| t.items.as_slice()).unwrap_or_default(), source);
    let (Some(first), Some(last)) = (segs.first(), segs.last()) else { return Err(bad(cmd, "no multi-camera clip on that track")) };
    let (span_start, span_end) = (first.start, last.end);
    let cameras = s.project.sequence(source).map(|q| q.cameras()).unwrap_or_default();
    let name_of = |angle: u32| cameras.cameras.get(angle as usize).map_or_else(|| format!("Camera {}", angle + 1), |c| c.name.clone());
    // Originals: overlay clips with no identical clip earlier on their track. The later identical
    // ones are copies (from an earlier run, or the user's own duplicates).
    let inside = |it: &TrackItem, sg: &Segment| it.start + tol >= sg.start && it.end() <= sg.end + tol;
    let in_span = |it: &TrackItem| it.start + tol >= span_start && it.end() <= span_end + tol;
    let overlay_tracks = || q.video_tracks.iter().enumerate().filter(|(ti, t)| *ti > mc_track && !t.locked);
    let mut originals: Vec<(usize, TrackItem)> = Vec::new();
    for (ti, t) in overlay_tracks() {
        let mut items: Vec<&TrackItem> = t.items.iter().filter(|it| in_span(it)).collect();
        items.sort_by_key(|it| it.start);
        for (k, it) in items.iter().enumerate() {
            if !items.iter().take(k).any(|e| same_overlay(e, it)) {
                originals.push((ti, (*it).clone()));
            }
        }
    }
    // the model of each angle: the originals inside its first dressed segment
    let mut models: BTreeMap<u32, (Segment, Vec<(usize, TrackItem)>)> = BTreeMap::new();
    for sg in &segs {
        if models.contains_key(&sg.angle) {
            continue;
        }
        let clips: Vec<(usize, TrackItem)> = originals.iter().filter(|(_, it)| inside(it, sg)).cloned().collect();
        if !clips.is_empty() {
            models.insert(sg.angle, (*sg, clips));
        }
    }
    if models.is_empty() {
        return Err(bad(cmd, "dress one stretch of each camera first: put its overlay on a video track above the multi-camera clips"));
    }
    let model_ids: Vec<ClipId> = models.values().flat_map(|(_, cs)| cs.iter().map(|c| c.1.id)).collect();
    let limits: BTreeMap<ItemId, Option<Tick>> =
        models.values().flat_map(|(_, cs)| cs.iter().map(|c| c.1.item)).map(|i| (i, crate::media_duration(&s.project, &s.media, i))).collect();
    let mut missing: Vec<String> = Vec::new();
    for name in segs.iter().map(|sg| sg.angle).filter(|a| !models.contains_key(a)).map(name_of) {
        if !missing.contains(&name) {
            missing.push(name);
        }
    }
    let (mut placed, mut replaced, mut skipped) = (0usize, 0usize, 0usize);
    s.edit("Apply POV Overlays", |pr, _| {
        let mut fresh: Vec<ClipId> = Vec::new();
        // ids for the copies, allocated up front (the sequence is borrowed below)
        let wanted: usize = segs.len().saturating_mul(models.values().map(|m| m.1.len()).max().unwrap_or(0));
        for _ in 0..wanted.min(1_000_000) {
            fresh.push(ClipId(pr.alloc_id()));
        }
        let mut ids = fresh.into_iter();
        let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        // 1. remove the copies an earlier run made (identical to a model, not a model)
        for (ti, t) in seq.video_tracks.iter_mut().enumerate() {
            if ti <= mc_track || t.locked {
                continue;
            }
            let before = t.items.len();
            t.items.retain(|it| {
                let managed = !model_ids.contains(&it.id)
                    && it.start + tol >= span_start
                    && it.end() <= span_end + tol
                    && models.values().any(|(_, cs)| cs.iter().any(|(mti, m)| *mti == ti && same_overlay(it, m)));
                !managed
            });
            replaced += before - t.items.len();
            filmcraft_edit::remove_orphan_transitions(t);
        }
        // 2. lay each model out on the other segments of its camera
        for sg in &segs {
            let Some((model_seg, clips)) = models.get(&sg.angle) else { continue };
            if model_seg == sg {
                continue;
            }
            for (ti, m) in clips {
                let whole = (m.start - model_seg.start).0.abs() <= tol.0 && (m.end() - model_seg.end).0.abs() <= tol.0;
                let start = if whole { sg.start } else { sg.start + (m.start - model_seg.start) };
                let mut dur = if whole { sg.end - sg.start } else { m.duration.min(sg.end - start) };
                if let Some(Some(limit)) = limits.get(&m.item)
                    && (m.speed - 1.0).abs() < f64::EPSILON
                {
                    dur = dur.min(*limit - m.source_in);
                }
                if dur < rate.frame_duration() || start >= sg.end {
                    continue;
                }
                let Some(t) = seq.video_tracks.get_mut(*ti) else { continue };
                if !filmcraft_edit::track_range_empty(t, filmcraft_time::TimeRange::new(start, dur)) {
                    skipped += 1;
                    continue;
                }
                let Some(id) = ids.next() else { continue };
                let mut c = m.clone();
                c.id = id;
                c.start = start;
                c.duration = dur;
                c.link = None;
                c.group = None;
                t.items.push(c);
                t.sort();
                placed += 1;
            }
        }
        seq.check().map_err(EngineError::Other)?;
        Ok(())
    })?;
    Ok(json!({
        "segments": segs.len(),
        "cameras": models.keys().map(|a| name_of(*a)).collect::<Vec<_>>(),
        "placed": placed,
        "replaced": replaced,
        "skipped": skipped,
        "camerasWithoutOverlay": missing,
    }))
}
