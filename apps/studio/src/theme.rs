//! Visual theme matching Screen Studio's editor (rounded cards, purple accent, amber clips), in a
//! dark (Dracula) and a light (Solarized Light) variant; dark is the default, the user can pick
//! light or follow the system appearance.

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
    // Dracula
    bg: Color32::from_rgb(0x1e, 0x1f, 0x29),
    card: Color32::from_rgb(0x28, 0x2a, 0x36),
    card_2: Color32::from_rgb(0x34, 0x37, 0x46),
    card_3: Color32::from_rgb(0x44, 0x47, 0x5a),
    border: Color32::from_rgb(0x3b, 0x3e, 0x4f),
    text: Color32::from_rgb(0xf8, 0xf8, 0xf2),
    text_dim: Color32::from_rgb(0xa5, 0xa8, 0xc4),
    text_faint: Color32::from_rgb(0x62, 0x72, 0xa4),
    accent: Color32::from_rgb(0xbd, 0x93, 0xf9),
    accent_hover: Color32::from_rgb(0xca, 0xa9, 0xfa),
    clip: Color32::from_rgb(0xff, 0xb8, 0x6c),
    clip_selected: Color32::from_rgb(0xff, 0xc9, 0x8a),
    clip_text: Color32::from_rgb(0x28, 0x2a, 0x36),
    clip_line: Color32::from_rgba_premultiplied(0, 0, 0, 40),
    marker: Color32::from_rgb(0xf1, 0xfa, 0x8c),
    zoom: Color32::from_rgb(0xff, 0x79, 0xc6),
    zoom_selected: Color32::from_rgb(0xff, 0x92, 0xd0),
    track_empty: Color32::from_rgb(0x21, 0x22, 0x2c),
    playhead: Color32::from_rgb(0xf8, 0xf8, 0xf2),
    ruler_text: Color32::from_rgb(0x62, 0x72, 0xa4),
    danger: Color32::from_rgb(0xff, 0x55, 0x55),
    meter: Color32::from_rgb(0x50, 0xfa, 0x7b),
    glass: Color32::from_rgba_premultiplied(18, 18, 24, 112),
    glass_solid: Color32::from_rgba_premultiplied(34, 36, 46, 216),
    glass_border: Color32::from_rgba_premultiplied(34, 34, 34, 34),
    on_glass: Color32::from_rgb(0xf8, 0xf8, 0xf2),
    glass_button: Color32::from_rgb(0xf8, 0xf8, 0xf2),
    glass_button_fg: Color32::from_rgb(0x28, 0x2a, 0x36),
    thumb_empty: Color32::from_rgb(0x34, 0x37, 0x46),
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
    track_empty: Color32::from_rgb(0xe4, 0xdd, 0xc8),
    playhead: Color32::from_rgb(0x07, 0x36, 0x42),
    ruler_text: Color32::from_rgb(0x83, 0x94, 0x96),
    danger: Color32::from_rgb(0xdc, 0x32, 0x2f),
    meter: Color32::from_rgb(0x2e, 0x9e, 0x4f),
    glass: Color32::from_rgba_premultiplied(127, 123, 114, 128),
    glass_solid: Color32::from_rgba_premultiplied(228, 222, 205, 230),
    glass_border: Color32::from_rgba_premultiplied(1, 6, 8, 30),
    on_glass: Color32::from_rgb(0x07, 0x36, 0x42),
    glass_button: Color32::from_rgb(0x07, 0x36, 0x42),
    glass_button_fg: Color32::from_rgb(0xfd, 0xf6, 0xe3),
    thumb_empty: Color32::from_rgb(0xe4, 0xdd, 0xc8),
};

static PALETTE: std::sync::RwLock<Palette> = std::sync::RwLock::new(DARK);

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
    let want_dark = ctx.theme() == egui::Theme::Dark;
    if want_dark == is_dark() {
        return false;
    }
    apply(ctx);
    true
}

pub const RADIUS: u8 = 10;
pub const RADIUS_SM: u8 = 7;

/// Apply the palette matching `ctx.theme()` to egui's visuals and style.
#[allow(non_snake_case)]
pub fn apply(ctx: &egui::Context) {
    let dark = ctx.theme() == egui::Theme::Dark;
    let pal = if dark { DARK } else { LIGHT };
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
