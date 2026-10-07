//! The screens behind each menu entry.

mod doctor;
mod home;
mod mods;
mod saves;
mod settings;

use eframe::egui;
use egui::{Color32, RichText};

use crate::loadorder::Level;
use crate::theme::*;

/// A small coloured tag, e.g. "Workshop" or "3 shared files".
pub(crate) fn tag(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(RichText::new(format!(" {text} ")).small().color(color).background_color(color.gamma_multiply(0.16)));
}

pub(crate) fn level_icon(level: Level) -> (&'static str, Color32) {
    match level {
        Level::Problem => ("●", BAD),
        Level::Warning => ("●", AMBER),
        Level::Note => ("●", DIM),
    }
}

/// A plain button with an accent fill, for the main action on a card.
pub(crate) fn accent_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let button = egui::Button::new(RichText::new(text).color(BG).font(egui::FontId::new(15.0, crate::art::medium_family())))
        .fill(if enabled { AMBER } else { LINE });
    ui.add_enabled(enabled, button).clicked()
}
