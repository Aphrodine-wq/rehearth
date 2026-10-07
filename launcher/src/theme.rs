//! Colours and the small widgets every screen shares.

use eframe::egui;
use egui::{Color32, CornerRadius, FontId, RichText, Stroke};

use crate::art;

// warm hearth tones, translucent where they sit on the title art
pub const BG: Color32 = Color32::from_rgb(0x1a, 0x16, 0x12);
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(0x16, 0x12, 0x0e, 0xe0);
pub const CARD: Color32 = Color32::from_rgba_premultiplied(0x2a, 0x23, 0x1b, 0xe8);
pub const CARD_BG: Color32 = Color32::from_rgb(0x26, 0x20, 0x19);
pub const FEATURE_BG: Color32 = Color32::from_rgb(0x2e, 0x24, 0x18);
pub const CHIP_BG: Color32 = Color32::from_rgb(0x3a, 0x30, 0x25);
pub const LINE: Color32 = Color32::from_rgb(0x4a, 0x3e, 0x30);
pub const TEXT: Color32 = Color32::from_rgb(0xf2, 0xe6, 0xd2);
pub const DIM: Color32 = Color32::from_rgb(0xb4, 0xa5, 0x8e);
pub const AMBER: Color32 = Color32::from_rgb(0xf2, 0xa9, 0x3b);
pub const AMBER_DIM: Color32 = Color32::from_rgb(0x8a, 0x55, 0x1c);
pub const GOOD: Color32 = Color32::from_rgb(0x8c, 0xc8, 0x6a);
pub const BAD: Color32 = Color32::from_rgb(0xe8, 0x70, 0x5c);

pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = Color32::from_rgba_premultiplied(0x0e, 0x0b, 0x08, 0xf0);
    visuals.faint_bg_color = CARD_BG;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = AMBER.linear_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, AMBER);
    visuals.hyperlink_color = AMBER;
    let radius = CornerRadius::same(5);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    visuals.widgets.inactive.weak_bg_fill = CHIP_BG;
    visuals.widgets.inactive.bg_fill = CHIP_BG;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x7a, 0x66, 0x4e));
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.5, TEXT);
    visuals.widgets.hovered.weak_bg_fill = LINE;
    visuals.widgets.hovered.bg_fill = LINE;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, AMBER);
    visuals.widgets.active.weak_bg_fill = AMBER_DIM;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.text_styles.insert(egui::TextStyle::Body, FontId::proportional(15.5));
        style.text_styles.insert(egui::TextStyle::Button, FontId::proportional(15.5));
        style.text_styles.insert(egui::TextStyle::Small, FontId::proportional(12.5));
    });
}

/// The translucent panel each screen sits in, over the title art.
pub fn panel<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(20)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            add(ui)
        })
        .inner
}

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(FontId::new(30.0, art::heading_family())).color(AMBER));
    ui.add_space(2.0);
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(FontId::new(19.0, art::heading_family())).color(TEXT));
}

pub fn card_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(FontId::new(17.0, art::heading_family())).color(AMBER));
}

pub fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).color(if selected { BG } else { TEXT }))
        .fill(if selected { AMBER } else { CHIP_BG })
        .corner_radius(CornerRadius::same(20));
    ui.add(button)
}

/// The big amber call-to-action.
pub fn action_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).font(FontId::new(20.0, art::heading_family())).color(BG))
        .fill(if enabled { AMBER } else { LINE })
        .corner_radius(CornerRadius::same(10))
        .min_size(egui::vec2(180.0, 44.0));
    ui.add_enabled(enabled, button)
}
