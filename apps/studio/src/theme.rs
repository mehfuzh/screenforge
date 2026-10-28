//! Visual theme matching Screen Studio's editor (rounded cards, accent colour, amber clips). The
//! default is ScreenForge Dark, the website's dark colours (`site/*.html` tokens); ScreenForge
//! Light, Match system (ScreenForge light/dark), Catppuccin Mocha, Solarized Light and Osaka Jade
//! can be picked instead.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

/// Colours for one appearance. `theme::p()` returns the active one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,          // window background
    pub card: Color32,        // sidebar card / rails
    pub card_2: Color32,      // buttons on a card
    pub card_3: Color32,      // hovered / selected segment
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,      // sliders, export button
    pub accent_hover: Color32,
    pub clip: Color32,        // amber clip blocks
    pub clip_selected: Color32,
    pub clip_text: Color32,
    pub clip_line: Color32,
    pub marker: Color32,
    pub zoom: Color32,
    pub zoom_selected: Color32,
    pub mask: Color32,
    pub mask_selected: Color32,
    /// Title and overlay-media blocks on the timeline.
    pub title: Color32,
    pub overlay: Color32,
    pub track_empty: Color32,
    pub playhead: Color32,
    pub ruler_text: Color32,
    pub danger: Color32,
    /// Live input level (the microphone icon fills with it).
    pub meter: Color32,
    /// Floating bars (picker, recording HUD, recent panel): tint over vibrancy, solid fallback, border.
    pub glass: Color32,
    pub glass_solid: Color32,
    pub glass_border: Color32,
    /// Icons / primary text drawn on the glass bars.
    pub on_glass: Color32,
    /// Contrasting colour for a round button on glass (the close button) and its glyph.
    pub glass_button: Color32,
    pub glass_button_fg: Color32,
    /// Placeholder thumbnail fill in the recent-projects panel.
    pub thumb_empty: Color32,
}

pub const DARK: Palette = Palette {
    dark: true,
    // Catppuccin Mocha
    bg: Color32::from_rgb(0x18, 0x18, 0x25),          // mantle
    card: Color32::from_rgb(0x1e, 0x1e, 0x2e),        // base
    card_2: Color32::from_rgb(0x31, 0x32, 0x44),      // surface0
    card_3: Color32::from_rgb(0x45, 0x47, 0x5a),      // surface1
    border: Color32::from_rgb(0x31, 0x32, 0x44),
    text: Color32::from_rgb(0xcd, 0xd6, 0xf4),
    text_dim: Color32::from_rgb(0xa6, 0xad, 0xc8),    // subtext0
    text_faint: Color32::from_rgb(0x6c, 0x70, 0x86),  // overlay0
    accent: Color32::from_rgb(0xcb, 0xa6, 0xf7),      // mauve
    accent_hover: Color32::from_rgb(0xd6, 0xbb, 0xf9),
    clip: Color32::from_rgb(0xfa, 0xb3, 0x87),        // peach
    clip_selected: Color32::from_rgb(0xfc, 0xc6, 0xa3),
    clip_text: Color32::from_rgb(0x1e, 0x1e, 0x2e),
    clip_line: Color32::from_rgba_premultiplied(0, 0, 0, 40),
    marker: Color32::from_rgb(0xf9, 0xe2, 0xaf),      // yellow
    zoom: Color32::from_rgb(0xf5, 0xc2, 0xe7),        // pink
    zoom_selected: Color32::from_rgb(0xf8, 0xd4, 0xee),
    mask: Color32::from_rgb(0x89, 0xdc, 0xeb),        // sky
    mask_selected: Color32::from_rgb(0xa6, 0xe5, 0xf0),
    title: Color32::from_rgb(0xcb, 0xa6, 0xf7),        // mauve
    overlay: Color32::from_rgb(0xfa, 0xb3, 0x87),        // peach
    track_empty: Color32::from_rgb(0x1b, 0x1b, 0x29),
    playhead: Color32::from_rgb(0xcd, 0xd6, 0xf4),
    ruler_text: Color32::from_rgb(0x7f, 0x84, 0x9c),  // overlay1
    danger: Color32::from_rgb(0xf3, 0x8b, 0xa8),      // red
    meter: Color32::from_rgb(0xa6, 0xe3, 0xa1),       // green
    glass: Color32::from_rgba_premultiplied(18, 18, 28, 190),
    glass_solid: Color32::from_rgba_premultiplied(26, 26, 39, 216),
    glass_border: Color32::from_rgba_premultiplied(34, 34, 34, 34),
    on_glass: Color32::from_rgb(0xcd, 0xd6, 0xf4),
    glass_button: Color32::from_rgb(0xcd, 0xd6, 0xf4),
    glass_button_fg: Color32::from_rgb(0x1e, 0x1e, 0x2e),
    thumb_empty: Color32::from_rgb(0x31, 0x32, 0x44),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    // Solarized Light
    bg: Color32::from_rgb(0xee, 0xe8, 0xd5),
    card: Color32::from_rgb(0xfd, 0xf6, 0xe3),
    card_2: Color32::from_rgb(0xe4, 0xdd, 0xc8),
    card_3: Color32::from_rgb(0xd8, 0xd1, 0xbb),
    border: Color32::from_rgb(0xd6, 0xcf, 0xb8),
    text: Color32::from_rgb(0x07, 0x36, 0x42),
    text_dim: Color32::from_rgb(0x65, 0x7b, 0x83),
    text_faint: Color32::from_rgb(0x93, 0xa1, 0xa1),
    accent: Color32::from_rgb(0x6c, 0x71, 0xc4),
    accent_hover: Color32::from_rgb(0x5a, 0x5f, 0xb8),
    clip: Color32::from_rgb(0xb5, 0x89, 0x00),
    clip_selected: Color32::from_rgb(0xcb, 0x9a, 0x00),
    clip_text: Color32::from_rgb(0xfd, 0xf6, 0xe3),
    clip_line: Color32::from_rgba_premultiplied(0, 0, 0, 40),
    marker: Color32::from_rgb(0xcb, 0x4b, 0x16),
    zoom: Color32::from_rgb(0xd3, 0x36, 0x82),
    zoom_selected: Color32::from_rgb(0xe0, 0x4a, 0x95),
    mask: Color32::from_rgb(0x2a, 0xa1, 0x98),
    mask_selected: Color32::from_rgb(0x35, 0xb5, 0xab),
    title: Color32::from_rgb(0x6c, 0x71, 0xc4),        // violet
    overlay: Color32::from_rgb(0xcb, 0x4b, 0x16),        // orange
    track_empty: Color32::from_rgb(0xe4, 0xdd, 0xc8),
    playhead: Color32::from_rgb(0x07, 0x36, 0x42),
    ruler_text: Color32::from_rgb(0x83, 0x94, 0x96),
    danger: Color32::from_rgb(0xdc, 0x32, 0x2f),
    meter: Color32::from_rgb(0x2e, 0x9e, 0x4f),
    glass: Color32::from_rgba_premultiplied(189, 183, 170, 190),
    glass_solid: Color32::from_rgba_premultiplied(228, 222, 205, 230),
    glass_border: Color32::from_rgba_premultiplied(1, 6, 8, 30),
    on_glass: Color32::from_rgb(0x07, 0x36, 0x42),
    glass_button: Color32::from_rgb(0x07, 0x36, 0x42),
    glass_button_fg: Color32::from_rgb(0xfd, 0xf6, 0xe3),
    thumb_empty: Color32::from_rgb(0xe4, 0xdd, 0xc8),
};

pub const OSAKA_JADE: Palette = Palette {
    dark: true,
    // Osaka Jade: deep green-black with jade and teal accents
    bg: Color32::from_rgb(0x11, 0x1c, 0x18),
    card: Color32::from_rgb(0x17, 0x26, 0x20),
    card_2: Color32::from_rgb(0x1f, 0x33, 0x2a),
    card_3: Color32::from_rgb(0x2a, 0x42, 0x36),
    border: Color32::from_rgb(0x23, 0x37, 0x2b),
    text: Color32::from_rgb(0xf6, 0xf5, 0xdd),
    text_dim: Color32::from_rgb(0xc1, 0xc4, 0x97),
    text_faint: Color32::from_rgb(0x6b, 0x83, 0x72),
    // muted jade: the theme's bright teal (#2dd5b7) glared on buttons
    accent: Color32::from_rgb(0x3a, 0x93, 0x7e),
    accent_hover: Color32::from_rgb(0x4b, 0xa8, 0x91),
    clip: Color32::from_rgb(0x54, 0x9e, 0x6a),
    clip_selected: Color32::from_rgb(0x63, 0xb0, 0x7a),
    clip_text: Color32::from_rgb(0x11, 0x1c, 0x18),
    clip_line: Color32::from_rgba_premultiplied(0, 0, 0, 40),
    marker: Color32::from_rgb(0xe5, 0xc7, 0x36),
    zoom: Color32::from_rgb(0xd2, 0x68, 0x9c),
    zoom_selected: Color32::from_rgb(0xdb, 0x7f, 0xae),
    mask: Color32::from_rgb(0x5f, 0x9e, 0xc9),
    mask_selected: Color32::from_rgb(0x76, 0xae, 0xd4),
    title: Color32::from_rgb(0x9d, 0x8c, 0xd6),
    overlay: Color32::from_rgb(0xe0, 0x9e, 0x5a),
    track_empty: Color32::from_rgb(0x14, 0x21, 0x1b),
    playhead: Color32::from_rgb(0xf6, 0xf5, 0xdd),
    ruler_text: Color32::from_rgb(0x6b, 0x83, 0x72),
    danger: Color32::from_rgb(0xff, 0x53, 0x45),
    meter: Color32::from_rgb(0x63, 0xb0, 0x7a),
    glass: Color32::from_rgba_premultiplied(8, 14, 12, 112),
    glass_solid: Color32::from_rgba_premultiplied(18, 30, 25, 216),
    glass_border: Color32::from_rgba_premultiplied(34, 34, 34, 34),
    on_glass: Color32::from_rgb(0xf6, 0xf5, 0xdd),
    glass_button: Color32::from_rgb(0xf6, 0xf5, 0xdd),
    glass_button_fg: Color32::from_rgb(0x11, 0x1c, 0x18),
    thumb_empty: Color32::from_rgb(0x1f, 0x33, 0x2a),
};

/// ScreenForge dark: the website's dark tokens (ground #15161e, ink, panel, orange accent).
pub const FORGE_DARK: Palette = Palette {
    dark: true,
    bg: Color32::from_rgb(0x15, 0x16, 0x1e),          // --ground
    card: Color32::from_rgb(0x1e, 0x1f, 0x29),        // --ink
    card_2: Color32::from_rgb(0x28, 0x2a, 0x36),      // --panel
    card_3: Color32::from_rgb(0x36, 0x38, 0x47),
    border: Color32::from_rgb(0x2b, 0x2d, 0x3a),
    text: Color32::from_rgb(0xe6, 0xe7, 0xf0),
    text_dim: Color32::from_rgb(0x8e, 0x90, 0xa6),    // --dim
    text_faint: Color32::from_rgb(0x5f, 0x61, 0x75),
    accent: Color32::from_rgb(0xff, 0x8a, 0x5c),      // --jade (orange)
    accent_hover: Color32::from_rgb(0xff, 0xa2, 0x7d),
    clip: Color32::from_rgb(0xff, 0xb8, 0x6c),        // --gold
    clip_selected: Color32::from_rgb(0xff, 0xc9, 0x8d),
    clip_text: Color32::from_rgb(0x1e, 0x1f, 0x29),
    clip_line: Color32::from_rgba_premultiplied(0, 0, 0, 40),
    marker: Color32::from_rgb(0xf1, 0xfa, 0x8c),
    zoom: Color32::from_rgb(0xe0, 0x50, 0x7a),        // --jade-deep
    zoom_selected: Color32::from_rgb(0xe8, 0x6c, 0x91),
    mask: Color32::from_rgb(0x8b, 0xe9, 0xfd),
    mask_selected: Color32::from_rgb(0xa8, 0xef, 0xfd),
    title: Color32::from_rgb(0xa7, 0x8b, 0xfa),
    overlay: Color32::from_rgb(0xfb, 0x92, 0x3c),
    track_empty: Color32::from_rgb(0x1a, 0x1b, 0x24),
    playhead: Color32::from_rgb(0xf8, 0xf8, 0xf2),    // --white
    ruler_text: Color32::from_rgb(0x8e, 0x90, 0xa6),
    danger: Color32::from_rgb(0xff, 0x55, 0x55),      // --coral
    meter: Color32::from_rgb(0x50, 0xfa, 0x7b),
    glass: Color32::from_rgba_premultiplied(16, 17, 23, 190),
    glass_solid: Color32::from_rgba_premultiplied(18, 19, 25, 216),
    glass_border: Color32::from_rgba_premultiplied(34, 34, 34, 34),
    on_glass: Color32::from_rgb(0xf8, 0xf8, 0xf2),
    glass_button: Color32::from_rgb(0xf8, 0xf8, 0xf2),
    glass_button_fg: Color32::from_rgb(0x1e, 0x1f, 0x29),
    thumb_empty: Color32::from_rgb(0x28, 0x2a, 0x36),
};

/// ScreenForge light: the website's light tokens (ground #ececf1, white panels, red-orange accent).
pub const FORGE_LIGHT: Palette = Palette {
    dark: false,
    bg: Color32::from_rgb(0xec, 0xec, 0xf1),          // --ground
    card: Color32::from_rgb(0xff, 0xff, 0xff),        // --panel
    card_2: Color32::from_rgb(0xec, 0xec, 0xf2),
    card_3: Color32::from_rgb(0xdd, 0xdd, 0xe6),
    border: Color32::from_rgb(0xd6, 0xd6, 0xdf),
    text: Color32::from_rgb(0x1a, 0x1b, 0x24),        // --white (light)
    text_dim: Color32::from_rgb(0x5f, 0x61, 0x75),    // --dim
    text_faint: Color32::from_rgb(0x93, 0x95, 0xa7),
    accent: Color32::from_rgb(0xd4, 0x44, 0x2a),      // --jade (red-orange)
    accent_hover: Color32::from_rgb(0xe0, 0x5b, 0x40),
    clip: Color32::from_rgb(0xb8, 0x63, 0x1a),        // --gold
    clip_selected: Color32::from_rgb(0xc9, 0x75, 0x2a),
    clip_text: Color32::from_rgb(0xff, 0xff, 0xff),
    clip_line: Color32::from_rgba_premultiplied(0, 0, 0, 40),
    marker: Color32::from_rgb(0xc4, 0x8a, 0x00),
    zoom: Color32::from_rgb(0xb8, 0x32, 0x5f),        // --jade-deep
    zoom_selected: Color32::from_rgb(0xc9, 0x48, 0x74),
    mask: Color32::from_rgb(0x1f, 0x8a, 0xa3),
    mask_selected: Color32::from_rgb(0x2b, 0x9e, 0xb8),
    title: Color32::from_rgb(0x6d, 0x28, 0xd9),
    overlay: Color32::from_rgb(0xc2, 0x41, 0x0c),
    track_empty: Color32::from_rgb(0xe3, 0xe3, 0xea),
    playhead: Color32::from_rgb(0x1a, 0x1b, 0x24),
    ruler_text: Color32::from_rgb(0x5f, 0x61, 0x75),
    danger: Color32::from_rgb(0xd2, 0x3b, 0x3b),      // --coral
    meter: Color32::from_rgb(0x2e, 0x9e, 0x4f),
    glass: Color32::from_rgba_premultiplied(175, 175, 179, 190),
    glass_solid: Color32::from_rgba_premultiplied(212, 212, 217, 230),
    glass_border: Color32::from_rgba_premultiplied(1, 2, 6, 30),
    on_glass: Color32::from_rgb(0x1a, 0x1b, 0x24),
    glass_button: Color32::from_rgb(0x1a, 0x1b, 0x24),
    glass_button_fg: Color32::from_rgb(0xff, 0xff, 0xff),
    thumb_empty: Color32::from_rgb(0xe3, 0xe3, 0xea),
};

static PALETTE: std::sync::RwLock<Palette> = std::sync::RwLock::new(FORGE_DARK);
/// Palettes used when egui's theme is dark / light (ScreenForge by default).
static DARK_PALETTE: std::sync::RwLock<Palette> = std::sync::RwLock::new(FORGE_DARK);
static LIGHT_PALETTE: std::sync::RwLock<Palette> = std::sync::RwLock::new(FORGE_LIGHT);

/// Choose the palettes the dark and light appearances use; call `sync` afterwards.
pub fn set_palettes(dark: Palette, light: Palette) {
    *DARK_PALETTE.write().unwrap_or_else(|e| e.into_inner()) = dark;
    *LIGHT_PALETTE.write().unwrap_or_else(|e| e.into_inner()) = light;
}

fn wanted(ctx: &egui::Context) -> Palette {
    let slot = if ctx.theme() == egui::Theme::Dark { &DARK_PALETTE } else { &LIGHT_PALETTE };
    *slot.read().unwrap_or_else(|e| e.into_inner())
}

/// The active palette.
pub fn p() -> Palette {
    *PALETTE.read().unwrap_or_else(|e| e.into_inner())
}

pub fn is_dark() -> bool {
    p().dark
}

/// White (dark mode) or black (light mode) at the given alpha, for hover fills and dividers on
/// glass bars.
pub fn glass_alpha(alpha: u8) -> Color32 {
    if is_dark() { Color32::from_rgba_unmultiplied(255, 255, 255, alpha) } else { Color32::from_rgba_unmultiplied(0, 0, 0, alpha) }
}

/// Re-apply the palette when egui's theme (system or user preference) changed. Returns true when it did.
pub fn sync(ctx: &egui::Context) -> bool {
    if wanted(ctx) == p() {
        return false;
    }
    apply(ctx);
    true
}

pub const RADIUS: u8 = 10;
pub const RADIUS_SM: u8 = 7;

/// System font with the key-cap symbols egui's bundled fonts lack (⌥ ⌫ ⌃), tried in order.
#[cfg(target_os = "macos")]
const SYMBOL_FONTS: &[&str] = &["/System/Library/Fonts/Apple Symbols.ttf"];
#[cfg(target_os = "windows")]
const SYMBOL_FONTS: &[&str] = &["C:\\Windows\\Fonts\\seguisym.ttf"];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const SYMBOL_FONTS: &[&str] = &["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/dejavu/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf"];

/// Install the fonts once: egui's proportional face has no ⇧ ↑ ↓, so Hack (which does) becomes
/// a fallback for it, and the system's symbol font, where present, supplies ⌥ ⌫ ⌃ for shortcut
/// hints. Without it those characters would show as boxes.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let families = [FontFamily::Proportional, FontFamily::Monospace];
    for fam in &families {
        let list = fonts.families.entry(fam.clone()).or_default();
        if !list.iter().any(|n| n == "Hack") {
            list.push("Hack".to_owned());
        }
    }
    if let Some(bytes) = SYMBOL_FONTS.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("symbols".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        for fam in &families {
            fonts.families.entry(fam.clone()).or_default().push("symbols".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// Apply the palette matching `ctx.theme()` to egui's visuals and style.
#[allow(non_snake_case)]
pub fn apply(ctx: &egui::Context) {
    let pal = wanted(ctx);
    let dark = pal.dark;
    *PALETTE.write().unwrap_or_else(|e| e.into_inner()) = pal;
    let Palette { bg: BG, card: CARD, card_2: CARD_2, card_3: CARD_3, text: TEXT, text_dim: TEXT_DIM, accent: ACCENT, accent_hover: ACCENT_HOVER, .. } = pal;
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = BG;
    // popups, menus and dialogs: translucent glass over whatever is beneath
    v.window_fill = pal.glass_solid;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.bg_fill = CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    v.widgets.inactive.bg_fill = CARD_2;
    v.widgets.inactive.weak_bg_fill = CARD_2;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_fill = CARD_3;
    v.widgets.hovered.weak_bg_fill = CARD_3;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.bg_fill = CARD_3;
    v.widgets.active.weak_bg_fill = CARD_3;
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.open.bg_fill = CARD_3;
    v.widgets.open.weak_bg_fill = CARD_3;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, ACCENT_HOVER);
    v.window_corner_radius = CornerRadius::same(14);
    v.window_stroke = Stroke::new(1.0, pal.glass_border);
    v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 32, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 50 }) };
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if dark { 90 } else { 40 }) };
    v.menu_corner_radius = CornerRadius::same(8);
    // buttons and menu items show the pointing hand (the custom icon buttons set it themselves)
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open, &mut v.widgets.noninteractive] {
        w.corner_radius = CornerRadius::same(RADIUS_SM);
        w.expansion = 0.0;
    }
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    ctx.set_visuals(v.clone());
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.slider_width = 200.0;
        style.spacing.slider_rail_height = 3.0;
        style.spacing.interact_size = egui::vec2(36.0, 24.0);
        style.spacing.combo_width = 120.0;
        style.spacing.menu_margin = egui::Margin::same(8);
        // macOS-like overlay scrollbars: thin, translucent, never a solid white bar on hover
        style.spacing.scroll = egui::style::ScrollStyle {
            ..egui::style::ScrollStyle::thin()
        };
        style.spacing.scroll = egui::style::ScrollStyle {
            bar_width: 6.0,
            handle_min_length: 24.0,
            bar_inner_margin: 2.0,
            bar_outer_margin: 2.0,
            floating: true,
            floating_width: 4.0,
            floating_allocated_width: 0.0,
            foreground_color: true,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.0,
            dormant_handle_opacity: 0.15,
            active_handle_opacity: 0.35,
            interact_handle_opacity: 0.5,
            ..egui::style::ScrollStyle::thin()
        };
        style.text_styles.insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
        style.visuals.indent_has_left_vline = false;
    });
    ctx.set_visuals(v);
}
