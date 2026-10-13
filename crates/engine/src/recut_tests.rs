//! RECut: tests of the RECut additions (RECUT.md): Nest as Multi-Camera, and multi-stream cameras
//! in Create Multi-Camera Source Sequence.

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
    let source = filmcraft_media::generators::GeneratorSource::new(filmcraft_media::Generator::BarsAndTone, 32, 32, RATE, RATE.tick_of(FRAMES));
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
    // the edit: one picture and one sound clip, linked, both multi-camera on angle 0
    let q = s.active_sequence().unwrap();
    let clips: Vec<_> = q.all_tracks().flat_map(|t| t.items.iter()).collect();
    assert_eq!(clips.len(), 2);
    assert!(clips.iter().all(|c| c.item == src && c.multicam.is_some_and(|m| m.enabled && m.angle == 0)));
    assert_eq!(clips[0].link, clips[1].link);
    assert!(clips[0].link.is_some());
    assert_eq!((q.video_tracks[0].items.len(), q.audio_tracks[0].items.len()), (1, 1));
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
