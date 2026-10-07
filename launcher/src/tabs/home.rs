//! Home: the title art up front, with a column of what matters before playing.

use eframe::egui;
use egui::{RichText, pos2};

use super::{accent_button, level_icon};
use crate::theme::*;
use crate::{Action, App, Tab, shop};

impl App {
    pub(crate) fn home_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let full = ui.max_rect();
        let col_w = 420.0_f32.min(full.width());
        let col = egui::Rect::from_min_max(pos2(full.right() - col_w, full.top()), full.right_bottom());
        ui.scope_builder(egui::UiBuilder::new().max_rect(col), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.home_cards(ui, actions));
        });
    }

    fn home_cards(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let has_ace = self.find_mod("stonehearth_ace").is_some();
        let has_patch = self.find_mod("rehearth_patch").is_some();

        if let Some(b) = &self.cfg.bisect {
            card(ui, |ui| {
                card_title(ui, "Finding the problem mod");
                let text = match (&b.found, b.launched, b.seen) {
                    (Some(_), ..) => "Done. Doctor has the answer.".to_string(),
                    (None, true, None) => "Test session running…".to_string(),
                    (None, _, Some(_)) => "Test finished. Confirm the result in Doctor.".to_string(),
                    (None, false, None) => format!("Round {}: ready for the next test session.", b.round),
                };
                ui.label(text);
                if ui.button("Open Doctor").clicked() {
                    actions.push(Action::Tab(Tab::Doctor));
                }
            });
            ui.add_space(4.0);
        }

        card(ui, |ui| {
            card_title(ui, "Your mods");
            if self.paths.is_none() {
                ui.label(RichText::new("Stonehearth wasn't found. Set its folder in Settings.").color(BAD));
                return;
            }
            let on = self.analysis.order.iter().filter(|&&i| self.mods[i].kind != crate::mods::ModKind::Base).count();
            let issues: Vec<_> = self.analysis.issues.iter().filter(|i| i.level != crate::loadorder::Level::Note).collect();
            if issues.is_empty() {
                let text = match on {
                    0 => "✔  Base game, no mods switched on".to_string(),
                    1 => "✔  1 mod loads cleanly".to_string(),
                    n => format!("✔  {n} mods load cleanly"),
                };
                ui.label(RichText::new(text).color(GOOD));
            } else {
                for issue in issues.iter().take(4) {
                    let (dot, color) = level_icon(issue.level);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(dot).color(color));
                        ui.label(&issue.title);
                    });
                }
                let fixes: Vec<_> = issues.iter().filter_map(|i| i.fix.clone()).collect();
                ui.horizontal(|ui| {
                    if !fixes.is_empty() && accent_button(ui, "Fix all", !self.running) {
                        actions.push(Action::ApplyFixes(fixes));
                    }
                    if ui.button("Details").clicked() {
                        actions.push(Action::Tab(Tab::Mods));
                    }
                });
                if self.cfg.auto_fix {
                    ui.label(RichText::new("Automatic fixes also run when you press Play.").color(DIM).small());
                }
            }
            if !has_ace {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("ACE isn't installed yet.");
                    if ui.button("Get ACE").clicked() {
                        actions.push(Action::ShowWorkshopItem(shop::ACE_ID));
                    }
                });
            } else if !has_patch {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("ReHearth Patch (lag fix) isn't installed.");
                    if ui.add_enabled(!self.running, egui::Button::new("Install")).clicked() {
                        actions.push(Action::InstallPatch);
                    }
                });
            }
        });

        ui.add_space(4.0);
        card(ui, |ui| {
            card_title(ui, "Last session");
            let r = &self.report;
            if !r.found {
                ui.label(RichText::new("No log yet. Play once and the report shows up here.").color(DIM));
                return;
            }
            ui.label(format!("{} → {}", r.started.as_deref().unwrap_or("?"), r.ended.as_deref().map(|e| e.get(11..).unwrap_or(e)).unwrap_or("?")));
            if r.clean_exit {
                ui.label(RichText::new("✔  Closed normally").color(GOOD));
            } else if self.running {
                ui.label(RichText::new("Running now").color(AMBER));
            } else {
                ui.label(RichText::new("✖  Didn't close normally (crash or forced quit)").color(BAD));
            }
            let n = r.errors.len();
            ui.horizontal(|ui| {
                if n == 0 {
                    ui.label(RichText::new("✔  No errors").color(GOOD));
                } else {
                    let total: usize = r.errors.iter().map(|e| e.count).sum();
                    ui.label(RichText::new(format!("{n} kind{} of error ({total} in total)", if n == 1 { "" } else { "s" })).color(AMBER));
                }
                if (n > 0 || !r.clean_exit) && ui.button("Open Doctor").clicked() {
                    actions.push(Action::Tab(Tab::Doctor));
                }
            });
        });

        ui.add_space(4.0);
        if let Some(s) = &self.settings {
            card(ui, |ui| {
                card_title(ui, "Quick settings");
                ui.add_enabled_ui(!self.running, |ui| {
                    crate::tabs::settings::toggle(ui, s, "renderer.enable_fullscreen", "Fullscreen", false, actions);
                    crate::tabs::settings::toggle(ui, s, "renderer.enable_vsync", "VSync", false, actions);
                    crate::tabs::settings::toggle(ui, s, "enable_auto_save", "Autosave", true, actions);
                });
            });
        }
    }
}
