//! RECut: UI tests of the RECut additions (RECUT.md): the timeline clip menu's audio settings and
//! Nest as Multi-Camera, and the search field of the Graphics font list.

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
    for id in ["timeline.clipMenu.clip.audioGain", "timeline.clipMenu.clip.audioChannels", "timeline.clipMenu.recut.nestMulticam"] {
        assert!(d.has(id), "{id} in the clip menu");
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
