//! Built-in cursor sets, drawn as vectors and rasterised on demand.
//!
//! The recording ships the pointer images the OS showed (`cursors/<id>.png`), and the default
//! set (`macos-tahoe`, "Regular") uses those as they are. Every other set is drawn here so the
//! choice works without any asset folder: an arrow in a few looks, a pixel arrow, a dot and a
//! fingertip. A set covers the common pointer types (arrow, hand, text, crosshair, grab); for
//! the rest the caller falls back to the recorded sprite.
//!
//! Sprites are drawn at 2× into an RGBA buffer with a 4×4 supersample, like the recorder's
//! own drawn arrow, so edges stay smooth when the editor scales them.

/// `(id, label)` of every selectable set, in picker order. `RECORDED` is first.
pub const SETS: &[(&str, &str)] = &[
    (RECORDED, "Regular"),
    ("macos", "Classic"),
    ("light", "Light"),
    ("pixel", "Pixel"),
    ("dot", "Dot"),
    ("touch", "Hand"),
];

/// The set that draws the sprites captured during the recording.
pub const RECORDED: &str = "macos-tahoe";

/// A rasterised sprite: RGBA pixels at `scale`× plus the logical geometry in points.
#[derive(Debug, Clone)]
pub struct SetSprite {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Hotspot in points.
    pub hotspot: [f64; 2],
    /// Logical size in points (the pixel size is `scale`× that).
    pub size: [f64; 2],
}

/// The sprite a set draws for a recorded cursor id, or `None` when the set has no drawing for
/// it (the recorded sprite is used) or the set is `RECORDED`.
pub fn draw(set: &str, cursor_id: &str) -> Option<SetSprite> {
    let kind = Kind::of(cursor_id);
    match set {
        "macos" => arrow_set(kind, BLACK, WHITE),
        "light" => arrow_set(kind, WHITE, BLACK),
        "pixel" => pixel_set(kind),
        "dot" => Some(dot()),
        "touch" => Some(fingertip()),
        _ => None,
    }
}

/// True when a set draws something for every cursor id (no recorded fallback needed).
pub fn covers_all(set: &str) -> bool {
    matches!(set, "dot" | "touch")
}

const BLACK: Rgba = [20, 20, 20, 255];
const WHITE: Rgba = [255, 255, 255, 255];
const SCALE: f64 = 2.0;

type Rgba = [u8; 4];

/// What a recorded cursor id looks like, for the sets that draw their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Arrow,
    PointingHand,
    IBeam,
    Crosshair,
    OpenHand,
    ClosedHand,
    Other,
}

impl Kind {
    fn of(id: &str) -> Kind {
        match id {
            "arrow" | "contextualMenu" | "dragCopy" | "dragLink" | "disappearingItem" | "operationNotAllowed" | "busyButClickable" => Kind::Arrow,
            "pointingHand" => Kind::PointingHand,
            "iBeam" | "iBeamCursorForVerticalLayout" => Kind::IBeam,
            "crosshair" => Kind::Crosshair,
            "openHand" => Kind::OpenHand,
            "closedHand" => Kind::ClosedHand,
            _ => Kind::Other,
        }
    }
}

// ───────────── vector sets ─────────────

/// A filled polygon with an outline, in points.
struct Poly {
    pts: Vec<(f64, f64)>,
    fill: Rgba,
    outline: Rgba,
    outline_w: f64,
}

fn poly(pts: &[(f64, f64)], fill: Rgba, outline: Rgba, outline_w: f64) -> Poly {
    Poly { pts: pts.to_vec(), fill, outline, outline_w }
}

/// An axis-aligned rectangle as a polygon.
fn rect(x: f64, y: f64, w: f64, h: f64, fill: Rgba, outline: Rgba, outline_w: f64) -> Poly {
    poly(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], fill, outline, outline_w)
}

/// The classic arrow and its companions (hand, I-beam, crosshair, grab) in two colours.
fn arrow_set(kind: Kind, fill: Rgba, outline: Rgba) -> Option<SetSprite> {
    let ow = 1.1;
    match kind {
        Kind::Arrow => {
            // the macOS arrow outline (tip at 2,2), 24×32 pt like the recorder's drawn arrow
            let pts = [(2.0, 2.0), (2.0, 21.5), (6.8, 17.3), (10.1, 25.0), (13.3, 23.6), (10.0, 16.1), (16.5, 16.1)];
            Some(rasterise(24.0, 32.0, [2.0, 2.0], &[poly(&pts, fill, outline, ow)]))
        }
        Kind::PointingHand => {
            // index finger up, three folded fingers and the palm; hotspot at the fingertip
            let hand = [
                (9.0, 2.0), (12.0, 2.0), (12.6, 3.0), (12.6, 12.5), (17.0, 13.0), (21.0, 13.8), (24.0, 15.0), (25.0, 17.0),
                (24.6, 22.5), (22.5, 27.0), (20.0, 29.0), (11.5, 29.0), (9.0, 27.5), (4.0, 20.5), (3.6, 18.0), (5.0, 16.5),
                (7.0, 16.8), (8.6, 18.5), (8.6, 3.0),
            ];
            let seams = [
                rect(12.4, 12.5, 1.0, 5.5, outline, outline, 0.0),
                rect(16.4, 13.3, 1.0, 5.0, outline, outline, 0.0),
                rect(20.4, 14.2, 1.0, 4.5, outline, outline, 0.0),
            ];
            let mut shapes = vec![poly(&hand, fill, outline, ow)];
            shapes.extend(seams);
            Some(rasterise(28.0, 32.0, [10.8, 2.0], &shapes))
        }
        Kind::IBeam => {
            let stem = rect(10.0, 4.0, 2.4, 16.0, fill, outline, ow);
            let top_l = rect(5.5, 2.0, 5.0, 2.4, fill, outline, ow);
            let top_r = rect(11.9, 2.0, 5.0, 2.4, fill, outline, ow);
            let bot_l = rect(5.5, 19.6, 5.0, 2.4, fill, outline, ow);
            let bot_r = rect(11.9, 19.6, 5.0, 2.4, fill, outline, ow);
            Some(rasterise(22.0, 24.0, [11.2, 12.0], &[stem, top_l, top_r, bot_l, bot_r]))
        }
        Kind::Crosshair => {
            let v = rect(11.0, 2.0, 2.0, 20.0, fill, outline, ow);
            let h = rect(2.0, 11.0, 20.0, 2.0, fill, outline, ow);
            Some(rasterise(24.0, 24.0, [12.0, 12.0], &[v, h]))
        }
        Kind::OpenHand => {
            let hand = [
                (6.0, 12.0), (7.5, 9.5), (9.5, 10.5), (10.0, 6.0), (12.5, 5.0), (13.5, 9.5), (15.0, 3.5), (17.5, 3.5), (18.0, 9.5),
                (20.5, 5.0), (23.0, 6.0), (22.5, 13.0), (26.0, 11.0), (28.0, 13.0), (25.0, 22.0), (22.0, 27.0), (12.0, 27.5), (7.0, 20.0),
            ];
            Some(rasterise(32.0, 32.0, [16.0, 16.0], &[poly(&hand, fill, outline, ow)]))
        }
        Kind::ClosedHand => {
            // a fist: thumb on the left, four knuckles along the top
            let fist = [
                (6.0, 15.5), (7.0, 12.5), (9.5, 11.5), (11.0, 13.0), (11.5, 10.5), (13.5, 9.0), (15.5, 10.5), (16.5, 9.0), (18.5, 8.5),
                (20.0, 10.5), (21.0, 9.0), (23.5, 9.5), (24.5, 12.0), (26.0, 11.5), (27.0, 14.0), (26.5, 20.0), (24.0, 25.0), (19.0, 27.0),
                (12.0, 27.0), (8.0, 24.0), (6.5, 19.0),
            ];
            Some(rasterise(32.0, 32.0, [16.0, 16.0], &[poly(&fist, fill, outline, ow)]))
        }
        Kind::Other => None,
    }
}

/// A soft disc: a presenter's pointer that shows where the mouse is, not what it does.
fn dot() -> SetSprite {
    let c = [14.0, 14.0];
    let shapes = [Circle { c, r: 11.0, fill: [0, 0, 0, 70] }, Circle { c, r: 7.0, fill: [255, 255, 255, 230] }, Circle { c, r: 5.0, fill: [30, 30, 30, 255] }];
    rasterise_circles(28.0, 28.0, c, &shapes)
}

/// A translucent fingertip, like a phone screencast.
fn fingertip() -> SetSprite {
    let c = [16.0, 16.0];
    let shapes = [Circle { c, r: 14.0, fill: [255, 255, 255, 150] }, Circle { c, r: 11.5, fill: [120, 120, 120, 120] }, Circle { c, r: 4.0, fill: [40, 40, 40, 220] }];
    rasterise_circles(32.0, 32.0, c, &shapes)
}

// ───────────── pixel set ─────────────

/// A 1-bit arrow, scaled up without smoothing: `#` black outline, `.` white fill, space clear.
const PIXEL_ARROW: &[&str] = &[
    "#           ",
    "##          ",
    "#.#         ",
    "#..#        ",
    "#...#       ",
    "#....#      ",
    "#.....#     ",
    "#......#    ",
    "#.......#   ",
    "#........#  ",
    "#.........# ",
    "#.....######",
    "#..#..#     ",
    "#.#.#..#    ",
    "##  #..#    ",
    "#    #..#   ",
    "     #..#   ",
    "      ##    ",
];

const PIXEL_HAND: &[&str] = &[
    "     ##         ",
    "    #..#        ",
    "    #..#        ",
    "    #..#        ",
    "    #..###      ",
    "    #..#..###   ",
    "    #..#..#..## ",
    "#   #..#..#..#.#",
    "##  #..#..#..#.#",
    "#.# #..........#",
    "#..##..........#",
    " #.............#",
    " #............# ",
    "  #...........# ",
    "  #..........#  ",
    "   #.........#  ",
    "   #.........#  ",
    "    #########   ",
];

const PIXEL_IBEAM: &[&str] = &[
    "### ###",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "   #   ",
    "### ###",
];

fn pixel_set(kind: Kind) -> Option<SetSprite> {
    let (rows, hot) = match kind {
        Kind::Arrow => (PIXEL_ARROW, [0.0, 0.0]),
        Kind::PointingHand => (PIXEL_HAND, [5.5, 0.0]),
        Kind::IBeam => (PIXEL_IBEAM, [3.5, 7.5]),
        _ => return None,
    };
    Some(rasterise_pixels(rows, hot))
}

/// Each cell is 1.5 pt (3 px at 2×): `#` black, `.` white, anything else transparent.
fn rasterise_pixels(rows: &[&str], hot: [f64; 2]) -> SetSprite {
    const CELL: f64 = 1.5;
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let (w_pt, h_pt) = (cols as f64 * CELL, rows.len() as f64 * CELL);
    let (w, h) = ((w_pt * SCALE) as u32, (h_pt * SCALE) as u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let cell_px = (CELL * SCALE) as u32;
    for (ry, row) in rows.iter().enumerate() {
        for (rx, ch) in row.chars().enumerate() {
            let px = match ch {
                '#' => BLACK,
                '.' => WHITE,
                _ => continue,
            };
            for y in 0..cell_px {
                for x in 0..cell_px {
                    let (ix, iy) = (rx as u32 * cell_px + x, ry as u32 * cell_px + y);
                    let i = ((iy * w + ix) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&px);
                }
            }
        }
    }
    SetSprite { width: w, height: h, rgba, hotspot: [hot[0] * CELL, hot[1] * CELL], size: [w_pt, h_pt] }
}

// ───────────── rasteriser ─────────────

struct Circle {
    c: [f64; 2],
    r: f64,
    fill: Rgba,
}

fn rasterise_circles(w_pt: f64, h_pt: f64, hot: [f64; 2], shapes: &[Circle]) -> SetSprite {
    render(w_pt, h_pt, hot, |x, y| {
        let mut out: Option<Rgba> = None;
        for s in shapes {
            let d = ((x - s.c[0]).powi(2) + (y - s.c[1]).powi(2)).sqrt();
            if d <= s.r {
                out = Some(s.fill);
            }
        }
        out
    })
}

fn rasterise(w_pt: f64, h_pt: f64, hot: [f64; 2], shapes: &[Poly]) -> SetSprite {
    render(w_pt, h_pt, hot, |x, y| {
        let mut out: Option<Rgba> = None;
        for s in shapes {
            if inside(&s.pts, x, y) {
                out = Some(if s.outline_w > 0.0 && edge_distance(&s.pts, x, y) < s.outline_w { s.outline } else { s.fill });
            } else if s.outline_w > 0.0 && edge_distance(&s.pts, x, y) < s.outline_w * 0.5 {
                // half the outline sits outside the shape so thin parts keep their fill
                out = Some(s.outline);
            }
        }
        out
    })
}

/// Supersamples `colour_at(x, y)` (points) 4×4 per output pixel and averages, straight alpha.
fn render(w_pt: f64, h_pt: f64, hot: [f64; 2], colour_at: impl Fn(f64, f64) -> Option<Rgba>) -> SetSprite {
    let (w, h) = ((w_pt * SCALE).ceil() as u32, (h_pt * SCALE).ceil() as u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for py in 0..h {
        for px in 0..w {
            let mut acc = [0u32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let x = (px as f64 + (sx as f64 + 0.5) / 4.0) / SCALE;
                    let y = (py as f64 + (sy as f64 + 0.5) / 4.0) / SCALE;
                    if let Some(c) = colour_at(x, y) {
                        let a = c[3] as u32;
                        acc[0] += c[0] as u32 * a;
                        acc[1] += c[1] as u32 * a;
                        acc[2] += c[2] as u32 * a;
                        acc[3] += a;
                    }
                }
            }
            if acc[3] > 0 {
                let i = ((py * w + px) * 4) as usize;
                rgba[i] = (acc[0] / acc[3]) as u8;
                rgba[i + 1] = (acc[1] / acc[3]) as u8;
                rgba[i + 2] = (acc[2] / acc[3]) as u8;
                rgba[i + 3] = (acc[3] / 16) as u8;
            }
        }
    }
    SetSprite { width: w, height: h, rgba, hotspot: hot, size: [w_pt, h_pt] }
}

fn inside(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

/// Distance from a point to the nearest polygon edge.
fn edge_distance(poly: &[(f64, f64)], x: f64, y: f64) -> f64 {
    let mut best = f64::MAX;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (ax, ay) = poly[j];
        let (bx, by) = poly[i];
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { ((x - ax) * dx + (y - ay) * dy) / len2 } else { 0.0 }.clamp(0.0, 1.0);
        let (cx, cy) = (ax + t * dx, ay + t * dy);
        best = best.min(((x - cx).powi(2) + (y - cy).powi(2)).sqrt());
        j = i;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(s: &SetSprite, x_pt: f64, y_pt: f64) -> u8 {
        let (x, y) = ((x_pt * SCALE) as u32, (y_pt * SCALE) as u32);
        s.rgba[((y * s.width + x) * 4 + 3) as usize]
    }

    #[test]
    fn every_set_draws_an_arrow_except_the_recorded_one() {
        for (id, _) in SETS {
            let s = draw(id, "arrow");
            assert_eq!(s.is_some(), *id != RECORDED, "set {id}");
            if let Some(s) = s {
                assert_eq!(s.rgba.len(), (s.width * s.height * 4) as usize);
                assert!(s.hotspot[0] <= s.size[0] && s.hotspot[1] <= s.size[1]);
                assert!(s.rgba.chunks(4).any(|p| p[3] > 128), "set {id} drew nothing visible");
            }
        }
    }

    #[test]
    fn classic_arrow_is_dark_inside_and_light_on_the_edge() {
        let s = draw("macos", "arrow").unwrap();
        // the hotspot pixel and the corner are clear; the body is black, the border white
        assert_eq!(alpha_at(&s, 0.2, 0.2), 0);
        let body = ((7.0 * SCALE) as u32, (12.0 * SCALE) as u32);
        let i = ((body.1 * s.width + body.0) * 4) as usize;
        assert_eq!(&s.rgba[i..i + 4], &BLACK);
        let light = draw("light", "arrow").unwrap();
        assert_eq!(&light.rgba[i..i + 4], &WHITE);
    }

    #[test]
    fn uncovered_ids_fall_back_unless_the_set_covers_everything() {
        assert!(draw("macos", "resizeLeftRight").is_none());
        assert!(draw("pixel", "crosshair").is_none());
        assert!(draw("touch", "resizeLeftRight").is_some());
        assert!(draw("dot", "iBeam").is_some());
        assert!(covers_all("dot") && covers_all("touch") && !covers_all("macos"));
    }

    #[test]
    fn pixel_arrow_is_black_outline_white_fill_and_clear_padding() {
        let s = draw("pixel", "arrow").unwrap();
        assert_eq!(s.hotspot, [0.0, 0.0]);
        // cell (0,0) is black, cell (1,2) white fill, cell (5,0) clear
        let cell = |cx: f64, cy: f64| ((cy * 1.5 + 0.5) * SCALE) as u32 * s.width + ((cx * 1.5 + 0.5) * SCALE) as u32;
        let at = |cx: f64, cy: f64| &s.rgba[(cell(cx, cy) * 4) as usize..(cell(cx, cy) * 4 + 4) as usize];
        assert_eq!(at(0.0, 0.0), &BLACK);
        assert_eq!(at(1.0, 2.0), &WHITE);
        assert_eq!(at(5.0, 0.0)[3], 0);
    }
}

#[cfg(test)]
mod dump {
    #[test]
    #[ignore]
    fn dump_sprites() {
        let dir = std::path::PathBuf::from(std::env::var("SF_DUMP_CURSORS").unwrap());
        for (set, _) in super::SETS {
            for id in ["arrow", "pointingHand", "iBeam", "crosshair", "openHand", "closedHand"] {
                if let Some(s) = super::draw(set, id) {
                    let img = image::RgbaImage::from_raw(s.width, s.height, s.rgba).unwrap();
                    let img = image::imageops::resize(&img, s.width * 4, s.height * 4, image::imageops::FilterType::Nearest);
                    img.save(dir.join(format!("{set}-{id}.png"))).unwrap();
                }
            }
        }
    }
}
