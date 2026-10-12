//! Primitive value types shared by all schema versions.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Unknown fields preserved for round-tripping.
pub type Extra = Map<String, Value>;

/// A point. Fields suffixed `01` in Screen Studio are normalized to `0..=1` of the
/// containing rect; others are in CSS points (not pixels).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Axis aligned rect; `rect01` variants are normalized 0..=1, origin top-left, y down.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const UNIT: Rect = Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
}

impl Default for Rect {
    fn default() -> Self {
        Rect::UNIT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// `#rrggbb` (lowercase in v4 files, mixed case in v3).
pub type HexColor = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub color: HexColor,
    /// 0..=1 position along the gradient
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub start: Point,
    pub end: Point,
    pub stops: Vec<GradientStop>,
}

impl Default for Gradient {
    fn default() -> Self {
        Gradient {
            start: Point::new(0.0, 0.0),
            end: Point::new(1.0, 1.0),
            stops: vec![
                GradientStop { color: "#4776e6".into(), at: 0.0 },
                GradientStop { color: "#8e54e9".into(), at: 1.0 },
            ],
        }
    }
}

/// Damped spring parameters (react-spring style). Screen Studio animates the virtual camera
/// (zoom/pan) and the cursor with these.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    pub stiffness: f64,
    pub damping: f64,
    pub mass: f64,
    /// When true the spring never overshoots its target (v4 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clamp: Option<bool>,
    /// Rest threshold for position and velocity (v4 only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<f64>,
}

impl Spring {
    pub const fn new(stiffness: f64, damping: f64, mass: f64) -> Self {
        Spring { stiffness, damping, mass, clamp: None, precision: None }
    }
}

/// Cursor set selection: a built-in set id (`macos`, `macos-tahoe`, `touch`, `halloween`,
/// `custom-1..20`) plus optional per-cursor-type overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorSet {
    pub id: String,
    #[serde(default)]
    pub variants: Map<String, Value>,
}

impl Default for CursorSet {
    fn default() -> Self {
        CursorSet { id: "macos-tahoe".into(), variants: Map::new() }
    }
}

/// `output.aspectRatio`: `"auto"` or a number (width / height).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AspectRatio {
    Ratio(f64),
    Auto(AutoTag),
}

/// Serializes as the literal string `"auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutoTag {
    #[serde(rename = "auto")]
    #[default]
    Auto,
}

impl Default for AspectRatio {
    fn default() -> Self {
        AspectRatio::Auto(AutoTag::Auto)
    }
}

impl AspectRatio {
    pub const AUTO: AspectRatio = AspectRatio::Auto(AutoTag::Auto);

    pub fn ratio(w: f64, h: f64) -> Self {
        AspectRatio::Ratio(w / h)
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            AspectRatio::Ratio(r) if r.is_finite() && *r > 0.0 => Some(*r),
            _ => None,
        }
    }

    /// UI presets as Screen Studio shows them.
    pub const PRESETS: &'static [(&'static str, Option<(f64, f64)>)] = &[
        ("Auto", None),
        ("Wide 16:9", Some((16.0, 9.0))),
        ("Square 1:1", Some((1.0, 1.0))),
        ("Classic 4:3", Some((4.0, 3.0))),
        ("Vertical 9:16", Some((9.0, 16.0))),
        ("Tall 3:4", Some((3.0, 4.0))),
        ("Portrait 4:5", Some((4.0, 5.0))),
    ];

    pub fn label(&self) -> String {
        match self.as_f64() {
            None => "Auto".into(),
            Some(r) => Self::PRESETS
                .iter()
                .find(|(_, p)| p.map(|(w, h)| ((w / h) - r).abs() < 1e-6).unwrap_or(false))
                .map(|(l, _)| (*l).to_string())
                .unwrap_or_else(|| format!("{r:.3}")),
        }
    }
}
