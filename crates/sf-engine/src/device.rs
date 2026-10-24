//! Device frames ("mockups") around the screen: a phone or tablet body with its metal rim,
//! glass bezel, camera cutout and side buttons, as plain rounded rectangles. Drawn shapes rather
//! than device images: they stay sharp at any zoom and need no artwork.
//!
//! Each device keeps its real proportions (a phone 9:19.5, a tablet about 7:10), standing up
//! unless the recording is clearly wide, and is fitted into the rect the screen would have had,
//! so padding, zooms and layouts work as without a frame. The recording is fitted into the
//! device's display at its own aspect ratio: a phone recording fills it, anything else plays in
//! the middle with black glass around it, like a video on the device. Nothing is cropped or
//! stretched.

use sf_project::schema::common::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Phone with a camera island at the top of the screen.
    Phone,
    /// Phone with a round camera hole in the screen.
    PunchHole,
    Tablet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    Black,
    Silver,
    Natural,
    White,
}

/// `(kind, key part, label)` in picker order.
pub const KINDS: [(Kind, &str, &str); 3] = [(Kind::Phone, "phone", "Phone"), (Kind::PunchHole, "punch-hole", "Punch-hole phone"), (Kind::Tablet, "tablet", "Tablet")];
pub const FINISHES: [(Finish, &str, &str); 4] = [(Finish::Black, "black", "Black"), (Finish::Silver, "silver", "Silver"), (Finish::Natural, "natural", "Natural"), (Finish::White, "white", "White")];

/// A device frame, saved as `config.device.frameKey` (`"phone-black"`, `"tablet-silver"` …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub finish: Finish,
}

impl Frame {
    /// The frame for `key`; `None` for keys this app does not draw (no frame then), such as
    /// the device images named by Screen Studio projects.
    pub fn parse(key: &str) -> Option<Frame> {
        let (kind, finish) = key.trim().rsplit_once('-')?;
        let kind = KINDS.iter().find(|k| k.1 == kind)?.0;
        let finish = FINISHES.iter().find(|f| f.1 == finish)?.0;
        Some(Frame { kind, finish })
    }

    pub fn key(&self) -> String {
        let kind = KINDS.iter().find(|k| k.0 == self.kind).map_or("phone", |k| k.1);
        let finish = FINISHES.iter().find(|f| f.0 == self.finish).map_or("black", |f| f.1);
        format!("{kind}-{finish}")
    }

    /// `(frame, rim highlight, buttons)` colours.
    fn colors(&self) -> (&'static str, &'static str, &'static str) {
        match self.finish {
            Finish::Black => ("#2a2a2d", "#55555b", "#232326"),
            Finish::Silver => ("#c4c6ca", "#eceef0", "#aeb0b5"),
            Finish::Natural => ("#b3a897", "#d9d1c4", "#9e9483"),
            Finish::White => ("#e6e4e0", "#fbfaf8", "#d3d0ca"),
        }
    }
}

/// A rounded rectangle of one colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub rect: Rect,
    pub radius: f64,
    pub color: &'static str,
}

/// A frame laid out around a screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Parts {
    /// The whole device, for its shadow.
    pub body: Rect,
    pub body_radius: f64,
    /// The device's display (black where the recording does not cover it).
    pub display: Rect,
    /// Where the recording goes: its own aspect ratio, fitted into the display.
    pub screen: Rect,
    pub screen_radius: f64,
    /// Drawn before the screen, back to front: buttons, rim, frame, glass.
    pub under: Vec<Shape>,
    /// Drawn over the screen: the camera island or hole.
    pub over: Vec<Shape>,
}

const GLASS: &str = "#050506";
const CAMERA: &str = "#000000";
/// The lens dot on a tablet's bezel.
const LENS: &str = "#1c1c20";

fn inset(r: Rect, by: f64) -> Rect {
    Rect { x: r.x + by, y: r.y + by, width: (r.width - 2.0 * by).max(0.0), height: (r.height - 2.0 * by).max(0.0) }
}

/// The largest rect of `aspect` (w / h) centred in `r`.
fn fit(aspect: f64, r: Rect) -> Rect {
    let (w, h) = if r.width / r.height.max(1e-9) > aspect { (r.height * aspect, r.height) } else { (r.width, r.width / aspect.max(1e-9)) };
    Rect { x: r.x + (r.width - w) / 2.0, y: r.y + (r.height - h) / 2.0, width: w, height: h }
}

/// Width / height of the device's display standing up.
fn proportions(kind: Kind) -> f64 {
    match kind {
        Kind::Phone | Kind::PunchHole => 9.0 / 19.5,
        Kind::Tablet => 0.7,
    }
}

/// Metal rim, glass bezel and corner radius, as fractions of the body's short side.
fn sizes(kind: Kind) -> (f64, f64, f64) {
    match kind {
        Kind::Phone | Kind::PunchHole => (0.014, 0.032, 0.17),
        Kind::Tablet => (0.011, 0.05, 0.065),
    }
}

/// Width / height of the body standing up: the display plus the same border all round.
fn body_proportions(kind: Kind) -> f64 {
    let (rim, bezel, _) = sizes(kind);
    let border = rim + bezel;
    1.0 / ((1.0 - 2.0 * border) / proportions(kind) + 2.0 * border)
}

/// A recording this much wider than tall turns the device on its side.
const LANDSCAPE_FROM: f64 = 1.15;

/// Lay out `frame` in `area` (output px; the rect the screen alone would have, which has the
/// recording's aspect ratio). On its side the device's top is to the left.
pub fn layout(frame: Frame, area: Rect) -> Parts {
    let aspect = area.width / area.height.max(1e-9);
    let portrait = aspect < LANDSCAPE_FROM;
    let device = if portrait { body_proportions(frame.kind) } else { 1.0 / body_proportions(frame.kind) };
    let outer = fit(device, area);
    let short = outer.width.min(outer.height);
    let (rim, bezel, radius) = sizes(frame.kind);
    let (rim, bezel, body_radius) = (rim * short, bezel * short, radius * short);
    let (frame_c, rim_c, button_c) = frame.colors();
    let glass = inset(outer, rim);
    let display = inset(glass, bezel);
    let display_radius = (body_radius - rim - bezel).max(0.02 * short);
    let recording = fit(aspect, display);
    // a recording that fills the display takes its rounded corners; one in the middle of it
    // (black above and below, or at the sides) only needs a slight rounding
    let fills = recording.width >= display.width * 0.95 && recording.height >= display.height * 0.95;
    let screen_radius = if fills { display_radius } else { 0.012 * short };

    let mut under = vec![];
    // side buttons stick out of the body, drawn first so the body covers their inner half
    let depth = 0.009 * short;
    let long = if portrait { outer.height } else { outer.width };
    let button = |left_side: bool, from: f64, to: f64| -> Shape {
        let (a, b) = (from * long, (to - from) * long);
        let rect = match (portrait, left_side) {
            (true, true) => Rect { x: outer.x - depth, y: outer.y + a, width: 2.0 * depth, height: b },
            (true, false) => Rect { x: outer.x + outer.width - depth, y: outer.y + a, width: 2.0 * depth, height: b },
            // on its side (top to the left): the left side faces down, the right side up
            (false, true) => Rect { x: outer.x + a, y: outer.y + outer.height - depth, width: b, height: 2.0 * depth },
            (false, false) => Rect { x: outer.x + a, y: outer.y - depth, width: b, height: 2.0 * depth },
        };
        Shape { rect, radius: depth, color: button_c }
    };
    match frame.kind {
        Kind::Phone | Kind::PunchHole => {
            under.push(button(true, 0.15, 0.19));
            under.push(button(true, 0.23, 0.30));
            under.push(button(true, 0.32, 0.39));
            under.push(button(false, 0.26, 0.36));
        }
        Kind::Tablet => {
            under.push(button(false, 0.06, 0.12));
        }
    }
    under.push(Shape { rect: outer, radius: body_radius, color: rim_c });
    under.push(Shape { rect: inset(outer, rim * 0.35), radius: body_radius - rim * 0.35, color: frame_c });
    under.push(Shape { rect: glass, radius: body_radius - rim, color: GLASS });

    let mut over = vec![];
    // the camera sits on the display, wherever the recording is in it
    let (screen, sw, sh) = (display, display.width, display.height);
    let across = if portrait { sw } else { sh };
    match frame.kind {
        Kind::Phone => {
            // island centred at the top edge (the left edge on its side)
            let (len, thick, gap) = (0.3 * across, 0.088 * across, 0.03 * across);
            let rect = if portrait {
                Rect { x: screen.x + (sw - len) / 2.0, y: screen.y + gap, width: len, height: thick }
            } else {
                Rect { x: screen.x + gap, y: screen.y + (sh - len) / 2.0, width: thick, height: len }
            };
            over.push(Shape { rect, radius: thick / 2.0, color: CAMERA });
        }
        Kind::PunchHole => {
            let (d, gap) = (0.034 * across, 0.035 * across);
            let rect = if portrait { Rect { x: screen.x + (sw - d) / 2.0, y: screen.y + gap, width: d, height: d } } else { Rect { x: screen.x + gap, y: screen.y + (sh - d) / 2.0, width: d, height: d } };
            over.push(Shape { rect, radius: d / 2.0, color: CAMERA });
        }
        Kind::Tablet => {
            // the lens sits in the bezel, centred above the screen
            let d = 0.012 * short;
            let rect = if portrait { Rect { x: screen.x + (sw - d) / 2.0, y: (glass.y + screen.y - d) / 2.0, width: d, height: d } } else { Rect { x: (glass.x + screen.x - d) / 2.0, y: screen.y + (sh - d) / 2.0, width: d, height: d } };
            under.push(Shape { rect, radius: d / 2.0, color: LENS });
        }
    }
    Parts { body: outer, body_radius, display, screen: recording, screen_radius, under, over }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, width: w, height: h }
    }

    fn inside(a: &Rect, b: &Rect) -> bool {
        a.x >= b.x - 1e-6 && a.y >= b.y - 1e-6 && a.x + a.width <= b.x + b.width + 1e-6 && a.y + a.height <= b.y + b.height + 1e-6
    }

    #[test]
    fn keys_round_trip_and_unknown_keys_draw_nothing() {
        for (k, _, _) in KINDS {
            for (f, _, _) in FINISHES {
                let frame = Frame { kind: k, finish: f };
                assert_eq!(Frame::parse(&frame.key()), Some(frame));
            }
        }
        assert_eq!(Frame::parse("punch-hole-natural"), Some(Frame { kind: Kind::PunchHole, finish: Finish::Natural }));
        // Screen Studio's device image names and nonsense: no frame
        assert_eq!(Frame::parse("iPad Pro 11 M4 Silver"), None);
        assert_eq!(Frame::parse("phone"), None);
        assert_eq!(Frame::parse("phone-gold"), None);
    }

    #[test]
    fn devices_keep_their_proportions_and_the_recording_its_own() {
        for kind in [Kind::Phone, Kind::PunchHole, Kind::Tablet] {
            // a phone recording, a square one, a wide desktop one
            for area in [r(100.0, 50.0, 390.0, 844.0), r(0.0, 0.0, 900.0, 880.0), r(0.0, 0.0, 1600.0, 900.0)] {
                let p = layout(Frame { kind, finish: Finish::Black }, area);
                let wide = area.width / area.height >= LANDSCAPE_FROM;
                let want = if wide { 1.0 / proportions(kind) } else { proportions(kind) };
                assert!((p.display.width / p.display.height - want).abs() < 1e-9, "{kind:?}: the display has the device's real proportions");
                assert!(inside(&p.body, &area), "{kind:?}: the device fits where the screen was");
                assert!((p.screen.width / p.screen.height - area.width / area.height).abs() < 1e-9, "{kind:?}: the recording is not stretched");
                assert!(inside(&p.screen, &p.display) && inside(&p.display, &inset(p.body, 1.0)));
                assert!(p.screen_radius < p.body_radius);
                for s in &p.over {
                    assert!(inside(&s.rect, &p.display), "{kind:?}: cutouts sit on the display");
                }
            }
        }
        // a phone recording fills the phone's display and takes its rounded corners
        let p = layout(Frame { kind: Kind::Phone, finish: Finish::Black }, r(0.0, 0.0, 390.0, 845.0));
        assert!(p.screen.height >= p.display.height * 0.99 && p.screen_radius > 0.1 * p.body.width);
        // a square recording stands in a tall phone, in the middle of its display
        let p = layout(Frame { kind: Kind::Phone, finish: Finish::Black }, r(0.0, 0.0, 900.0, 880.0));
        assert!(p.body.height > 2.0 * p.body.width && p.screen.height < p.display.height * 0.6);
    }

    #[test]
    fn the_island_follows_the_device_on_its_side() {
        let up = layout(Frame { kind: Kind::Phone, finish: Finish::Silver }, r(0.0, 0.0, 400.0, 800.0));
        let island = &up.over[0].rect;
        assert!(island.width > island.height && (island.x + island.width / 2.0 - 200.0).abs() < 1e-6 && island.y < 100.0);
        let side = layout(Frame { kind: Kind::Phone, finish: Finish::Silver }, r(0.0, 0.0, 1600.0, 800.0));
        let island = &side.over[0].rect;
        assert!(island.height > island.width && (island.y + island.height / 2.0 - 400.0).abs() < 1e-6 && island.x < side.body.x + 100.0);
    }

    #[test]
    fn buttons_stick_out_of_the_sides() {
        let p = layout(Frame { kind: Kind::Phone, finish: Finish::Black }, r(0.0, 0.0, 400.0, 800.0));
        let b = p.body;
        let buttons: Vec<&Shape> = p.under.iter().filter(|s| s.rect.width < 20.0).collect();
        assert_eq!(buttons.len(), 4);
        assert!(buttons.iter().all(|s| s.rect.x < b.x || s.rect.x + s.rect.width > b.x + b.width));
    }
}
