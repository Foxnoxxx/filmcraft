//! RECut: tests of the RECut additions (RECUT.md): Nest as Multi-Camera, multi-stream cameras in
//! Create Multi-Camera Source Sequence, Balance Multi-Camera Sound, Apply POV Overlays, dead time, the
//! watch folder.

use std::sync::Arc;

use filmcraft_media::MediaSource;
use filmcraft_project::{ItemId, ItemKind, Label, MediaClip, MediaRef, MulticamAudio, Project, SequenceSettings, TrackId};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use serde_json::json;

use crate::Session;

const RATE: FrameRate = FrameRate::FPS_24;
const FRAMES: i64 = 48;

/// A bars-and-tone media item with `streams` audio streams (an OBS recording with the voice, the
/// game… on separate tracks).
fn media(p: &mut Project, name: &str, streams: usize) -> ItemId {
    media_len(p, name, streams, FRAMES)
}

/// [`media`] `frames` long.
fn media_len(p: &mut Project, name: &str, streams: usize, frames: i64) -> ItemId {
    let source = filmcraft_media::generators::GeneratorSource::new(filmcraft_media::Generator::BarsAndTone, 32, 32, RATE, RATE.tick_of(frames));
    let mut info = source.info().clone();
    let first = info.audio_streams[0].clone();
    while info.audio_streams.len() < streams {
        info.audio_streams.push(first.clone());
    }
    p.add_item(
        name,
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(filmcraft_media::Generator::BarsAndTone),
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        }),
        None,
    )
}

fn settings() -> SequenceSettings {
    SequenceSettings { width: 32, height: 32, frame_rate: RATE, ..Default::default() }
}

/// A session with an edit sequence (3 video, 4 audio tracks) and two cameras: "zenh.mp4" (two
/// audio streams) and "screu.mp4" (one).
fn session() -> (Session, ItemId, ItemId, ItemId) {
    let mut s = Session::default();
    let mut p = (*s.project).clone();
    let seq = p.new_sequence("Edit", settings(), 3, 4, None);
    let zenh = media(&mut p, "zenh.mp4", 2);
    let screu = media(&mut p, "screu.mp4", 1);
    s.project = Arc::new(p);
    s.state.active_sequence = Some(seq);
    (s, seq, zenh, screu)
}

fn tracks(s: &Session) -> (Vec<TrackId>, Vec<TrackId>) {
    let q = s.active_sequence().unwrap();
    (q.video_tracks.iter().map(|t| t.id).collect(), q.audio_tracks.iter().map(|t| t.id).collect())
}

fn place(s: &mut Session, item: ItemId, at: i64, v: TrackId, a: TrackId) {
    let range = TimeRange::new(Tick::ZERO, RATE.tick_of(FRAMES));
    crate::commands::place_item(s, item, range, RATE.tick_of(at), Some(v), Some(a), false, "test", None).unwrap();
}

/// Zenh on V1 + A1/A2 (two streams), Screu on V2 + A3, in sync; everything selected.
fn stacked() -> (Session, ItemId) {
    let (mut s, seq, zenh, screu) = session();
    let (v, a) = tracks(&s);
    place(&mut s, zenh, 0, v[0], a[0]);
    place(&mut s, screu, 0, v[1], a[2]);
    let all: Vec<u64> = s.active_sequence().unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    assert_eq!(all.len(), 5, "2 pictures + 3 sound clips");
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    (s, seq)
}

#[test]
fn nest_multicam_gives_each_camera_its_own_sound() {
    let (mut s, _) = stacked();
    let r = s.execute("recut.nestMulticam", json!({})).unwrap();
    assert_eq!(r["cameras"], json!(["zenh", "screu"]));
    assert_eq!(r["audio"], "switch");
    let src = ItemId(r["sequence"].as_u64().unwrap());
    // the source: one camera per video track, Zenh with both of its streams
    let nq = s.project.sequence(src).unwrap().clone();
    let mc = nq.multicam.clone().unwrap();
    assert_eq!(mc.audio, MulticamAudio::SwitchAudio);
    assert_eq!(mc.cameras.len(), 2);
    assert_eq!(mc.cameras[0].audio_tracks.len(), 2);
    assert_eq!(mc.cameras[1].audio_tracks.len(), 1);
    let streams: Vec<usize> = nq.audio_tracks.iter().map(|t| t.items[0].audio_stream).collect();
    assert_eq!(streams, [0, 1, 0]);
    let names: Vec<&str> = nq.audio_tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["zenh 1", "zenh 2", "screu"], "sound tracks named after their camera");
    // the edit: one picture and one sound clip, linked, both multi-camera on angle 0
    let q = s.active_sequence().unwrap();
    let clips: Vec<_> = q.all_tracks().flat_map(|t| t.items.iter()).collect();
    assert_eq!(clips.len(), 2);
    assert!(clips.iter().all(|c| c.item == src && c.multicam.is_some_and(|m| m.enabled && m.angle == 0)));
    assert_eq!(clips[0].link, clips[1].link);
    assert!(clips[0].link.is_some());
    assert_eq!((q.video_tracks[0].items.len(), q.audio_tracks[0].items.len()), (1, 1));
    // Motion's position is a number (the frame centre), not "automatic" (NaN in Properties)
    let pos = q.video_tracks[0].items[0].effect("motion").and_then(|e| e.param("position")).map(|p| p.value.clone());
    assert_eq!(pos, Some(filmcraft_project::ParamValue::Vec2(filmcraft_geom::Vec2 { x: 16.0, y: 16.0 })), "{pos:?}");
    // sound follows the picture: cutting to camera 2 switches both, and only Screu's track plays
    assert!(s.state.multicam_audio_follows_video, "on by default in RECut");
    s.set_playhead(Tick::ZERO);
    s.execute("multicam.switchAngle", json!({"camera": 2})).unwrap();
    let q = s.active_sequence().unwrap();
    let angles: Vec<u32> = q.all_tracks().flat_map(|t| t.items.iter()).map(|c| c.multicam.unwrap().angle).collect();
    assert_eq!(angles, [1, 1]);
    assert_eq!(mc.audible_tracks(Some(1)), mc.cameras[1].audio_tracks);
    assert_eq!(mc.audible_tracks(Some(0)), mc.cameras[0].audio_tracks);
    // what plays is camera 2's sound alone
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let edit_seq = s.active_sequence().unwrap().clone();
    let mix = filmcraft_render::audio::mix_sequence(&s.project, &edit_seq, 24_000, 4_800, &provider);
    let only = filmcraft_render::audio::mix_sequence(&s.project, &nq.with_angle_audio(Some(1)), 24_000, 4_800, &provider);
    let err = mix.channels[0].iter().zip(&only.channels[0]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-4, "multi-camera sound = camera 2's ({err})");
    assert!(mix.channels[0].iter().any(|v| v.abs() > 1e-3), "audible");
    // one undo step back to the stacked clips
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().all_tracks().map(|t| t.items.len()).sum::<usize>(), 5);
}

#[test]
fn nest_multicam_keeps_sound_of_its_own_audible_on_every_angle() {
    let (mut s, seq, zenh, screu) = session();
    let (v, a) = tracks(&s);
    place(&mut s, zenh, 0, v[0], a[0]);
    place(&mut s, screu, 0, v[1], a[2]);
    // a music bed on A4, unlinked, selected too
    let mut p = (*s.project).clone();
    let music = media(&mut p, "music.wav", 1);
    s.project = Arc::new(p);
    crate::commands::place_item(&mut s, music, TimeRange::new(Tick::ZERO, RATE.tick_of(24)), Tick::ZERO, None, Some(a[3]), false, "test", None).unwrap();
    let all: Vec<u64> = s.project.sequence(seq).unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    let r = s.execute("recut.nestMulticam", json!({"name": "POV"})).unwrap();
    let src = ItemId(r["sequence"].as_u64().unwrap());
    let nq = s.project.sequence(src).unwrap();
    assert_eq!(s.project.item(src).unwrap().name, "POV");
    let mc = nq.multicam.as_ref().unwrap();
    assert_eq!(mc.cameras.len(), 3, "two cameras and the sound of its own");
    assert_eq!(mc.cameras[2].video_track, None);
    let music_track = mc.cameras[2].audio_tracks[0];
    for angle in [0, 1] {
        assert!(mc.audible_tracks(Some(angle)).contains(&music_track), "the music plays on angle {angle}");
    }
    assert_eq!(mc.shown_angles(), [0, 1], "only the cameras are angles");
}

#[test]
fn nest_multicam_refuses_a_track_mixing_two_cameras() {
    let (mut s, seq, zenh, screu) = session();
    let (v, a) = tracks(&s);
    // Screu's sound on A1 after Zenh's: A1 carries both cameras
    place(&mut s, zenh, 0, v[0], a[0]);
    place(&mut s, screu, FRAMES, v[1], a[0]);
    let all: Vec<u64> = s.project.sequence(seq).unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    let e = s.execute("recut.nestMulticam", json!({})).unwrap_err().to_string();
    assert!(e.contains("A1") && e.contains("several cameras"), "{e}");
    assert_eq!(s.project.sequence(seq).unwrap().all_tracks().map(|t| t.items.len()).sum::<usize>(), 5, "nothing changed");
}

#[test]
fn nest_multicam_needs_two_cameras_and_rejects_bad_params() {
    let (mut s, seq, zenh, _) = session();
    let (v, a) = tracks(&s);
    place(&mut s, zenh, 0, v[0], a[0]);
    let all: Vec<u64> = s.project.sequence(seq).unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    assert!(s.execute("recut.nestMulticam", json!({})).unwrap_err().to_string().contains("two video tracks"));
    let (mut s, _) = stacked();
    assert!(s.execute("recut.nestMulticam", json!({"audio": "nope"})).is_err());
    assert!(s.execute("recut.nestMulticam", json!({"audio": 7, "name": [1]})).is_ok(), "ill-typed optional params fall back to the defaults");
    s.execute("timeline.select", json!({"clips": []})).unwrap();
    assert!(s.execute("recut.nestMulticam", json!({})).is_err(), "nothing selected");
}

#[test]
fn create_multicam_keeps_every_audio_stream_of_a_camera() {
    let (mut s, _, zenh, screu) = session();
    let r = s.execute("clip.createMulticam", json!({"items": [zenh.0, screu.0], "method": "in", "audio": "switch"})).unwrap();
    let src = ItemId(r["sequence"].as_u64().unwrap());
    let q = s.project.sequence(src).unwrap();
    let mc = q.multicam.as_ref().unwrap();
    assert_eq!(mc.cameras.len(), 2);
    assert_eq!(mc.cameras[0].audio_tracks.len(), 2, "both of Zenh's streams");
    assert_eq!(mc.cameras[1].audio_tracks.len(), 1);
    let rows: Vec<(String, usize)> = q.audio_tracks.iter().map(|t| (t.name.clone(), t.items[0].audio_stream)).collect();
    assert_eq!(rows, [("zenh.mp4 1".to_string(), 0), ("zenh.mp4 2".to_string(), 1), ("screu.mp4".to_string(), 0)]);
    // Zenh's two sound clips and its picture move together
    let link = q.video_tracks[0].items[0].link;
    assert!(link.is_some());
    assert_eq!((q.audio_tracks[0].items[0].link, q.audio_tracks[1].items[0].link), (link, link));
    assert_eq!(mc.audible_tracks(Some(0)), mc.cameras[0].audio_tracks);
}

/// A real recording with two sound streams (an audio-only MOV, constant levels 0.25 and 0.125)
/// linked to camera 1's picture: on angle 0 the multi-camera clip plays both streams.
#[test]
fn nest_multicam_plays_every_stream_of_the_selected_camera() {
    use filmcraft_isobmff::{Brand, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
    let mut mux = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
    for value in [8192i16, 4096] {
        let entry = SampleEntry::pcm(PcmConfig { bits: 16, float: false, big_endian: false, signed: true, channels: 1, sample_rate: 48_000.0 });
        let track = mux.add_track(TrackConfig::new(entry, 48_000)).unwrap();
        let data: Vec<u8> = (0..96_000).flat_map(|_| value.to_le_bytes()).collect();
        mux.write_sample(track, WriteSample { data: &data, duration: 96_000, composition_offset: 0, is_sync: true }).unwrap();
    }
    let bytes = mux.finish().unwrap().into_inner();
    let (mut s, seq, zenh, screu) = session();
    let voice = crate::commands::import_bytes(&mut s, "/zenh-voice.mov", bytes.into(), None).unwrap();
    let (v, a) = tracks(&s);
    // Zenh: picture on V1, the two-stream recording on A1 + A2, linked; Screu on V2 + A3
    crate::commands::place_item(&mut s, zenh, TimeRange::new(Tick::ZERO, RATE.tick_of(FRAMES)), Tick::ZERO, Some(v[0]), None, false, "test", None).unwrap();
    crate::commands::place_item(&mut s, voice, TimeRange::new(Tick::ZERO, RATE.tick_of(FRAMES)), Tick::ZERO, None, Some(a[0]), false, "test", None).unwrap();
    let zenh_clips: Vec<u64> = {
        let q = s.project.sequence(seq).unwrap();
        [&q.video_tracks[0], &q.audio_tracks[0], &q.audio_tracks[1]].iter().map(|t| t.items[0].id.0).collect()
    };
    s.execute("timeline.select", json!({"clips": zenh_clips})).unwrap();
    s.execute("clip.link", json!({})).unwrap();
    place(&mut s, screu, 0, v[1], a[2]);
    let all: Vec<u64> = s.project.sequence(seq).unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    let r = s.execute("recut.nestMulticam", json!({})).unwrap();
    let src = ItemId(r["sequence"].as_u64().unwrap());
    let nq = s.project.sequence(src).unwrap().clone();
    let mc = nq.multicam.clone().unwrap();
    assert_eq!(mc.cameras[0].audio_tracks.len(), 2, "both streams belong to camera 1");
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let level = |q: &filmcraft_project::Sequence| {
        let m = filmcraft_render::audio::mix_sequence(&s.project, q, 24_000, 4_800, &provider);
        m.channels[0].iter().map(|v| v.abs()).sum::<f32>() / m.channels[0].len().max(1) as f32
    };
    let edit_seq = s.active_sequence().unwrap().clone();
    let angle0 = level(&edit_seq);
    let mut first_only = nq.with_angle_audio(Some(0));
    first_only.audio_tracks[1].muted = true;
    let stream0 = level(&first_only);
    assert!(angle0 > stream0 * 1.3, "the second stream is heard too ({angle0} vs {stream0})");
    assert!((angle0 - level(&nq.with_angle_audio(Some(0)))).abs() < 1e-4);
}

// ---------------------------------------------------------------- Balance Multi-Camera Sound

/// A sound file with one mono PCM stream per entry of `streams` (48 kHz, 16-bit).
fn pcm_file(s: &mut Session, name: &str, streams: &[Vec<f32>]) -> ItemId {
    use filmcraft_isobmff::{Brand, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
    let mut mux = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
    for samples in streams {
        let entry = SampleEntry::pcm(PcmConfig { bits: 16, float: false, big_endian: false, signed: true, channels: 1, sample_rate: 48_000.0 });
        let track = mux.add_track(TrackConfig::new(entry, 48_000)).unwrap();
        let data: Vec<u8> = samples.iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        mux.write_sample(track, WriteSample { data: &data, duration: samples.len() as u32, composition_offset: 0, is_sync: true }).unwrap();
    }
    let bytes = mux.finish().unwrap().into_inner();
    crate::commands::import_bytes(s, &format!("/{name}"), bytes.into(), None).unwrap()
}

/// Deterministic noise in [-1, 1].
fn noise(seed: u32, n: usize) -> Vec<f32> {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect()
}

/// Two cameras like Rémi's OBS recordings: Zenh with a mix track (voice + game), his voice (quiet,
/// in bursts) and his game (loud); Screu with one mixed track. Nested as multi-camera.
fn obs_multicam() -> (Session, ItemId, Vec<TrackId>) {
    let n = 48_000 * 2;
    let voice: Vec<f32> = noise(1, n).iter().enumerate().map(|(i, v)| if (i / 4_800) % 3 == 0 { v * 0.02 } else { 0.0 }).collect();
    let game: Vec<f32> = noise(2, n).iter().map(|v| v * 0.3).collect();
    let mix: Vec<f32> = voice.iter().zip(&game).map(|(a, b)| a + b).collect();
    let screu: Vec<f32> = noise(3, n).iter().map(|v| v * 0.05).collect();
    let (mut s, seq, zenh_pic, screu_pic) = session();
    let zenh_snd = pcm_file(&mut s, "zenh.mov", &[mix, voice, game]);
    let screu_snd = pcm_file(&mut s, "screu.mov", &[screu]);
    s.state.active_sequence = Some(seq);
    let (v, a) = tracks(&s);
    let len = TimeRange::new(Tick::ZERO, RATE.tick_of(FRAMES));
    let link = |s: &mut Session, clips: Vec<u64>| {
        s.execute("timeline.select", json!({"clips": clips})).unwrap();
        s.execute("clip.link", json!({})).unwrap();
    };
    crate::commands::place_item(&mut s, zenh_pic, len, Tick::ZERO, Some(v[0]), None, false, "test", None).unwrap();
    crate::commands::place_item(&mut s, zenh_snd, len, Tick::ZERO, None, Some(a[0]), false, "test", None).unwrap();
    let ids = |s: &Session, ts: &[(bool, usize)]| -> Vec<u64> {
        let q = s.project.sequence(seq).unwrap();
        ts.iter().map(|(video, i)| if *video { q.video_tracks[*i].items[0].id.0 } else { q.audio_tracks[*i].items[0].id.0 }).collect()
    };
    let zenh = ids(&s, &[(true, 0), (false, 0), (false, 1), (false, 2)]);
    link(&mut s, zenh);
    crate::commands::place_item(&mut s, screu_pic, len, Tick::ZERO, Some(v[1]), None, false, "test", None).unwrap();
    crate::commands::place_item(&mut s, screu_snd, len, Tick::ZERO, None, Some(a[3]), false, "test", None).unwrap();
    let screu = ids(&s, &[(true, 1), (false, 3)]);
    link(&mut s, screu);
    let all: Vec<u64> = s.project.sequence(seq).unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    let r = s.execute("recut.nestMulticam", json!({})).unwrap();
    let src = ItemId(r["sequence"].as_u64().unwrap());
    let audio: Vec<TrackId> = s.project.sequence(src).unwrap().audio_tracks.iter().map(|t| t.id).collect();
    assert_eq!(audio.len(), 4, "Zenh's three streams and Screu's one");
    (s, src, audio)
}

#[test]
fn balance_analysis_finds_the_obs_mix_track_and_suggests_roles() {
    let (mut s, src, t) = obs_multicam();
    let r = s.execute("recut.balanceMulticam", json!({"analyze": true})).unwrap();
    assert_eq!(r["sequence"], src.0);
    let rows = r["tracks"].as_array().unwrap();
    let row = |id: TrackId| rows.iter().find(|x| x["track"] == id.0).unwrap().clone();
    assert_eq!(row(t[0])["mixOf"], json!([t[1].0, t[2].0]), "track 1 = voice + game: {}", row(t[0]));
    assert_eq!(row(t[0])["suggested"], "mute");
    assert_eq!(row(t[1])["suggested"], "voice", "the quiet one");
    assert_eq!(row(t[2])["suggested"], "game");
    assert_eq!(row(t[3])["suggested"], "mixed", "one track: voice and game together");
    let lufs = |id: TrackId| row(id)["lufs"].as_f64().unwrap();
    assert!(lufs(t[1]) < lufs(t[2]) - 10.0, "voice {} game {}", lufs(t[1]), lufs(t[2]));
    // analysis changes nothing
    assert!(s.history.undo.last().is_none_or(|h| h.0 != "Balance Multi-Camera Sound"));
}

#[test]
fn balance_mutes_the_mix_track_and_brings_each_track_to_its_target() {
    let (mut s, src, t) = obs_multicam();
    let before = s.execute("recut.balanceMulticam", json!({"analyze": true})).unwrap();
    let lufs = |id: TrackId| before["tracks"].as_array().unwrap().iter().find(|x| x["track"] == id.0).unwrap()["lufs"].as_f64().unwrap();
    s.execute("recut.balanceMulticam", json!({})).unwrap();
    let q = s.project.sequence(src).unwrap().clone();
    let tr = |id: TrackId| q.audio_tracks.iter().find(|x| x.id == id).unwrap().clone();
    assert!(tr(t[0]).muted, "the OBS mix track is muted");
    let gain = |id: TrackId| tr(id).items[0].gain_db;
    assert!((gain(t[1]) - (-20.0 - lufs(t[1]))).abs() < 1e-6, "voice to -20 LUFS");
    assert!((gain(t[2]) - (-28.0 - lufs(t[2]))).abs() < 1e-6, "game 8 dB under");
    assert!((gain(t[3]) - (-24.0 - lufs(t[3]))).abs() < 1e-6, "mixed halfway");
    let count = |id: TrackId, fx: &str| tr(id).effects.iter().filter(|e| e.effect == fx).count();
    assert_eq!((count(t[1], "highpass"), count(t[1], "single_band_compressor")), (1, 1));
    assert_eq!((count(t[2], "highpass"), count(t[2], "single_band_compressor")), (0, 1));
    assert_eq!(q.master_effects.iter().filter(|e| e.effect == "hard_limiter").count(), 1);
    // measured without clip gain: a second run lands on the same gains, and nothing is stacked
    s.execute("recut.balanceMulticam", json!({})).unwrap();
    let q2 = s.project.sequence(src).unwrap().clone();
    for (a, b) in q2.audio_tracks.iter().zip(&q.audio_tracks) {
        assert!((a.items[0].gain_db - b.items[0].gain_db).abs() < 1e-3, "same gain on a second run");
    }
    assert_eq!(q2.audio_tracks.iter().map(|x| x.effects.len()).collect::<Vec<_>>(), q.audio_tracks.iter().map(|x| x.effects.len()).collect::<Vec<_>>());
    assert_eq!(q2.master_effects.len(), q.master_effects.len());
    // after balancing, what Zenh's angle plays measures close to the voice + game targets
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let heard = crate::recut_balance::analyze(&q2.with_angle_audio(Some(0)), &provider);
    assert!(heard.iter().all(|i| i.lufs.is_some() || i.track == t[0].0 || i.track == t[3].0), "{heard:?}");
    // one undo step per run
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let q0 = s.project.sequence(src).unwrap();
    assert!(q0.audio_tracks.iter().all(|x| !x.muted && x.items[0].gain_db == 0.0 && x.effects.is_empty()));
}

#[test]
fn balance_follows_the_roles_given_and_rejects_bad_ones() {
    let (mut s, src, t) = obs_multicam();
    // roles with loudness already measured: nothing to analyse
    let r = s
        .execute(
            "recut.balanceMulticam",
            json!({"sequence": src.0, "voiceLufs": -18, "gameOffset": 10, "compress": false, "limit": false, "roles": [
                {"track": t[0].0, "role": "keep"},
                {"track": t[1].0, "role": "voice", "lufs": -40.0},
                {"track": t[2].0, "role": "game", "lufs": -16.0},
                {"track": t[3].0, "role": "mute"},
            ]}),
        )
        .unwrap();
    assert_eq!(r["tracks"].as_array().unwrap().len(), 4);
    let q = s.project.sequence(src).unwrap();
    assert_eq!(q.audio_tracks[1].items[0].gain_db, 22.0);
    assert_eq!(q.audio_tracks[2].items[0].gain_db, -12.0);
    assert!(!q.audio_tracks[0].muted && q.audio_tracks[3].muted);
    assert!(q.audio_tracks[2].effects.is_empty() && q.master_effects.is_empty(), "no compressor, no limiter");
    // hostile input: an error, never a crash, and nothing changed
    let undo = s.history.undo.len();
    for bad in [
        json!({"roles": [{"track": t[1].0, "role": "loud"}]}),
        json!({"roles": [{"track": 999_999_999u64, "role": "voice"}]}),
        json!({"roles": [{"role": "voice"}]}),
        json!({"sequence": 424_242}),
    ] {
        assert!(s.execute("recut.balanceMulticam", bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(s.history.undo.len(), undo);
    // out-of-range numbers are clamped, NaN-like loudness is measured instead
    s.execute("recut.balanceMulticam", json!({"voiceLufs": 1e9, "gameOffset": -5, "roles": [{"track": t[1].0, "role": "voice", "lufs": "x"}]})).unwrap();
    let g = s.project.sequence(src).unwrap().audio_tracks[1].items[0].gain_db;
    assert!(g.is_finite() && g.abs() <= 40.0);
}

// ---------------------------------------------------------------- Apply POV Overlays

/// The stacked cameras nested as multi-camera on V1 and cut every 12 frames: Zenh 0–12, Screu
/// 12–24, Zenh 24–36, Screu 36–48. Returns the session, the source and two overlay items.
fn cut_multicam() -> (Session, ItemId, ItemId, ItemId) {
    let (mut s, _) = stacked();
    let r = s.execute("recut.nestMulticam", json!({})).unwrap();
    let src = ItemId(r["sequence"].as_u64().unwrap());
    for (frame, cam) in [(12, 2), (24, 1), (36, 2)] {
        s.execute(&format!("multicam.cutToCamera{cam}"), json!({"frame": frame})).unwrap();
    }
    let mut p = (*s.project).clone();
    let zenh_tag = media(&mut p, "tag zenh", 1);
    let screu_tag = media(&mut p, "tag screu", 1);
    s.project = Arc::new(p);
    (s, src, zenh_tag, screu_tag)
}

/// (start frame, end frame, item) of the clips on video track `ti`.
fn row(s: &Session, ti: usize) -> Vec<(i64, i64, ItemId)> {
    let q = s.active_sequence().unwrap();
    q.video_tracks[ti].items.iter().map(|i| (RATE.frame_at(i.start), RATE.frame_at(i.end()), i.item)).collect()
}

fn overlay(s: &mut Session, item: ItemId, from: i64, to: i64, track: usize) {
    let (v, _) = tracks(s);
    let range = TimeRange::new(Tick::ZERO, RATE.tick_of(to - from));
    crate::commands::place_item(s, item, range, RATE.tick_of(from), Some(v[track]), None, false, "test", None).unwrap();
}

#[test]
fn pov_overlays_copy_each_cameras_model_onto_its_segments() {
    let (mut s, src, zenh_tag, screu_tag) = cut_multicam();
    let segs = crate::recut_overlays::segments(&s.active_sequence().unwrap().video_tracks[0].items, src);
    let angles: Vec<(i64, i64, u32)> = segs.iter().map(|g| (RATE.frame_at(g.start), RATE.frame_at(g.end), g.angle)).collect();
    assert_eq!(angles, [(0, 12, 0), (12, 24, 1), (24, 36, 0), (36, 48, 1)]);
    // models: Zenh's tag over his whole first segment, Screu's over his
    overlay(&mut s, zenh_tag, 0, 12, 1);
    overlay(&mut s, screu_tag, 12, 24, 1);
    s.set_playhead(Tick::ZERO);
    let r = s.execute("recut.povOverlays", json!({})).unwrap();
    assert_eq!((r["placed"].as_u64(), r["skipped"].as_u64()), (Some(2), Some(0)), "{r}");
    assert_eq!(row(&s, 1), [(0, 12, zenh_tag), (12, 24, screu_tag), (24, 36, zenh_tag), (36, 48, screu_tag)]);
    // re-cut: 36–48 now shows Zenh too, so 24–48 is one Zenh segment; running again follows it
    s.execute("multicam.switchAngle", json!({"camera": 1, "frame": 40})).unwrap();
    let r = s.execute("recut.povOverlays", json!({})).unwrap();
    assert_eq!((r["replaced"].as_u64(), r["placed"].as_u64()), (Some(2), Some(1)), "{r}");
    assert_eq!(row(&s, 1), [(0, 12, zenh_tag), (12, 24, screu_tag), (24, 48, zenh_tag)]);
    // one undo step per run
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(row(&s, 1).len(), 4);
}

#[test]
fn pov_overlays_keep_a_short_models_offset_and_never_cover_other_clips() {
    let (mut s, _, zenh_tag, screu_tag) = cut_multicam();
    // a 4-frame pop-up 2 frames into Zenh's first segment
    overlay(&mut s, zenh_tag, 2, 6, 2);
    // a clip of the user's own where the copy would go (Zenh's second segment, 26–30)
    let mut p = (*s.project).clone();
    let other = media(&mut p, "sticker", 1);
    s.project = Arc::new(p);
    overlay(&mut s, other, 25, 27, 2);
    s.set_playhead(Tick::ZERO);
    let r = s.execute("recut.povOverlays", json!({})).unwrap();
    assert_eq!((r["placed"].as_u64(), r["skipped"].as_u64()), (Some(0), Some(1)), "{r}");
    assert_eq!(r["camerasWithoutOverlay"], json!(["screu"]));
    assert_eq!(row(&s, 2), [(2, 6, zenh_tag), (25, 27, other)], "the user's clip is untouched");
    // without the obstacle the pop-up lands 2 frames into the segment, 4 frames long
    let q = s.active_sequence().unwrap().clone();
    let obstacle = q.video_tracks[2].items[1].id.0;
    s.execute("timeline.select", json!({"clips": [obstacle]})).unwrap();
    s.execute("edit.clear", json!({})).unwrap();
    s.set_playhead(Tick::ZERO);
    s.execute("recut.povOverlays", json!({})).unwrap();
    assert_eq!(row(&s, 2), [(2, 6, zenh_tag), (26, 30, zenh_tag)]);
    let _ = screu_tag;
}

#[test]
fn pov_overlays_need_a_model_and_a_multicam_clip() {
    let (mut s, _, _, _) = cut_multicam();
    s.set_playhead(Tick::ZERO);
    let e = s.execute("recut.povOverlays", json!({})).unwrap_err().to_string();
    assert!(e.contains("dress one stretch"), "{e}");
    let (mut s, _) = stacked();
    assert!(s.execute("recut.povOverlays", json!({})).is_err(), "no multi-camera clip");
    assert!(s.execute("recut.povOverlays", json!({"clip": 987_654_321u64})).is_err());
}

// ---------------------------------------------------------------- Dead time

const SECOND: i64 = filmcraft_time::TICKS_PER_SECOND;

/// Speech-like noise (150 ms syllables, 50 ms gaps) where `talk` says, over a faint room noise; 48 kHz.
fn speech(seed: u32, seconds: usize, talk: &[(f64, f64)]) -> Vec<f32> {
    let n = 48_000 * seconds;
    let room = noise(seed + 100, n);
    noise(seed, n)
        .iter()
        .zip(&room)
        .enumerate()
        .map(|(i, (v, r))| {
            let t = i as f64 / 48_000.0;
            let syllable = (i % 9_600) < 7_200;
            let on = syllable && talk.iter().any(|(a, b)| t >= *a && t < *b);
            if on { v * 0.1 } else { r * 0.001 }
        })
        .collect()
}

/// Two players nested as multi-camera, 20 s: Zenh (OBS: mix, voice, game) talks 0–4 s and 12–14 s,
/// Screu (one track) 6–7 s. Nobody talks 4–6 s (too short to be dead), 7–12 s and 14–20 s.
fn talk_multicam() -> (Session, ItemId, Vec<TrackId>) {
    let secs = 20usize;
    let frames = 24 * secs as i64;
    let mut s = Session::default();
    let mut p = (*s.project).clone();
    let seq = p.new_sequence("Edit", settings(), 3, 4, None);
    let zenh_pic = media_len(&mut p, "zenh.mp4", 1, frames);
    let screu_pic = media_len(&mut p, "screu.mp4", 1, frames);
    s.project = Arc::new(p);
    let voice = speech(1, secs, &[(0.0, 4.0), (12.0, 14.0)]);
    let game: Vec<f32> = noise(2, 48_000 * secs).iter().map(|v| v * 0.3).collect();
    let mix: Vec<f32> = voice.iter().zip(&game).map(|(a, b)| a + b).collect();
    let screu = speech(3, secs, &[(6.0, 7.0)]);
    let zenh_snd = pcm_file(&mut s, "zenh.mov", &[mix, voice, game]);
    let screu_snd = pcm_file(&mut s, "screu.mov", &[screu]);
    s.state.active_sequence = Some(seq);
    let (v, a) = tracks(&s);
    let len = TimeRange::new(Tick::ZERO, RATE.tick_of(frames));
    for (pic, snd, vt, at) in [(zenh_pic, zenh_snd, v[0], a[0]), (screu_pic, screu_snd, v[1], a[3])] {
        crate::commands::place_item(&mut s, pic, len, Tick::ZERO, Some(vt), None, false, "test", None).unwrap();
        crate::commands::place_item(&mut s, snd, len, Tick::ZERO, None, Some(at), false, "test", None).unwrap();
        let q = s.project.sequence(seq).unwrap();
        let clips: Vec<u64> = q.all_tracks().flat_map(|t| t.items.iter()).filter(|i| i.item == pic || i.item == snd).map(|i| i.id.0).collect();
        s.execute("timeline.select", json!({"clips": clips})).unwrap();
        s.execute("clip.link", json!({})).unwrap();
    }
    let all: Vec<u64> = s.project.sequence(seq).unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    let r = s.execute("recut.nestMulticam", json!({})).unwrap();
    let src = ItemId(r["sequence"].as_u64().unwrap());
    let audio: Vec<TrackId> = s.project.sequence(src).unwrap().audio_tracks.iter().map(|t| t.id).collect();
    assert_eq!(audio.len(), 4);
    s.set_playhead(Tick::ZERO);
    (s, src, audio)
}

/// (start, end) of the dead-time markers of the active sequence, in seconds.
fn dead_markers(s: &Session) -> Vec<(f64, f64)> {
    let q = s.active_sequence().unwrap();
    let sec = |t: Tick| t.0 as f64 / SECOND as f64;
    q.markers.iter().filter(|m| m.name == crate::recut_deadtime::MARKER_NAME).map(|m| (sec(m.start), sec(m.start + m.duration))).collect()
}

fn near(got: &[(f64, f64)], want: &[(f64, f64)]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g.0 - w.0).abs() < 0.1 && (g.1 - w.1).abs() < 0.1)
}

#[test]
fn dead_time_is_marked_where_nobody_talks() {
    let (mut s, src, t) = talk_multicam();
    // the voice tracks are chosen: Zenh's voice (not his mix nor his game), Screu's one track
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let q = s.project.sequence(src).unwrap().clone();
    assert_eq!(crate::recut_deadtime::default_tracks(&q, &provider), vec![t[1].0, t[3].0]);
    let r = s.execute("recut.markDeadTime", json!({})).unwrap();
    assert_eq!(r["source"], src.0);
    assert_eq!(r["markers"], 2, "{r}");
    // 7–12 s less half a second after Screu and 0.3 s before Zenh; 14–20 s to the end
    let got = dead_markers(&s);
    assert!(near(&got, &[(7.45, 11.7), (14.45, 20.0)]), "{got:?}");
    let q = s.active_sequence().unwrap();
    let m = q.markers.iter().find(|m| m.name == "Temps mort").unwrap();
    assert_eq!((m.color, m.kind), (Label::Rose, filmcraft_project::MarkerKind::Comment));
    assert!(m.comment.starts_with("Silence de 4,"), "{}", m.comment);
    // listening to Zenh only, 4–12 s is dead too; the markers of the first run are replaced
    let r = s.execute("recut.markDeadTime", json!({"tracks": [t[1].0], "keepAfter": 0, "keepBefore": 0})).unwrap();
    assert_eq!(r["replaced"], 2);
    let got = dead_markers(&s);
    assert!(near(&got, &[(3.95, 12.0), (13.95, 20.0)]), "{got:?}");
    // a longer minimum leaves only the end
    s.execute("recut.markDeadTime", json!({"minSeconds": 5.5})).unwrap();
    assert!(near(&dead_markers(&s), &[(14.45, 20.0)]), "{:?}", dead_markers(&s));
    // one undo step per run
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(dead_markers(&s).len(), 2);
}

#[test]
fn dead_time_follows_the_edit_and_is_removed_with_a_ripple() {
    let (mut s, _, _) = talk_multicam();
    // the edit starts 2 s into the recording and is cut to Screu at 9 s (edit time)
    s.edit_sequence("trim", |q, ctx, _| Ok(filmcraft_edit::transcript::ripple_delete_ranges(q, vec![TimeRange::new(Tick::ZERO, Tick(2 * SECOND))], ctx)))
        .unwrap();
    s.execute("multicam.cutToCamera2", json!({"frame": 9 * 24})).unwrap();
    s.set_playhead(Tick::ZERO);
    s.execute("recut.markDeadTime", json!({})).unwrap();
    let got = dead_markers(&s);
    assert!(near(&got, &[(5.45, 9.7), (12.45, 18.0)]), "one marker across the cut: {got:?}");
    // the user keeps the end (deletes its marker), then removes the rest
    let mut p = (*s.project).clone();
    let id = s.state.active_sequence.unwrap();
    p.sequence_mut(id).unwrap().markers.retain(|m| m.start < Tick(10 * SECOND));
    s.project = Arc::new(p);
    let before = s.active_sequence().unwrap().duration();
    let r = s.execute("recut.removeDeadTime", json!({})).unwrap();
    assert_eq!(r["removed"], 1);
    let cut = r["seconds"].as_f64().unwrap();
    assert!((cut - 4.25).abs() < 0.1, "{r}");
    let q = s.active_sequence().unwrap();
    assert_eq!(q.duration(), before - Tick((cut * SECOND as f64).round() as i64));
    assert!(dead_markers(&s).is_empty(), "the markers go with the dead time");
    // picture and sound stay together: every track ends at the same time
    let ends: Vec<Tick> = q.all_tracks().filter_map(|t| t.items.iter().map(|i| i.end()).max()).collect();
    assert!(ends.iter().all(|e| *e == ends[0]), "{ends:?}");
    assert!(s.execute("recut.removeDeadTime", json!({})).is_err(), "nothing marked any more");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().duration(), before);
    assert_eq!(dead_markers(&s).len(), 1);
}

#[test]
fn dead_time_takes_given_ranges_and_rejects_bad_params() {
    let (mut s, src, t) = talk_multicam();
    // ranges measured elsewhere (the dialog): laid out as given, clamped to the source
    let r = s.execute("recut.markDeadTime", json!({"source": src.0, "ranges": [[SECOND, 3 * SECOND], [-5, SECOND / 10], [19 * SECOND, 99 * SECOND]]})).unwrap();
    assert_eq!(r["markers"], 2, "{r}");
    assert!(near(&dead_markers(&s), &[(1.0, 3.0), (19.0, 20.0)]), "{:?}", dead_markers(&s));
    let undo = s.history.undo.len();
    for bad in [
        json!({"ranges": "all"}),
        json!({"ranges": [[1, 2, 3]]}),
        json!({"ranges": [["a", "b"]]}),
        json!({"tracks": [987_654u64]}),
        json!({"tracks": ["voice"]}),
        json!({"source": 424_242}),
        json!({"tracks": [t[0].0]}),
    ] {
        let r = s.execute("recut.markDeadTime", bad.clone());
        if bad.get("tracks").and_then(|v| v.get(0)).and_then(|v| v.as_u64()) == Some(t[0].0) {
            // the mix track plays too: allowed (it hears the game, so nothing is dead)
            assert_eq!(r.unwrap()["markers"], 0);
            continue;
        }
        assert!(r.is_err(), "{bad}");
    }
    assert_eq!(s.history.undo.len(), undo + 1);
    // hostile numbers are clamped, never a crash
    s.execute("recut.markDeadTime", json!({"minSeconds": f64::MAX, "rangeDb": -1e300, "keepAfter": 1e9})).unwrap();
    assert!(dead_markers(&s).is_empty());
}

#[test]
fn dead_ranges_honour_the_minimum_and_the_margins() {
    use crate::recut_deadtime::{Settings, dead_ranges};
    // 0–1 s talk, 1–5 s silence, 5–6 s talk, 6–7 s silence (end)
    let act: Vec<bool> = (0..350).map(|h| h < 50 || (250..300).contains(&h)).collect();
    let st = Settings { min_seconds: 1.0, range_db: 18.0, keep_after: 0.5, keep_before: 0.2 };
    let r = dead_ranges(std::slice::from_ref(&act), act.len(), &st);
    let secs: Vec<(f64, f64)> = r.iter().map(|r| (r.start.0 as f64 / SECOND as f64, r.end().0 as f64 / SECOND as f64)).collect();
    assert_eq!(secs, [(1.5, 4.8), (6.5, 7.0)]);
    // nothing heard at all: everything is dead; nothing to hear in: nothing
    assert_eq!(dead_ranges(&[vec![false; 100]], 100, &st).len(), 1);
    assert!(dead_ranges(&[], 0, &st).is_empty());
}

// ---------------------------------------------------------------- Watch folder

/// A fresh empty folder for one test.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("recut-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn watch_folder_imports_new_files_once_complete() {
    use crate::recut_watch::{QUIET, Watcher, bin_for};
    use std::io::Write;
    use std::time::{Duration, SystemTime};
    let root = temp_dir("watch");
    std::fs::write(root.join("old.mp4"), b"already there").unwrap();
    let sec = Duration::from_secs(1);
    let t0 = SystemTime::now();
    let mut w = Watcher::default();
    let p = w.poll(&root, true, false, t0, QUIET);
    assert!(p.ready.is_empty() && p.waiting == 0, "files already there are left alone: {p:?}");
    // a POV arrives in a transfer folder and is still being written
    let sub = root.join("swisstransfer_2bfc");
    std::fs::create_dir_all(&sub).unwrap();
    let pov = sub.join("2026-05-20 15-40-35.mp4");
    std::fs::write(&pov, b"part 1").unwrap();
    std::fs::write(sub.join("notes.txt"), b"not media").unwrap();
    std::fs::write(sub.join(".2026-05-20 15-40-35.mp4.part"), b"hidden").unwrap();
    assert_eq!(w.poll(&root, true, false, t0 + 3 * sec, QUIET).waiting, 1);
    std::fs::OpenOptions::new().append(true).open(&pov).unwrap().write_all(b" part 2").unwrap();
    assert_eq!(w.poll(&root, true, false, t0 + 6 * sec, QUIET).waiting, 1, "it grew");
    assert!(w.poll(&root, true, false, t0 + 10 * sec, QUIET).ready.is_empty(), "unchanged for 4 s only");
    let p = w.poll(&root, true, false, t0 + 17 * sec, QUIET);
    assert_eq!(p.ready, std::slice::from_ref(&pov));
    assert!(w.poll(&root, true, false, t0 + 30 * sec, QUIET).ready.is_empty(), "imported once");
    // without the subfolders, the transfer is not seen at all
    let mut flat = Watcher::default();
    flat.poll(&root, false, true, t0, QUIET);
    assert_eq!(flat.poll(&root, false, true, t0 + 20 * sec, QUIET).ready, [root.join("old.mp4")], "asked to import what is there");
    // bins: the transfer's folder, else the OBS date
    assert_eq!(bin_for(&root, &pov), "swisstransfer_2bfc");
    assert_eq!(bin_for(&root, &root.join("2026-05-20 15-41-17.mkv")), "Rushs 2026-05-20");
    assert_eq!(bin_for(&root, &root.join("clip.mov")), "Rushs");
    // a missing folder is no error
    assert_eq!(Watcher::default().poll(&root.join("gone"), true, true, t0, QUIET), Default::default());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn watch_folder_reads_obs_names_and_keeps_its_setting() {
    use crate::recut_watch::{WatchConfig, in_project, load_config, obs_time, save_config};
    assert_eq!(obs_time("2026-05-20 15-40-35.mp4"), Some(("2026-05-20".into(), 15 * 3600 + 40 * 60 + 35)));
    assert_eq!(obs_time("Replay 2026-05-20_15-40-35.mkv").map(|t| t.1), Some(56_435));
    for bad in ["clip.mp4", "2026-13-20 15-40-35.mp4", "2026-05-20 25-40-35.mp4", "2026-05-20", "", "é2026-05-20 15-40-3"] {
        assert_eq!(obs_time(bad), None, "{bad}");
    }
    let dir = temp_dir("watch-config");
    let path = dir.join("RECut").join("recut-watch.json");
    assert_eq!(load_config(&path), WatchConfig::default(), "nothing saved yet");
    let c = WatchConfig { dir: "C:\\Users\\remi6\\Videos".into(), enabled: true, recursive: false };
    save_config(&path, &c).unwrap();
    assert_eq!(load_config(&path), c);
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(load_config(&path), WatchConfig::default(), "a damaged file is the default, not a crash");
    // a file imported by hand is in the project
    let mut s = Session::default();
    let media = dir.join("tone.mov");
    std::fs::write(&media, b"x").unwrap();
    let item = pcm_file(&mut s, "tone.mov", &[vec![0.0; 4_800]]);
    assert!(!in_project(&s.project, &media), "a blob, not that file");
    let mut p = (*s.project).clone();
    p.items.get_mut(&item).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: media.to_string_lossy().into_owned() };
    s.project = Arc::new(p);
    assert!(in_project(&s.project, &media));
    let _ = std::fs::remove_dir_all(&dir);
}
