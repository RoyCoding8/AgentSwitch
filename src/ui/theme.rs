use egui::{Color32, FontId, Visuals, Style, CornerRadius, Stroke};

pub const BG_DARK: Color32 = Color32::from_rgb(0x18, 0x18, 0x1B);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x22);
pub const BG_SIDEBAR: Color32 = Color32::from_rgb(0x14, 0x14, 0x17);
pub const BG_HOVER: Color32 = Color32::from_rgb(0x2C, 0x2C, 0x32);
pub const BG_SELECTED: Color32 = Color32::from_rgb(0x38, 0x38, 0x42);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE0, 0xE0, 0xE0);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x80, 0x80, 0x88);
pub const TEXT_ACCENT: Color32 = Color32::from_rgb(0xA0, 0xD0, 0xFF);
pub const BORDER: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x44);
pub const GREEN: Color32 = Color32::from_rgb(0x50, 0xC8, 0x78);
pub const YELLOW: Color32 = Color32::from_rgb(0xE8, 0xC0, 0x40);

const BTN_BG: Color32 = Color32::from_rgb(0x34, 0x34, 0x3E);
const BTN_HOVER: Color32 = Color32::from_rgb(0x44, 0x44, 0x50);
const BTN_ACTIVE: Color32 = Color32::from_rgb(0x50, 0x50, 0x5E);

pub fn apply(ctx: &egui::Context) {
    let mut vis = Visuals::dark();
    vis.panel_fill = BG_PANEL;
    vis.window_fill = BG_DARK;
    vis.faint_bg_color = BG_HOVER;
    vis.extreme_bg_color = Color32::from_rgb(0x10, 0x10, 0x14);
    // non-interactive
    vis.widgets.noninteractive.bg_fill = BG_PANEL;
    vis.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    vis.widgets.noninteractive.bg_stroke = Stroke::new(0.5, BORDER);
    // inactive (buttons at rest)
    vis.widgets.inactive.bg_fill = BTN_BG;
    vis.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    vis.widgets.inactive.bg_stroke = Stroke::new(0.5, BORDER);
    vis.widgets.inactive.corner_radius = CornerRadius::same(4);
    // hovered
    vis.widgets.hovered.bg_fill = BTN_HOVER;
    vis.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    vis.widgets.hovered.bg_stroke = Stroke::new(1.0, TEXT_ACCENT);
    vis.widgets.hovered.corner_radius = CornerRadius::same(4);
    // active (pressed)
    vis.widgets.active.bg_fill = BTN_ACTIVE;
    vis.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    vis.widgets.active.corner_radius = CornerRadius::same(4);
    // selection
    vis.selection.bg_fill = Color32::from_rgba_premultiplied(0x50, 0x80, 0xC0, 0x60);
    vis.selection.stroke = Stroke::new(1.0, TEXT_ACCENT);
    vis.window_stroke = Stroke::new(1.0, BORDER);
    // override text color globally for visibility
    vis.override_text_color = Some(TEXT_PRIMARY);

    let mut style = Style { visuals: vis, ..Style::default() };
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    ctx.set_style(style);
}

pub fn heading_font() -> FontId { FontId::proportional(18.0) }
pub fn body_font() -> FontId { FontId::proportional(14.0) }
pub fn small_font() -> FontId { FontId::proportional(12.0) }
