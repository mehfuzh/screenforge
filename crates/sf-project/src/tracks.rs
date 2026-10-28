//! ScreenForge extension: the media bin and clip tracks (issue #38, design 4.1).
//!
//! `sfMedia` lists the files a project uses besides its recording; they are copied into
//! `<package>/media/<sha8>-<name>.<ext>` and never edited in place. `sfTracks` holds tracks of
//! clips on the output timeline. This module covers the visual tracks: `text` tracks of titles
//! and `video` tracks with the `overlay` role, whose clips show an image or a video over the
//! recording. A clip with a `link` is pinned to a moment of the recording, so cutting earlier
//! footage moves it (see `sf_engine::tracks`). Both keys live in the scene's unknown-field map,
//! so Screen Studio ignores them.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ids::new_id;
use crate::schema::common::{Extra, Point, Rect};
use crate::v4::Scene;

pub const MEDIA_KEY: &str = "sfMedia";
pub const TRACKS_KEY: &str = "sfTracks";
/// Folder inside the package that holds imported media.
pub const MEDIA_DIR: &str = "media";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Image,
    Audio,
}

/// A file of the media bin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: String,
    /// Relative to the package (`media/…`).
    pub path: String,
    pub kind: MediaKind,
    /// Length of a video or audio file; `None` for an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default)]
    pub has_audio: bool,
    #[serde(default)]
    pub alpha: bool,
    /// `"import"` or `{ "ai": <provenance id> }`.
    #[serde(default)]
    pub origin: Value,
    #[serde(flatten)]
    pub extra: Extra,
}

impl MediaItem {
    /// The kind of a file by its extension, `None` when it is not a supported media file.
    pub fn kind_of(path: &Path) -> Option<MediaKind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" => Some(MediaKind::Image),
            "mp4" | "mov" | "m4v" | "webm" | "mkv" => Some(MediaKind::Video),
            "wav" | "mp3" | "m4a" | "aac" | "flac" | "ogg" => Some(MediaKind::Audio),
            _ => None,
        }
    }
}

/// Copy `file` into the package's media folder (content-addressed, so importing the same file
/// twice reuses it) and describe it. `duration_ms` comes from the caller's probe.
pub fn import_media(package: &Path, file: &Path, duration_ms: Option<f64>, has_audio: bool) -> std::io::Result<MediaItem> {
    let kind = MediaItem::kind_of(file).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("not an image, video or audio file: {}", file.display())))?;
    let bytes = std::fs::read(file)?;
    let hash = Sha256::digest(&bytes);
    let sha8: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("media");
    let rel = format!("{MEDIA_DIR}/{sha8}-{name}");
    let dst = package.join(&rel);
    if !dst.is_file() {
        std::fs::create_dir_all(package.join(MEDIA_DIR))?;
        std::fs::write(&dst, &bytes)?;
    }
    Ok(MediaItem { id: format!("m_{sha8}"), path: rel, kind, duration_ms, has_audio, alpha: false, origin: Value::String("import".into()), extra: Extra::new() })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Video,
    Audio,
    Captions,
    Text,
    /// A kind written by a newer version; kept as it is.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    /// Video tracks: `"camera"` | `"overlay"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Track {
    pub const OVERLAY: &'static str = "overlay";

    pub fn text() -> Self {
        Track { id: new_id(), kind: TrackKind::Text, role: None, hidden: false, clips: vec![], extra: Extra::new() }
    }

    pub fn overlay() -> Self {
        Track { id: new_id(), kind: TrackKind::Video, role: Some(Self::OVERLAY.into()), hidden: false, clips: vec![], extra: Extra::new() }
    }

    /// A track drawn over the recording: titles or overlay media.
    pub fn is_visual(&self) -> bool {
        self.kind == TrackKind::Text || (self.kind == TrackKind::Video && self.role.as_deref() == Some(Self::OVERLAY))
    }
}

/// A moment of the recording a clip is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub source_ms: f64,
}

/// How a clip enters or leaves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipAnim {
    /// `fade` | `blur` | `words` (titles: each word blurs in after the previous one) | `rise` |
    /// `zoom` | `slide-left` | `slide-right` | `slide-up` | `slide-down`.
    pub kind: String,
    #[serde(default = "anim_ms")]
    pub duration_ms: f64,
}

fn anim_ms() -> f64 {
    ClipAnim::DEFAULT_MS
}

impl ClipAnim {
    pub const DEFAULT_MS: f64 = 450.0;
    pub const KINDS: [&'static str; 9] = ["fade", "blur", "words", "rise", "zoom", "slide-left", "slide-right", "slide-up", "slide-down"];

    pub fn new(kind: &str) -> Self {
        ClipAnim { kind: kind.into(), duration_ms: Self::DEFAULT_MS }
    }
}

/// The words and look of a title. Words wrapped in `*` are drawn in `accentColor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
    /// One or more lines (`\n`).
    pub content: String,
    /// A smaller second paragraph under the title, in `subtitleColor`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subtitle: String,
    /// `sans-serif` | `heavy` | `serif` | `mono`, as for captions.
    #[serde(default = "sans")]
    pub font: String,
    /// Font size as a fraction of the video height.
    #[serde(default = "text_size")]
    pub size_ratio: f64,
    #[serde(default = "white")]
    pub color: String,
    #[serde(default = "accent")]
    pub accent_color: String,
    #[serde(default = "grey")]
    pub subtitle_color: String,
    /// `left` | `center` | `right`.
    #[serde(default = "left")]
    pub align: String,
    /// Anchor of the block as fractions of the video: x is its left edge, centre or right edge
    /// (by `align`), y its vertical centre.
    #[serde(default = "text_pos")]
    pub position01: Point,
    /// A soft shadow under the letters.
    #[serde(default)]
    pub shadow: bool,
}

fn sans() -> String {
    "sans-serif".into()
}
fn text_size() -> f64 {
    TextStyle::DEFAULT_SIZE
}
fn white() -> String {
    "#ffffff".into()
}
fn accent() -> String {
    TextStyle::DEFAULT_ACCENT.into()
}
fn grey() -> String {
    "#a1a1aa".into()
}
fn left() -> String {
    "left".into()
}
fn text_pos() -> Point {
    Point::new(0.06, 0.8)
}

impl TextStyle {
    pub const DEFAULT_SIZE: f64 = 0.06;
    pub const DEFAULT_ACCENT: &'static str = "#8ea2ff";

    pub fn new(content: &str) -> Self {
        TextStyle {
            content: content.into(),
            subtitle: String::new(),
            font: sans(),
            size_ratio: Self::DEFAULT_SIZE,
            color: white(),
            accent_color: accent(),
            subtitle_color: grey(),
            align: left(),
            position01: text_pos(),
            shadow: false,
        }
    }
}

/// A clip on a track. Media clips play `[sourceStartMs, sourceEndMs)` of their media at `speed`;
/// titles and images have no media time, their length is `sourceEndMs - sourceStartMs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    /// Start on the output timeline. For a linked clip this is refreshed from its link.
    /// Defaulted, like every field here, so clips of tracks this version does not draw (a
    /// caption clip is just `{id, follows}`) still read and are written back unchanged.
    #[serde(default)]
    pub output_start_ms: f64,
    #[serde(default)]
    pub source_start_ms: f64,
    #[serde(default)]
    pub source_end_ms: f64,
    #[serde(default = "one")]
    pub speed: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Pinned to this moment of the recording; hidden while that moment is cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,
    /// Overlays: `full` | `center` | `pip-br` | `pip-bl` | `pip-tr` | `pip-tl` | `custom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    /// The `custom` placement, as fractions of the video (the media keeps its aspect inside).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect01: Option<Rect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim_in: Option<ClipAnim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim_out: Option<ClipAnim>,
    /// Overlays: corner radius as a fraction of the overlay's shorter side.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub corner_ratio: f64,
    /// Overlays: drop shadow opacity (0..1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shadow: f64,
    /// Blur the recording behind while the clip shows (0..1), like a camera's shallow focus.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub backdrop_blur: f64,
    /// Darken the recording behind while the clip shows (0..1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub backdrop_dim: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

fn one() -> f64 {
    1.0
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Clip {
    pub const MIN_DURATION_MS: f64 = 200.0;
    pub const TITLE_MS: f64 = 3000.0;
    pub const PLACEMENTS: [&'static str; 7] = ["full", "center", "pip-br", "pip-bl", "pip-tr", "pip-tl", "custom"];

    fn bare(output_start_ms: f64, duration_ms: f64) -> Self {
        Clip {
            id: new_id(),
            media: None,
            output_start_ms,
            source_start_ms: 0.0,
            source_end_ms: duration_ms.max(Self::MIN_DURATION_MS),
            speed: 1.0,
            volume: None,
            link: None,
            placement: None,
            rect01: None,
            text: None,
            anim_in: None,
            anim_out: None,
            corner_ratio: 0.0,
            shadow: 0.0,
            backdrop_blur: 0.0,
            backdrop_dim: 0.0,
            extra: Extra::new(),
        }
    }

    /// A title shown for `duration_ms`, its words blurring in one by one and the whole title
    /// blurring out.
    pub fn title(output_start_ms: f64, duration_ms: f64, content: &str) -> Self {
        Clip { text: Some(TextStyle::new(content)), anim_in: Some(ClipAnim::new("words")), anim_out: Some(ClipAnim::new("blur")), ..Self::bare(output_start_ms, duration_ms) }
    }

    /// An image or video of the bin, centred, fading in and out. A video plays from its start
    /// for `duration_ms` (or to its end).
    pub fn overlay(media: &MediaItem, output_start_ms: f64, duration_ms: f64) -> Self {
        let mut c = Self::bare(output_start_ms, duration_ms);
        if let Some(d) = media.duration_ms.filter(|_| media.kind == MediaKind::Video) {
            c.source_end_ms = duration_ms.min(d).max(Self::MIN_DURATION_MS);
        }
        c.media = Some(media.id.clone());
        c.placement = Some("center".into());
        c.volume = Some(0.0);
        c.corner_ratio = 0.03;
        c.shadow = 0.5;
        c.anim_in = Some(ClipAnim::new("fade"));
        c.anim_out = Some(ClipAnim::new("fade"));
        c
    }

    pub fn source_duration_ms(&self) -> f64 {
        (self.source_end_ms - self.source_start_ms).max(0.0)
    }

    pub fn output_duration_ms(&self) -> f64 {
        self.source_duration_ms() / self.speed.max(0.01)
    }

    pub fn output_end_ms(&self) -> f64 {
        self.output_start_ms + self.output_duration_ms()
    }

    /// Media time shown at output time `t`.
    pub fn output_to_source(&self, t: f64) -> f64 {
        self.source_start_ms + (t - self.output_start_ms) * self.speed
    }

    /// Change the length on the output timeline, keeping the start.
    pub fn set_output_duration(&mut self, ms: f64) {
        self.source_end_ms = self.source_start_ms + ms.max(Self::MIN_DURATION_MS) * self.speed.max(0.01);
    }
}

pub trait SceneTracksExt {
    fn media(&self) -> Vec<MediaItem>;
    fn set_media(&mut self, media: Vec<MediaItem>);
    fn tracks(&self) -> Vec<Track>;
    fn set_tracks(&mut self, tracks: Vec<Track>);
    /// Add `item` to the bin unless an item with its id is there already.
    fn add_media(&mut self, item: MediaItem) {
        let mut m = self.media();
        if !m.iter().any(|x| x.id == item.id) {
            m.push(item);
            self.set_media(m);
        }
    }
}

impl SceneTracksExt for Scene {
    fn media(&self) -> Vec<MediaItem> {
        self.extra.get(MEDIA_KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    fn set_media(&mut self, media: Vec<MediaItem>) {
        if media.is_empty() {
            self.extra.remove(MEDIA_KEY);
        } else {
            self.extra.insert(MEDIA_KEY.into(), serde_json::to_value(media).unwrap_or(Value::Null));
        }
    }

    fn tracks(&self) -> Vec<Track> {
        self.extra.get(TRACKS_KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    fn set_tracks(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            self.extra.remove(TRACKS_KEY);
        } else {
            self.extra.insert(TRACKS_KEY.into(), serde_json::to_value(tracks).unwrap_or(Value::Null));
        }
    }
}

/// Put `clip` on the first track of its sort (text for a title, overlay for media) that is
/// free over its time, or on a new such track. Returns the track's index.
pub fn add_clip(tracks: &mut Vec<Track>, clip: Clip) -> usize {
    let text = clip.text.is_some();
    let (s, e) = (clip.output_start_ms, clip.output_end_ms());
    let fits = |t: &Track| (if text { t.kind == TrackKind::Text } else { t.is_visual() && t.kind == TrackKind::Video }) && t.clips.iter().all(|c| e <= c.output_start_ms || s >= c.output_end_ms());
    let i = match tracks.iter().position(fits) {
        Some(i) => i,
        None => {
            tracks.push(if text { Track::text() } else { Track::overlay() });
            tracks.len() - 1
        }
    };
    tracks[i].clips.push(clip);
    tracks[i].clips.sort_by(|a, b| a.output_start_ms.total_cmp(&b.output_start_ms));
    i
}

/// The clip with `id` on any track, mutably.
pub fn find_clip_mut<'a>(tracks: &'a mut [Track], id: &str) -> Option<&'a mut Clip> {
    tracks.iter_mut().flat_map(|t| t.clips.iter_mut()).find(|c| c.id == id)
}

/// Remove the clip `id`; drops tracks left empty. True when it was found.
pub fn remove_clip(tracks: &mut Vec<Track>, id: &str) -> bool {
    let before: usize = tracks.iter().map(|t| t.clips.len()).sum();
    for t in tracks.iter_mut() {
        t.clips.retain(|c| c.id != id);
    }
    let found = tracks.iter().map(|t| t.clips.len()).sum::<usize>() < before;
    tracks.retain(|t| !t.clips.is_empty() || !t.is_visual());
    found
}

/// Split the clip `id` at output time `t` (strictly inside it); the right part gets a new id
/// and, when linked, a link moved by the same amount. Returns the new clip's id.
pub fn split_clip(tracks: &mut [Track], id: &str, t: f64) -> Option<String> {
    for tr in tracks.iter_mut() {
        let Some(i) = tr.clips.iter().position(|c| c.id == id) else { continue };
        let c = &tr.clips[i];
        if t <= c.output_start_ms + Clip::MIN_DURATION_MS || t >= c.output_end_ms() - Clip::MIN_DURATION_MS {
            return None;
        }
        let mut right = c.clone();
        let src = c.output_to_source(t);
        let shift = t - c.output_start_ms;
        right.id = new_id();
        right.output_start_ms = t;
        right.source_start_ms = src;
        right.link = c.link.map(|l| Link { source_ms: l.source_ms + shift });
        // the cut ends one part and starts the other without an animation
        right.anim_in = None;
        tr.clips[i].source_end_ms = src;
        tr.clips[i].anim_out = None;
        let rid = right.id.clone();
        tr.clips.insert(i + 1, right);
        return Some(rid);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_round_trip_through_the_scene_and_keep_unknown_fields() {
        let mut scene = Scene::recording("s", 0);
        let mut tracks = vec![];
        let mut title = Clip::title(1000.0, 2000.0, "The *probability* of success.");
        title.link = Some(Link { source_ms: 4000.0 });
        add_clip(&mut tracks, title);
        tracks[0].clips[0].extra.insert("lineId".into(), Value::String("l_01".into()));
        scene.set_tracks(tracks.clone());
        let json = serde_json::to_value(&scene).unwrap();
        let back: Scene = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.tracks(), tracks);
        let clip = &json["sfTracks"][0]["clips"][0];
        assert_eq!(clip["link"]["sourceMs"], 4000.0);
        assert_eq!(clip["animIn"]["kind"], "words");
        assert_eq!(clip["lineId"], "l_01");
        assert!(clip.get("backdropBlur").is_none(), "zero extras are not written");
        scene.set_tracks(vec![]);
        assert!(scene.extra.get(TRACKS_KEY).is_none());
    }

    #[test]
    fn tracks_this_version_does_not_draw_survive_an_edit() {
        // #38's caption track: clips only follow audio clips, and a kind from the future
        let json = serde_json::json!([
            { "id": "t_cap", "kind": "captions", "style": "default", "clips": [{ "id": "c_3", "follows": "c_2" }] },
            { "id": "t_new", "kind": "sticker", "clips": [] },
        ]);
        let mut scene = Scene::recording("s", 0);
        scene.extra.insert(TRACKS_KEY.into(), json);
        let mut tracks = scene.tracks();
        assert_eq!(tracks.len(), 2, "nothing is dropped");
        add_clip(&mut tracks, Clip::title(0.0, 1000.0, "x"));
        scene.set_tracks(tracks);
        let out = serde_json::to_value(&scene).unwrap();
        assert_eq!(out["sfTracks"][0]["clips"][0]["follows"], "c_2");
        assert_eq!(out["sfTracks"][0]["style"], "default");
        assert_eq!(out["sfTracks"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn clips_go_to_a_free_track_of_their_sort() {
        let mut tracks = vec![];
        assert_eq!(add_clip(&mut tracks, Clip::title(0.0, 2000.0, "a")), 0);
        assert_eq!(add_clip(&mut tracks, Clip::title(2000.0, 1000.0, "b")), 0, "after the first");
        assert_eq!(add_clip(&mut tracks, Clip::title(500.0, 1000.0, "c")), 1, "overlaps: a new lane");
        let img = MediaItem { id: "m".into(), path: "media/x.png".into(), kind: MediaKind::Image, duration_ms: None, has_audio: false, alpha: false, origin: Value::Null, extra: Extra::new() };
        assert_eq!(add_clip(&mut tracks, Clip::overlay(&img, 0.0, 3000.0)), 2);
        assert_eq!(tracks[2].role.as_deref(), Some("overlay"));
        let id = tracks[1].clips[0].id.clone();
        assert!(remove_clip(&mut tracks, &id));
        assert_eq!(tracks.len(), 2, "the emptied title lane is dropped");
    }

    #[test]
    fn split_keeps_media_time_and_moves_the_link() {
        let mut tracks = vec![Track::overlay()];
        let mut c = Clip::title(1000.0, 3000.0, "x");
        c.link = Some(Link { source_ms: 5000.0 });
        let id = c.id.clone();
        tracks[0].clips.push(c);
        let rid = split_clip(&mut tracks, &id, 2000.0).unwrap();
        let (l, r) = (&tracks[0].clips[0], &tracks[0].clips[1]);
        assert_eq!((l.output_end_ms(), r.output_start_ms, r.output_end_ms()), (2000.0, 2000.0, 4000.0));
        assert_eq!(r.link.unwrap().source_ms, 6000.0);
        assert!(l.anim_out.is_none() && r.anim_in.is_none());
        assert_eq!(r.id, rid);
        assert!(split_clip(&mut tracks, &id, 1050.0).is_none(), "too short a piece");
    }

    #[test]
    fn import_copies_once_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("card.png");
        std::fs::write(&src, b"not really a png").unwrap();
        let pkg = dir.path().join("p.screenforge");
        std::fs::create_dir_all(&pkg).unwrap();
        let a = import_media(&pkg, &src, None, false).unwrap();
        let b = import_media(&pkg, &src, None, false).unwrap();
        assert_eq!(a, b);
        assert!(a.path.starts_with("media/") && a.path.ends_with("-card.png"));
        assert!(pkg.join(&a.path).is_file());
        assert!(import_media(&pkg, &dir.path().join("x.txt"), None, false).is_err());
    }
}
