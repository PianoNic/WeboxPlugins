//! Video: more camera options. A video file or a picture instead of the camera, the background blurred or replaced by a
//! picture or a looping video (person segmentation on the CPU), and cheap per-frame tweaks: mirror, brightness / contrast,
//! a soft-focus face filter, a name as a lower third, a low-light boost.
//!
//! Host methods (Webox with plugin-api 1.6.1): `camera.source` (the host decodes the file and feeds it in place of the camera,
//! so the camera stays off), `camera.filter` through `Host::camera_filter` (every camera picture, full size RGBA, on a host
//! thread under a time budget), `media.open` / `media.read` / `media.close` (the background picture or video, decoded by the
//! host). The filter is only registered while an effect is on, so with everything off the plugin costs nothing per frame.
//! Everything runs on this computer: no picture is sent anywhere but in the call itself.
mod fx;
mod i18n;
mod seg;
mod text;
#[cfg(test)]
mod bench;

use collabhost_plugin_api::{export_plugin, json_str, CameraFilter, Frame, Host, Plugin, Schema, Values};
use fx::{Behind, Img};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VIDEO_EXT: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv"];
const PICTURE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Bg { Off, Blur, Picture, Video }

#[derive(Clone, Debug, PartialEq)]
struct Cfg {
    /// The file shown instead of the camera ("" = the camera).
    source: String,
    bg: Bg, bg_file: String, blur: usize,
    mirror: bool, brightness: f32, contrast: f32,
    /// Soft-focus amount 0..1.
    soften: f32,
    name: String, low_light: bool,
}
impl Default for Cfg {
    fn default() -> Cfg { Cfg::from(&Values::default()) }
}
impl Cfg {
    fn from(v: &Values) -> Cfg {
        let t = |id: &str| v.text(id).unwrap_or("").trim().to_string();
        let source = if matches!(v.text("source"), Some("video" | "picture")) { t("source_file") } else { String::new() };
        let bg = match v.text("background") { Some("blur") => Bg::Blur, Some("picture") => Bg::Picture, Some("video") => Bg::Video, _ => Bg::Off };
        Cfg {
            source, bg, bg_file: t("background_file"), blur: v.number("blur").unwrap_or(5.0) as usize,
            mirror: v.bool("mirror").unwrap_or(false), brightness: v.number("brightness").unwrap_or(0.0) as f32,
            contrast: v.number("contrast").unwrap_or(0.0) as f32, soften: v.number("soften").unwrap_or(0.0) as f32 / 100.0,
            name: t("name"), low_light: v.bool("low_light").unwrap_or(false),
        }
    }
    /// The background effect that can actually run (a picture or video background needs its file).
    fn background(&self) -> Bg { if matches!(self.bg, Bg::Picture | Bg::Video) && self.bg_file.is_empty() { Bg::Off } else { self.bg } }
    /// Whether any per-frame effect is on (else no filter is registered at all).
    fn any_effect(&self) -> bool {
        self.background() != Bg::Off || self.mirror || self.brightness != 0.0 || self.contrast != 0.0 || self.soften > 0.0 || !self.name.is_empty() || self.low_light
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

/// A background picture or video, decoded by the host at the frame's size.
struct Clip { path: String, w: usize, h: usize, id: u64, still: bool, px: Vec<u8>, read: bool }

/// The per-frame work, owned by the frame filter (it runs on Webox's filter thread).
struct Pipe {
    host: Host,
    cfg: Arc<Mutex<Cfg>>,
    /// Started when a background effect is on (dropped, with its thread, when it goes off); `failed` = logged once.
    seg: Option<seg::Worker>,
    failed: bool,
    smooth: seg::Smoother,
    gain: f32,
    clip: Option<Clip>,
    label: Option<text::Label>,
}

impl Pipe {
    fn new(host: Host, cfg: Arc<Mutex<Cfg>>) -> Pipe {
        Pipe { host, cfg, seg: None, failed: false, smooth: seg::Smoother::new(0.6), gain: 1.0, clip: None, label: None }
    }

    fn process(&mut self, f: &mut Frame) {
        let c = lock(&self.cfg).clone();
        let mut img = Img { w: f.width, h: f.height, stride: f.stride, px: &mut *f.data };
        // 1. light: brightness, contrast and the low-light gain as one tone curve
        let target = if c.low_light { fx::low_light_gain(fx::mean_luma(&img)) } else { 1.0 };
        self.gain += (target - self.gain) * 0.1; // eases in, no pumping from frame to frame
        let lut = fx::tone_lut(c.brightness, c.contrast, self.gain);
        if !fx::is_identity(&lut) { fx::apply_lut(&mut img, &lut); }
        // 2. background
        let bg = c.background();
        let mask = if bg != Bg::Off { self.mask(&img) } else { self.smooth.reset(); self.seg = None; false };
        if mask {
            match bg {
                Bg::Blur => { let b = fx::blurred(&img, c.blur); fx::composite(&mut img, &self.smooth.alpha, seg::W, seg::H, &Behind::Blur(b)); }
                Bg::Picture | Bg::Video => if self.background(&c.bg_file, img.w, img.h) {
                    let px = &self.clip.as_ref().map_or(&[][..], |c| &c.px[..]);
                    fx::composite(&mut img, &self.smooth.alpha, seg::W, seg::H, &Behind::Picture(px));
                },
                Bg::Off => {}
            }
        }
        if bg == Bg::Off || !matches!(bg, Bg::Picture | Bg::Video) { self.close_clip(); }
        // 3. soft focus, on the person only when there is a mask
        if c.soften > 0.0 { fx::soften(&mut img, c.soften, mask.then(|| (&self.smooth.alpha[..], seg::W, seg::H))); }
        // 4. mirror (before the name, which must stay readable)
        if c.mirror { fx::mirror(&mut img); }
        // 5. name
        if c.name.is_empty() { self.label = None; } else {
            if !self.label.as_ref().is_some_and(|l| l.fits(&c.name, img.h)) { self.label = text::Label::new(&c.name, img.h, img.w * 2 / 3); }
            if let Some(l) = &self.label { l.draw(&mut img); }
        }
    }

    /// Updates the person mask (`self.smooth.alpha`) from the segmentation thread; false when there is none (the model failed
    /// to load: logged once, the background effect stays off).
    fn mask(&mut self, img: &Img) -> bool {
        let w = self.seg.get_or_insert_with(seg::Worker::start);
        let mut m = w.step(img.px, img.w, img.h, img.stride);
        // the first frames: wait a little for the first mask; until then the whole picture counts as background, so the room
        // never shows while the model loads
        if m.is_none() && self.smooth.alpha.is_empty() { m = w.wait(Duration::from_millis(12)); }
        if let Some(e) = &w.failed {
            if !self.failed { self.host.log(&i18n::t("Segmentation: {e}").replace("{e}", e)); self.failed = true; }
            return false;
        }
        match m {
            Some(m) => { self.smooth.update(&m); }
            None if self.smooth.alpha.is_empty() => self.smooth.alpha = vec![0; seg::W * seg::H],
            None => {}
        }
        true
    }

    /// Brings the background picture (`self.clip.px`, `w` x `h`) up to date: opened through the host, the next video picture
    /// each frame. False when there is none.
    fn background(&mut self, path: &str, w: usize, h: usize) -> bool {
        if !self.clip.as_ref().is_some_and(|c| c.path == path && (c.w, c.h) == (w, h)) {
            self.close_clip();
            let r = self.host.call("media.open", &format!("{{\"path\":{},\"width\":{w},\"height\":{h}}}", json_str(path))).unwrap_or_default();
            let v = Values::parse(&r);
            let Some(id) = v.number("id") else {
                self.host.log(&i18n::t("Background: {e}").replace("{e}", &v.text("error").map_or_else(|| i18n::t("cannot be opened"), String::from)));
                // remembered as failed for this path and size: no retry every frame
                self.clip = Some(Clip { path: path.into(), w, h, id: 0, still: true, px: vec![], read: true });
                return false;
            };
            self.clip = Some(Clip { path: path.into(), w, h, id: id as u64, still: v.bool("still").unwrap_or(false), px: vec![0; w * h * 4], read: false });
        }
        let Some(c) = self.clip.as_mut() else { return false };
        if c.id == 0 { return false; }
        if !c.read || !c.still {
            let Some(r) = self.host.call("media.read", &format!("{{\"id\":{},\"data\":{},\"width\":{w},\"height\":{h},\"stride\":{}}}", c.id, c.px.as_mut_ptr() as usize, w * 4)) else { return false };
            if Values::parse(&r).text("error").is_some() { return c.read; }
            c.read = true;
        }
        true
    }

    fn close_clip(&mut self) {
        if let Some(c) = self.clip.take() { if c.id != 0 { self.host.call("media.close", &format!("{{\"id\":{}}}", c.id)); } }
    }
}

#[derive(Default)]
struct Video {
    host: Option<Host>,
    cfg: Arc<Mutex<Cfg>>,
    filter: Option<CameraFilter>,
    /// The file last given to `camera.source` ("" = the camera).
    source: String,
    warned: bool,
}

impl Video {
    /// Brings the host in line with the settings: the camera source, and the frame filter while an effect is on.
    fn sync(&mut self) {
        let Some(h) = self.host else { return };
        let c = lock(&self.cfg).clone();
        if !h.has("camera.filter") {
            if !self.warned && (c.any_effect() || !c.source.is_empty()) {
                h.toast("This Webox version does not give plugins camera pictures. Please update Webox.");
                self.warned = true;
            }
            return;
        }
        if c.source != self.source {
            let r = h.call("camera.source", &format!("{{\"path\":{}}}", json_str(&c.source))).unwrap_or_default();
            if let Some(e) = Values::parse(&r).text("error") { h.toast(&i18n::t("Video source: {e}").replace("{e}", e)); }
            self.source = c.source.clone();
        }
        if c.any_effect() && self.filter.is_none() {
            let mut pipe = Pipe::new(h, self.cfg.clone());
            self.filter = h.camera_filter(move |f| pipe.process(f));
        } else if !c.any_effect() {
            // dropping it unregisters the filter (Webox waits for a running frame) and frees the pipeline
            self.filter = None;
        }
    }
}

impl Plugin for Video {
    const NAME: &'static str = "Video";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    const EVENTS: &'static str = "i18n.strings";
    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> { (name == "i18n.strings").then(|| i18n::strings(payload)) }

    fn settings(&self) -> Option<Schema> {
        let mut files: Vec<&str> = VIDEO_EXT.to_vec();
        files.extend_from_slice(PICTURE_EXT);
        Some(Schema::new()
            .section("Video source", "What the others see instead of your camera. With a file the camera stays off; a video loops.")
            .choice("source", "Video source", "", "camera", &[("camera", "Camera"), ("video", "Video file"), ("picture", "Picture")])
            .file("source_file", "File", "Video file or picture for \"Video file\" / \"Picture\".", "Videos and pictures", &files)
            .section("Background", "Finds you in the picture (locally, without internet) and replaces everything behind you.")
            .choice("background", "Background", "", "off", &[("off", "Off"), ("blur", "Blur"), ("picture", "Picture"), ("video", "Video")])
            .file("background_file", "Background file", "Picture or video for \"Picture\" / \"Video\". A video loops.", "Pictures and videos", &files)
            .number("blur", "Blur strength", "", 5.0, 1.0, 8.0, 1.0)
            .section("Picture", "Applies to the picture the others see. The preview under Settings > Video shows it.")
            .toggle("mirror", "Mirror", "The others see you mirrored. Webox mirrors your own preview anyway.", false)
            .number("brightness", "Brightness", "0 = unchanged.", 0.0, -100.0, 100.0, 5.0)
            .number("contrast", "Contrast", "0 = unchanged.", 0.0, -100.0, 100.0, 5.0)
            .number("soften", "Soft focus for the face", "Smooths skin and flat areas, edges stay sharp. 0 = off.", 0.0, 0.0, 100.0, 5.0)
            .toggle("low_light", "Low-light boost", "Brightens a dark picture automatically.", false)
            .text("name", "Show name", "Shown bottom left. Empty = off.", ""))
    }

    fn settings_changed(&mut self, v: &Values) {
        *lock(&self.cfg) = Cfg::from(v);
        self.sync();
    }

    fn start(&mut self, host: Host) {
        i18n::sync(host);
        self.host = Some(host);
        self.source = String::new();
        self.sync();
    }

    fn stop(&mut self) {
        // Webox already removed the filter and the source before calling stop; this drops the pipeline and asks again for an
        // older host.
        self.filter = None;
        if let Some(h) = self.host.take() {
            if !self.source.is_empty() { h.call("camera.source", "{\"path\":\"\"}"); }
            h.call("media.close", "{}");
        }
        self.source = String::new();
    }
}

export_plugin!(Video);

#[cfg(test)]
mod i18n_tests {
    use super::*;
    #[test]
    fn every_setting_has_a_german_text() {
        let s = Video::default().settings().unwrap().to_json();
        assert_eq!(i18n::untranslated(&s, &[]), Vec::<String>::new());
    }
    #[test]
    fn the_event_answers_german_and_nothing_for_other_languages() {
        let mut p = Video::default();
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"fr","fallbacks":["fr_FR","fr"]}"#).as_deref(), Some("{\"strings\":{}}"));
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"en","fallbacks":["en_US","en"]}"#).as_deref(), Some("{\"strings\":{}}"));
        let de: serde_json::Value = serde_json::from_str(&i18n::answer("de_CH")).unwrap();
        assert_eq!(de["strings"]["Mirror"], "Spiegeln");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(json: &str) -> Values { Values::parse(json) }

    #[test]
    fn settings_map_to_the_config() {
        let d = Cfg::default();
        assert!(!d.any_effect() && d.source.is_empty() && d.bg == Bg::Off && d.blur == 5);
        let c = Cfg::from(&values(r#"{"source":"video","source_file":" C:/v.mp4 ","background":"picture","background_file":"","mirror":true,"soften":50}"#));
        assert_eq!(c.source, "C:/v.mp4");
        assert_eq!((c.bg, c.background()), (Bg::Picture, Bg::Off), "no file: no picture background");
        assert!(c.mirror && c.soften == 0.5 && c.any_effect());
        // a file chosen but the camera selected: the camera
        assert!(Cfg::from(&values(r#"{"source":"camera","source_file":"C:/v.mp4"}"#)).source.is_empty());
        assert!(Cfg::from(&values(r#"{"background":"blur"}"#)).any_effect());
        assert!(Cfg::from(&values(r#"{"name":"  "}"#)).name.is_empty());
    }

    /// The schema is what Webox reads (ids, the two file fields, German labels).
    #[test]
    fn schema() {
        let j = Video::default().settings().unwrap().to_json();
        for id in ["source", "source_file", "background", "background_file", "mirror", "brightness", "contrast", "soften", "low_light", "name"] {
            assert!(j.contains(&format!("\"id\":\"{id}\"")), "{id}");
        }
        assert!(j.contains("\"type\":\"file\"") && j.contains("Blur") && j.contains("Low-light boost"));
    }

    /// The whole pipeline on a synthetic 1280x720 frame with every cheap effect on (no host needed without a background).
    #[test]
    fn pipeline_runs_the_cheap_effects() {
        let cfg = Arc::new(Mutex::new(Cfg { mirror: true, brightness: 20.0, contrast: 10.0, soften: 0.5, low_light: true, name: "Anna".into(), ..Cfg::default() }));
        let mut p = Pipe::new(unsafe { std::mem::zeroed() }, cfg);
        let (w, h) = (1280, 720);
        let mut px: Vec<u8> = (0..w * h).flat_map(|i| { let x = (i % w) as u8; [x, 40, 40, 255] }).collect();
        let mut f = Frame { width: w, height: h, stride: w * 4, data: &mut px, seq: 1, time_us: 0 };
        p.process(&mut f);
        // mirrored: the right edge had x = 255 (1279 % 256), now on the left; brighter than before
        assert!(px[0] > px[(w - 1) * 4], "{} {}", px[0], px[(w - 1) * 4]);
        assert!(px[4 * w * 10 + 1] > 40, "brighter");
        assert!(p.label.is_some());
    }
}
