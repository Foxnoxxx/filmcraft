//! RECut: UI tests of the RECut additions (RECUT.md): the timeline clip menu's audio settings,
//! Nest as Multi-Camera, Balance Multi-Camera Sound, Apply POV Overlays, Mark Dead Time and the watch
//! folder, and the search field of the Graphics font list. With `RECUT_UI_SHOTS=<dir>` the balance and dead-time
//! dialogs are also rendered (wgpu) to PNGs there.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use filmcraft_engine::Session;
use filmcraft_project::{ClipId, TrackItem};
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 1100.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn has(&mut self, id: &str) -> bool {
        self.frames(2);
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().any(|e| e["id"] == id)
    }
    fn click(&mut self, id: &str) {
        assert!(self.has(id), "no element {id}");
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }
    fn item(&self, clip: u64) -> TrackItem {
        self.harness.state().session.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.clone()
    }
}

/// The demo's first video clip and the audio clip linked to it.
fn first_pair(d: &mut Driver) -> (u64, u64) {
    let seq = d.exec("sequence.inspect", json!({}));
    let v = &seq["video"][0]["items"][0];
    let a = seq["audio"][0]["items"].as_array().unwrap().iter().find(|a| a["link"] == v["link"]).expect("linked audio").clone();
    (v["clip"].as_u64().unwrap(), a["clip"].as_u64().unwrap())
}

#[test]
fn clip_menu_offers_audio_settings_and_nest_as_multicam() {
    let mut d = Driver::demo();
    let (_, audio) = first_pair(&mut d);
    // a Project panel item selected too: Audio Channels… from the timeline still edits the clip
    let item = d.item(audio).item;
    d.harness.state_mut().session.state.project_selection = vec![item];
    d.ok("ui.click", json!({"id": format!("timeline.clip.{audio}"), "button": "right"}));
    d.frames(3);
    for id in ["timeline.clipMenu.clip.audioGain", "timeline.clipMenu.clip.audioChannels", "timeline.clipMenu.recut.menu"] {
        assert!(d.has(id), "{id} in the clip menu");
    }
    // RECut's commands are in their own submenu
    d.click("timeline.clipMenu.recut.menu");
    for id in ["recut.nestMulticam", "recut.balanceMulticam", "recut.povOverlays", "recut.markDeadTime"] {
        assert!(d.has(&format!("timeline.clipMenu.{id}")), "{id} in Clip menu ▸ RECut");
    }
    d.click("timeline.clipMenu.clip.audioChannels");
    // timeline mode: one row of source channels; untick channel 2 → mono from channel 1
    d.click("audioChannels.clip.0.ch.1");
    d.click("audioChannels.ok");
    assert_eq!(d.item(audio).source_channels, [0], "the timeline clip plays channel 1 alone");
    let media = d.harness.state().session.project.item(item).unwrap().as_media().unwrap().interpret.audio_channels.clone();
    assert_eq!(media, None, "the Project panel item is unchanged");
    // Audio Gain… opens its dialog
    d.ok("ui.click", json!({"id": format!("timeline.clip.{audio}"), "button": "right"}));
    d.frames(3);
    d.click("timeline.clipMenu.clip.audioGain");
    assert!(d.has("audioGain.ok"), "Audio Gain dialog");
}

#[test]
fn font_list_has_a_search_field() {
    let mut d = Driver::demo();
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    let r = d.exec("graphics.newText", json!({"text": "POV Zenh"}));
    let clip = r["clip"].as_u64().unwrap();
    d.exec("graphics.selectLayer", json!({"clip": clip, "layers": [0]}));
    d.frames(4);
    d.click("graphics.prop.font");
    assert!(d.has("graphics.prop.fontSearch"), "search field in the font list");
    // typing goes straight into the search field and keeps the matching fonts
    d.ok("ui.type", json!({"text": "jetbrains"}));
    d.frames(3);
    assert!(d.harness.query_by_label("Noto Serif").is_none(), "fonts without the text are hidden");
    d.harness.get_by_label("JetBrains Mono").click();
    d.frames(4);
    let font = {
        let s = &d.harness.state().session;
        let (_, it) = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap();
        let e = it.effects.iter().find(|e| e.effect == "graphic_text").unwrap();
        e.param("font").unwrap().value_at(filmcraft_time::Tick::ZERO)
    };
    assert_eq!(font, filmcraft_project::ParamValue::Text("JetBrains Mono".into()));
}

/// Two cameras (bars and tone, picture and sound linked), stacked on V1/A1 and V2/A2, nested as
/// multi-camera: returns the driver, the multi-camera picture clip and the source sequence.
fn multicam_driver() -> (Driver, u64, u64) {
    use filmcraft_media::MediaSource;
    use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef, SequenceSettings};
    use filmcraft_time::FrameRate;
    let rate = FrameRate::FPS_24;
    let mut s = Session::default();
    let mut p = (*s.project).clone();
    let seq = p.new_sequence("Cut", SequenceSettings { width: 64, height: 36, frame_rate: rate, ..Default::default() }, 2, 2, None);
    let mut items = Vec::new();
    for name in ["zenh.mp4", "screu.mp4"] {
        let src = filmcraft_media::generators::GeneratorSource::new(filmcraft_media::Generator::BarsAndTone, 64, 36, rate, rate.tick_of(48));
        let clip = MediaClip {
            media: MediaRef::Generator(filmcraft_media::Generator::BarsAndTone),
            info: src.info().clone(),
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        };
        items.push(p.add_item(name, Label::Iris, ItemKind::Media(clip), None));
    }
    s.project = std::sync::Arc::new(p);
    s.state.active_sequence = Some(seq);
    s.state.open_sequences = vec![seq];
    for (item, (v, a)) in items.iter().zip([("V1", "A1"), ("V2", "A2")]) {
        s.execute("timeline.place", json!({"item": item.0, "track": v, "audioTrack": a, "frame": 0})).unwrap();
    }
    let all: Vec<u64> = s.active_sequence().unwrap().all_tracks().flat_map(|t| t.items.iter().map(|i| i.id.0)).collect();
    s.execute("timeline.select", json!({"clips": all})).unwrap();
    let r = s.execute("recut.nestMulticam", json!({})).unwrap();
    let clips: Vec<u64> = r["clips"].as_array().unwrap().iter().map(|c| c.as_u64().unwrap()).collect();
    let src = r["sequence"].as_u64().unwrap();
    let (tx, rx) = channel();
    let app = FilmcraftApp::new(s).with_control(rx);
    let mut b = Harness::builder().with_size(egui::vec2(1600.0, 1100.0)).with_max_steps(10_000);
    if std::env::var_os("RECUT_UI_SHOTS").is_some() {
        b = b.wgpu().with_pixels_per_point(1.0);
    }
    let harness = b.build_eframe(move |_cc| app);
    let mut d = Driver { harness, tx };
    d.frames(4);
    (d, clips[0], src)
}

#[test]
fn balance_dialog_analyses_the_tracks_and_applies_the_roles() {
    let (mut d, clip, src) = multicam_driver();
    let tracks: Vec<u64> = d.harness.state().session.project.sequence(filmcraft_project::ItemId(src)).unwrap().audio_tracks.iter().map(|t| t.id.0).collect();
    assert_eq!(tracks.len(), 2);
    d.ok("ui.click", json!({"id": format!("timeline.clip.{clip}"), "button": "right"}));
    d.frames(3);
    d.click("timeline.clipMenu.recut.menu");
    d.click("timeline.clipMenu.recut.balanceMulticam");
    // the analysis runs in the background; the role buttons appear when it is done
    let row = format!("balance.{}.voice", tracks[0]);
    let mut ready = false;
    for _ in 0..200 {
        if d.has(&row) {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(ready, "analysis finished");
    if let Some(dir) = std::env::var_os("RECUT_UI_SHOTS").map(std::path::PathBuf::from) {
        d.frames(6);
        let img = d.harness.render().expect("wgpu render");
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join("balance-dialog.png")).unwrap();
    }
    // one camera muted, the other kept as it is; no limiter
    d.click(&format!("balance.{}.mute", tracks[0]));
    d.click(&format!("balance.{}.keep", tracks[1]));
    d.click("balance.limit");
    d.click("balance.ok");
    assert!(!d.has("balance.ok"), "dialog closed: {}", d.harness.state().ui.status);
    let q = d.harness.state().session.project.sequence(filmcraft_project::ItemId(src)).unwrap().clone();
    assert!(q.audio_tracks[0].muted && !q.audio_tracks[1].muted);
    assert!(q.master_effects.is_empty(), "limiter unticked");
    let undo = d.exec("history.list", json!({}));
    assert_eq!(undo["undo"].as_array().unwrap().last().and_then(Value::as_str), Some("Balance Multi-Camera Sound"));
    // Escape closes the dialog without changing anything
    d.ok("ui.click", json!({"id": format!("timeline.clip.{clip}"), "button": "right"}));
    d.frames(3);
    d.click("timeline.clipMenu.recut.menu");
    d.click("timeline.clipMenu.recut.balanceMulticam");
    assert!(d.has("balance.cancel"));
    d.ok("ui.key", json!({"key": "Escape"}));
    d.frames(3);
    assert!(!d.has("balance.cancel"));
}

#[test]
fn clip_menu_applies_pov_overlays() {
    use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef, TrackKind};
    use filmcraft_time::{FrameRate, Tick, TimeRange};
    let (mut d, clip, _) = multicam_driver();
    // cut: zenh 0–12, screu 12–24, zenh 24–36, screu 36–48
    for (frame, cam) in [(12, 2), (24, 1), (36, 2)] {
        d.exec(&format!("multicam.cutToCamera{cam}"), json!({"frame": frame}));
    }
    // a model overlay over Zenh's first stretch, on V2
    let rate = FrameRate::FPS_24;
    {
        use filmcraft_media::MediaSource;
        let s = &mut d.harness.state_mut().session;
        let seq = s.state.active_sequence.unwrap();
        let mut p = (*s.project).clone();
        let src = filmcraft_media::generators::GeneratorSource::new(filmcraft_media::Generator::BarsAndTone, 64, 36, rate, rate.tick_of(48));
        let info = src.info().clone();
        let media = MediaClip {
            media: MediaRef::Generator(filmcraft_media::Generator::BarsAndTone),
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        };
        let tag = p.add_item("tag zenh", Label::Iris, ItemKind::Media(media), None);
        let ti = p.make_track_item(tag, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, rate.tick_of(12)), rate).unwrap();
        p.sequence_mut(seq).unwrap().video_tracks[1].items.push(ti);
        s.project = std::sync::Arc::new(p);
    }
    d.frames(3);
    d.ok("ui.click", json!({"id": format!("timeline.clip.{clip}"), "button": "right"}));
    d.frames(3);
    d.click("timeline.clipMenu.recut.menu");
    d.click("timeline.clipMenu.recut.povOverlays");
    let s = &d.harness.state().session;
    let v2: Vec<i64> = s.active_sequence().unwrap().video_tracks[1].items.iter().map(|i| rate.frame_at(i.start)).collect();
    assert_eq!(v2, [0, 24], "Zenh's overlay copied onto his second stretch");
    let status = d.harness.state().ui.status.clone();
    assert!(status.starts_with("Habillages posés : 1") && status.contains("sans modèle : screu"), "{status}");
}

#[test]
fn dead_time_dialog_marks_and_the_menu_removes() {
    let (mut d, clip, src) = multicam_driver();
    let src = filmcraft_project::ItemId(src);
    // Screu's sound is silenced (clip gain −100 dB): with Zenh's tone unticked, nobody talks
    let tracks: Vec<u64> = {
        let s = &mut d.harness.state_mut().session;
        let mut p = (*s.project).clone();
        let q = p.sequence_mut(src).unwrap();
        for it in &mut q.audio_tracks[1].items {
            it.gain_db = -100.0;
        }
        let t = q.audio_tracks.iter().map(|t| t.id.0).collect();
        s.project = std::sync::Arc::new(p);
        t
    };
    d.ok("ui.click", json!({"id": format!("timeline.clip.{clip}"), "button": "right"}));
    d.frames(3);
    d.click("timeline.clipMenu.recut.menu");
    d.click("timeline.clipMenu.recut.markDeadTime");
    // the tracks are listened to in the background; their checkboxes appear when it is done
    let zenh = format!("deadtime.track.{}", tracks[0]);
    let mut ready = false;
    for _ in 0..200 {
        if d.has(&zenh) {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(ready, "analysis finished");
    // Zenh's bars and tone never stop: no dead time while his track is ticked
    assert!(d.has("deadtime.ok"));
    d.click("deadtime.ok");
    assert!(d.has("deadtime.ok"), "nothing to mark: the dialog stays");
    d.click(&zenh);
    // the 2-second recording is shorter than the 3-second minimum: type 1 s
    d.click("deadtime.min");
    d.ok("ui.key", json!({"key": "A", "modifiers": ["command"]}));
    d.ok("ui.type", json!({"text": "1"}));
    d.ok("ui.key", json!({"key": "Enter"}));
    d.frames(3);
    if let Some(dir) = std::env::var_os("RECUT_UI_SHOTS").map(std::path::PathBuf::from) {
        d.frames(6);
        let img = d.harness.render().expect("wgpu render");
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join("deadtime-dialog.png")).unwrap();
    }
    d.click("deadtime.range.0");
    d.click("deadtime.ok");
    assert!(!d.has("deadtime.ok"), "dialog closed: {}", d.harness.state().ui.status);
    let status = d.harness.state().ui.status.clone();
    assert!(status.starts_with("Temps morts marqués : 1 (2,0 s)"), "{status}");
    let markers: Vec<String> = d.harness.state().session.active_sequence().unwrap().markers.iter().map(|m| m.name.clone()).collect();
    assert_eq!(markers, ["Temps mort"]);
    // Sequence ▸ Remove Marked Dead Time: the whole (silent) edit goes
    d.ok("ui.menu.invoke", json!({"id": "recut.removeDeadTime"}));
    d.frames(3);
    let status = d.harness.state().ui.status.clone();
    assert!(status.starts_with("Temps morts supprimés : 1 passage(s), 2,0 s"), "{status}");
    assert!(d.harness.state().session.active_sequence().unwrap().markers.is_empty());
}

/// A 16-bit mono WAV of `seconds` of a quiet tone.
fn wav(seconds: f64) -> Vec<u8> {
    let rate = 48_000u32;
    let n = (seconds * rate as f64) as u32;
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + n * 2).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((rate * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((n * 2).to_le_bytes());
    for i in 0..n {
        let v = ((i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin() * 3000.0) as i16;
        b.extend(v.to_le_bytes());
    }
    b
}

#[test]
fn watch_folder_imports_new_rushs_into_their_bin() {
    let mut d = Driver::demo();
    let root = std::env::temp_dir().join(format!("recut-ui-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("swisstransfer_2bfc")).unwrap();
    std::fs::write(root.join("old.wav"), wav(0.2)).unwrap();
    // File ▸ Watch Folder… opens the dialog
    d.ok("ui.menu.invoke", json!({"id": "recut.watchFolder"}));
    assert!(d.has("watch.enabled") && d.has("watch.choose"));
    d.click("watch.close");
    assert!(!d.has("watch.enabled"));
    // an agent sets it directly (not saved here; short delays for the test)
    let dir = root.to_string_lossy().into_owned();
    let r = d.exec("recut.watchFolder", json!({"dir": dir, "enabled": true, "persist": false, "quietSeconds": 0, "pollSeconds": 0.05}));
    assert_eq!(r["enabled"], true);
    assert!(
        d.call("engine.execute", json!({"command": "recut.watchFolder", "params": {"dir": "/no/such/folder", "enabled": true, "persist": false}}))["ok"]
            == false
    );
    // let the first look see what is already there, then a POV arrives
    d.frames(20);
    std::thread::sleep(std::time::Duration::from_millis(100));
    d.frames(5);
    std::fs::write(root.join("swisstransfer_2bfc").join("2026-05-20 15-40-35.wav"), wav(0.5)).unwrap();
    let named = |d: &Driver, n: &str| d.harness.state().session.project.items.values().any(|i| i.name.starts_with(n));
    let mut imported = false;
    for _ in 0..300 {
        d.frames(2);
        if named(&d, "2026-05-20 15-40-35") {
            imported = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(imported, "the new POV is imported: {}", d.harness.state().ui.status);
    let status = d.harness.state().ui.status.clone();
    assert_eq!(status, "Dossier de veille : rushs importés, 1 dans « swisstransfer_2bfc »");
    assert!(!named(&d, "old"), "what was there before is left alone");
    let project = &d.harness.state().session.project;
    let bin = project.root.children.iter().find_map(|e| match e {
        filmcraft_project::BinEntry::Bin(b) if b.name == "swisstransfer_2bfc" => Some(b.clone()),
        _ => None,
    });
    assert_eq!(bin.map(|b| b.children.len()), Some(1), "in its transfer's bin");
    // imported once
    let count = project.items.len();
    d.frames(30);
    assert_eq!(d.harness.state().session.project.items.len(), count);
    d.exec("recut.watchFolder", json!({"enabled": false}));
    let _ = std::fs::remove_dir_all(&root);
}
