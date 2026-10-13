//! RECut commands: additions made for RECut, the customised version of FilmCraft (RECUT.md).
//!
//! | Id | Menu | |
//! |---|---|---|
//! | `recut.nestMulticam` | Clip ▸ Nest as Multi-Camera | clips already in sync, stacked on several video tracks → a multi-camera source sequence (one camera per video track, with the sound linked to it) replaced by one multi-camera clip whose sound switches with the angle |
//!
//! Why `recut.nestMulticam`: Nest… follows Premiere, so a selection whose sound sits on several
//! audio tracks nests the picture only, and Multi-Camera ▸ Enable on that nest switches the picture
//! while the sound stays put. Here every camera keeps its own sound tracks (all the streams of an OBS
//! recording, say) and the new multi-camera audio clip plays the selected camera's sound.

use std::collections::{BTreeMap, BTreeSet};

use filmcraft_edit as edit;
use filmcraft_project::{Camera, ClipId, MulticamAudio, MulticamSource, Track, TrackId, TrackItem, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_selection, str_p};
use crate::{EngineError, Result, Session};

const NEST_MULTICAM: &str = "recut.nestMulticam";

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: NEST_MULTICAM,
        label: "Nest as Multi-Camera",
        menu: &["Clip"],
        shortcut: None,
        params: r#"{"name":str?,"audio":"switch|camera1|all"?}"#,
        enabled: has_selection,
        run: nest_multicam,
        journal: true,
    }]
}

/// The selection plus every clip linked to a selected clip (whatever Linked Selection says: a
/// camera's sound goes with its picture).
fn selection_with_partners(q: &filmcraft_project::Sequence, sel: &[ClipId]) -> BTreeSet<ClipId> {
    let links: BTreeSet<u64> = sel.iter().filter_map(|c| q.find_item(*c).and_then(|(_, i)| i.link)).collect();
    let mut out: BTreeSet<ClipId> = sel.iter().copied().filter(|c| q.find_item(*c).is_some()).collect();
    for it in q.all_tracks().flat_map(|t| t.items.iter()) {
        if it.link.is_some_and(|l| links.contains(&l)) {
            out.insert(it.id);
        }
    }
    out
}

/// A camera's name: the track's own name when the user gave it one, else the first clip's name
/// without its extension.
fn camera_name(track: &Track, first: Option<&TrackItem>, n: usize) -> String {
    let numbered = |prefix: &str| track.name.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    let default_name = track.name.trim().is_empty() || numbered("Video ") || numbered("V");
    if !default_name {
        return track.name.clone();
    }
    let clip = first.map(|i| i.name.as_str()).unwrap_or_default();
    let stem = match clip.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => stem,
        _ => clip,
    };
    if stem.trim().is_empty() { format!("Camera {n}") } else { stem.to_string() }
}

fn nest_multicam(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = NEST_MULTICAM;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let audio = match str_p(p, "audio") {
        Some(a) => MulticamAudio::from_name(a).ok_or_else(|| bad(cmd, format!("unknown audio mode `{a}` (switch, camera1, all)")))?,
        None => MulticamAudio::SwitchAudio,
    };
    let sel = selection_with_partners(&q, &s.state.selection);
    // selected clips per track index, video and audio
    let mut video: BTreeMap<usize, Vec<TrackItem>> = BTreeMap::new();
    let mut sound: BTreeMap<usize, Vec<TrackItem>> = BTreeMap::new();
    for (kind, tracks, out) in [(TrackKind::Video, &q.video_tracks, &mut video), (TrackKind::Audio, &q.audio_tracks, &mut sound)] {
        for (ti, t) in tracks.iter().enumerate() {
            let picked: Vec<TrackItem> = t.items.iter().filter(|i| sel.contains(&i.id)).cloned().collect();
            if picked.is_empty() {
                continue;
            }
            if t.locked {
                let word = if kind == TrackKind::Video { "V" } else { "A" };
                return Err(bad(cmd, format!("track {word}{} is locked: unlock it first", ti + 1)));
            }
            out.insert(ti, picked);
        }
    }
    if video.len() < 2 {
        return Err(bad(cmd, "select clips on at least two video tracks, one per camera, already in sync"));
    }
    // camera n = the n-th video track used (bottom up); which camera each selected link belongs to
    let cam_tracks: Vec<usize> = video.keys().copied().collect();
    let mut link_cam: BTreeMap<u64, usize> = BTreeMap::new();
    for (cam, ti) in cam_tracks.iter().enumerate() {
        for it in video.get(ti).into_iter().flatten() {
            if let Some(l) = it.link {
                link_cam.insert(l, cam);
            }
        }
    }
    // each audio track used belongs to one camera (its clips are linked to that camera's pictures),
    // or to none (sound of its own, heard whatever the angle)
    let mut track_cam: BTreeMap<usize, Option<usize>> = BTreeMap::new();
    for (ti, items) in &sound {
        let owners: BTreeSet<usize> = items.iter().filter_map(|i| i.link.and_then(|l| link_cam.get(&l).copied())).collect();
        if owners.len() > 1 {
            return Err(bad(
                cmd,
                format!("A{} holds the sound of several cameras: each camera's sound must be on audio tracks of its own", ti.saturating_add(1)),
            ));
        }
        track_cam.insert(*ti, owners.first().copied());
    }
    let all: Vec<&TrackItem> = video.values().chain(sound.values()).flatten().collect();
    let start = all.iter().map(|i| i.start).min().unwrap_or_default();
    let end = all.iter().map(|i| i.end()).max().unwrap_or_default();
    if end <= start {
        return Err(bad(cmd, "nothing to nest"));
    }
    let span = TimeRange::new(start, end - start);
    // audio tracks of the nest: each camera's in camera order, then the sound of its own
    let mut audio_order: Vec<(usize, Option<usize>)> = Vec::new();
    for cam in 0..cam_tracks.len() {
        audio_order.extend(track_cam.iter().filter(|(_, c)| **c == Some(cam)).map(|(ti, c)| (*ti, *c)));
    }
    audio_order.extend(track_cam.iter().filter(|(_, c)| c.is_none()).map(|(ti, c)| (*ti, *c)));
    let names: Vec<String> = cam_tracks
        .iter()
        .enumerate()
        .filter_map(|(n, ti)| q.video_tracks.get(*ti).map(|t| camera_name(t, video.get(ti).and_then(|v| v.first()), n + 1)))
        .collect();
    let name = str_p(p, "name").filter(|n| !n.trim().is_empty()).map(str::to_string).unwrap_or_else(|| {
        let first = names.first().cloned().unwrap_or_default();
        format!("{first} Multicam")
    });
    let removed: Vec<ClipId> = sel.iter().copied().collect();
    let rate = q.settings.frame_rate;
    let lowest_v = cam_tracks.first().copied().unwrap_or(0);
    let lowest_a = sound.keys().next().copied();
    let r = s.edit("Nest as Multi-Camera", |pr, st| {
        let nid = pr.new_sequence(&name, q.settings.clone(), cam_tracks.len(), audio_order.len().max(1), None);
        {
            let nq = pr.sequence_mut(nid).ok_or(EngineError::NoSequence)?;
            let vids: Vec<TrackId> = nq.video_tracks.iter().map(|t| t.id).collect();
            let aids: Vec<TrackId> = nq.audio_tracks.iter().map(|t| t.id).collect();
            let mut cameras: Vec<Camera> = names
                .iter()
                .zip(&vids)
                .map(|(n, id)| Camera { name: n.clone(), video_track: Some(*id), audio_tracks: Vec::new(), enabled: true, source: None })
                .collect();
            let mut own_sound: Vec<TrackId> = Vec::new();
            // pictures, shifted to the nest's start; transitions between nested clips go along
            for (cam, ti) in cam_tracks.iter().enumerate() {
                let (Some(src), Some(dst)) = (q.video_tracks.get(*ti), nq.video_tracks.get_mut(cam)) else { continue };
                dst.name = names.get(cam).cloned().unwrap_or_default();
                copy_clips(src, dst, &sel, start);
            }
            for (k, (ti, owner)) in audio_order.iter().enumerate() {
                let (Some(src), Some(dst), Some(id)) = (q.audio_tracks.get(*ti), nq.audio_tracks.get_mut(k), aids.get(k)) else { continue };
                // the track's own settings go with its sound (volume, pan, inserts, channel format)
                dst.name = src.name.clone();
                dst.channels = src.channels;
                dst.volume_db = src.volume_db;
                dst.pan = src.pan;
                dst.effects = src.effects.clone();
                dst.muted = src.muted;
                copy_clips(src, dst, &sel, start);
                match owner.and_then(|c| cameras.get_mut(c)) {
                    Some(c) => c.audio_tracks.push(*id),
                    None => own_sound.push(*id),
                }
            }
            if !own_sound.is_empty() {
                // sound linked to no picture: an audio-only source, heard whatever the angle
                cameras.push(Camera { name: "Audio".into(), video_track: None, audio_tracks: own_sound, enabled: true, source: None });
            }
            nq.multicam = Some(MulticamSource { cameras, audio, sync: "timeline".into() });
            nq.check().map_err(EngineError::Other)?;
        }
        // the nest replaces the selection: picture on the lowest camera track, sound on the lowest
        // audio track used (or the first free one above), linked
        let whole = TimeRange::new(Tick::ZERO, span.duration);
        let v = pr.make_track_item(nid, TrackKind::Video, start, whole, rate).ok_or_else(|| bad(cmd, "cannot place the multi-camera clip"))?;
        let a = pr.make_track_item(nid, TrackKind::Audio, start, whole, rate).ok_or_else(|| bad(cmd, "cannot place the multi-camera clip"))?;
        let (link, new_v, new_a) = (pr.alloc_id(), pr.alloc_id(), pr.alloc_id());
        let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        edit::delete_items(seq, &removed);
        let mut ids = Vec::new();
        for (kind, lowest, mut clip, new_track) in [(TrackKind::Video, Some(lowest_v), v, new_v), (TrackKind::Audio, lowest_a.or(Some(0)), a, new_a)] {
            let Some(lowest) = lowest else { continue };
            let tracks = seq.tracks_mut(kind);
            let free = (lowest..tracks.len()).find(|i| tracks.get(*i).is_some_and(|t| !t.locked && edit::track_range_empty(t, span)));
            let at = match free {
                Some(i) => i,
                None => {
                    let (word, n) = (if kind == TrackKind::Video { "Video" } else { "Audio" }, tracks.len() + 1);
                    tracks.push(Track::new(TrackId(new_track), kind, format!("{word} {n}")));
                    tracks.len() - 1
                }
            };
            clip.link = Some(link);
            ids.push(clip.id);
            let t = tracks.get_mut(at).ok_or(EngineError::NoSequence)?;
            t.items.push(clip);
            t.sort();
        }
        seq.check().map_err(EngineError::Other)?;
        st.selection = ids.clone();
        Ok((nid, ids))
    })?;
    Ok(json!({
        "sequence": r.0.0,
        "clips": r.1.iter().map(|c| c.0).collect::<Vec<_>>(),
        "cameras": names,
        "audioTracks": audio_order.len(),
        "audio": audio.name(),
    }))
}

/// Copy the clips of `src` that are in `sel` into `dst`, `start` earlier, with the transitions
/// whose clips are all copied.
fn copy_clips(src: &Track, dst: &mut Track, sel: &BTreeSet<ClipId>, start: Tick) {
    for it in src.items.iter().filter(|i| sel.contains(&i.id)) {
        let mut it = it.clone();
        it.start -= start;
        dst.items.push(it);
    }
    let inside = |c: Option<ClipId>| c.is_none_or(|c| sel.contains(&c));
    for trn in src.transitions.iter().filter(|x| (x.from.is_some() || x.to.is_some()) && inside(x.from) && inside(x.to)) {
        let mut trn = trn.clone();
        trn.start -= start;
        dst.transitions.push(trn);
    }
    dst.sort();
    edit::remove_orphan_transitions(dst);
}
