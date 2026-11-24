use egui::{Color32, Context, FontId, RichText, Rounding, Stroke, TextStyle, Ui, Visuals};

/// Beekeeper Studio-inspired warm dark palette.
pub const BG_BASE: Color32 = Color32::from_rgb(28, 28, 28); // #1c1c1c
pub const BG_SIDEBAR: Color32 = Color32::from_rgb(23, 23, 23); // #171717
pub const BG_SURFACE: Color32 = Color32::from_rgb(36, 36, 36); // #242424
pub const BG_SURFACE_ALT: Color32 = Color32::from_rgb(32, 32, 32); // #202020
pub const BG_HOVER: Color32 = Color32::from_rgb(46, 46, 46); // #2e2e2e
pub const BG_SELECTED: Color32 = Color32::from_rgb(56, 48, 22); // warm amber tint
pub const BG_EDITED: Color32 = Color32::from_rgb(66, 53, 0); // #423500 amber cell highlight
pub const BG_DELETED: Color32 = Color32::from_rgb(60, 20, 20); // dark red row tint
pub const BG_INSERTED: Color32 = Color32::from_rgb(17, 44, 31); // dark green row tint

pub const BORDER: Color32 = Color32::from_rgb(54, 54, 54); // #363636
pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(44, 44, 44); // #2c2c2c

pub const ACCENT: Color32 = Color32::from_rgb(250, 214, 102); // #fad666 Beekeeper yellow
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(245, 197, 66); // #f5c542
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(39, 33, 9); // #272109 dark contrast

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(232, 232, 232); // #e8e8e8
pub const TEXT_MUTED: Color32 = Color32::from_rgb(142, 142, 142); // #8e8e8e
pub const TEXT_NULL: Color32 = Color32::from_rgb(112, 112, 112); // #707070

pub const SUCCESS: Color32 = Color32::from_rgb(21, 219, 149); // #15db95
pub const DANGER: Color32 = Color32::from_rgb(255, 93, 89); // #ff5d59
pub const WARNING: Color32 = Color32::from_rgb(255, 141, 33); // #ff8d21
pub const INFO: Color32 = Color32::from_rgb(52, 152, 219); // #3498db

/// Apply Beekeeper Studio dark theme styling to the egui context.
pub fn apply_beekeeper_theme(ctx: &Context) {
    let mut visuals = Visuals::dark();

    visuals.dark_mode = true;
    visuals.override_text_color = Some(TEXT_PRIMARY);
    visuals.panel_fill = BG_BASE;
    visuals.window_fill = BG_SURFACE;
    visuals.extreme_bg_color = BG_SIDEBAR;
    visuals.faint_bg_color = BG_SURFACE_ALT;
    visuals.code_bg_color = BG_SIDEBAR;

    visuals.window_rounding = Rounding::same(6.0);
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.menu_rounding = Rounding::same(4.0);

    visuals.selection.bg_fill = BG_SELECTED;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.weak_bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.noninteractive.rounding = Rounding::same(4.0);

    visuals.widgets.inactive.bg_fill = BG_SURFACE;
    visuals.widgets.inactive.weak_bg_fill = BG_SURFACE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);

    visuals.widgets.hovered.bg_fill = BG_HOVER;
    visuals.widgets.hovered.weak_bg_fill = BG_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);

    visuals.widgets.active.bg_fill = BG_SELECTED;
    visuals.widgets.active.weak_bg_fill = BG_SELECTED;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_HOVER);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.rounding = Rounding::same(4.0);

    visuals.widgets.open.bg_fill = BG_HOVER;
    visuals.widgets.open.weak_bg_fill = BG_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.open.rounding = Rounding::same(4.0);

    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = DANGER;
    visuals.striped = true;

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.window_margin = egui::Margin::same(12.0);
    style.spacing.interact_size.y = 26.0;

    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(18.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(13.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.0, egui::FontFamily::Monospace),
    );

    ctx.set_style(style);
}

/// Primary yellow Beekeeper-styled action button.
pub fn accent_button(ui: &mut Ui, label: &str) -> egui::Response {
    let btn = egui::Button::new(RichText::new(label).color(ACCENT_TEXT).strong())
        .fill(ACCENT)
        .stroke(Stroke::new(1.0_f32, ACCENT_HOVER))
        .rounding(Rounding::same(4.0));
    ui.add(btn)
}

/// Danger red action button (e.g. Cancel, Delete).
pub fn danger_button(ui: &mut Ui, label: &str) -> egui::Response {
    let btn = egui::Button::new(RichText::new(label).color(Color32::WHITE).strong())
        .fill(Color32::from_rgb(175, 45, 45))
        .stroke(Stroke::new(1.0_f32, DANGER))
        .rounding(Rounding::same(4.0));
    ui.add(btn)
}

/// Compact inline pill badge (e.g. PK, VIEW, row counts).
pub fn badge(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) {
    egui::Frame::none()
        .fill(bg)
        .rounding(Rounding::same(3.0))
        .inner_margin(egui::Margin::symmetric(5.0, 1.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.5).color(fg).strong());
        });
}
