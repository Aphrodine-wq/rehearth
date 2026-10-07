//! Settings: the launcher's own options and the game's graphics settings.

use eframe::egui;
use egui::RichText;
use serde_json::Value;

use crate::game::UserSettings;
use crate::theme::*;
use crate::{Action, App};

pub(crate) fn toggle(ui: &mut egui::Ui, s: &UserSettings, key: &str, label: &str, default: bool, actions: &mut Vec<Action>) {
    let mut on = s.get_bool(key).unwrap_or(default);
    if ui.checkbox(&mut on, label).changed() {
        actions.push(Action::SetSetting(key.to_string(), Value::Bool(on)));
    }
}

impl App {
    pub(crate) fn settings_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        heading(ui, "Settings");
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                card_title(ui, "Launcher");
                let mut changed = false;
                changed |= ui.checkbox(&mut self.cfg.auto_fix, "Fix mod problems automatically when I press Play").changed();
                ui.label(RichText::new("Switches on mods your mods need, turns off duplicate or broken copies, and pins contested files. It never deletes anything.").color(DIM).small());
                changed |= ui.checkbox(&mut self.cfg.close_on_launch, "Close ReHearth when the game starts").changed();
                changed |= ui.checkbox(&mut self.cfg.check_updates, "Check for ReHearth updates when it starts").changed();
                if changed {
                    actions.push(Action::SaveConfig);
                }
                ui.add_space(6.0);
                ui.label("Game folder (leave empty to find it automatically)");
                ui.add(egui::TextEdit::singleline(&mut self.game_dir_edit).desired_width(f32::INFINITY));
                ui.label("Extra launch arguments (passed to the game, e.g. --some.config.key=value)");
                ui.add(egui::TextEdit::singleline(&mut self.cfg.extra_args).desired_width(f32::INFINITY));
                if ui.button("Save").clicked() {
                    actions.push(Action::SaveConfig);
                }
                if !crate::update::in_app_menu() {
                    ui.add_space(6.0);
                    if ui.button("Add ReHearth to the app menu").clicked() {
                        actions.push(Action::AddToAppMenu);
                    }
                }
            });
            ui.add_space(4.0);
            if let Some(s) = &self.settings {
                card(ui, |ui| {
                    card_title(ui, "Graphics");
                    ui.add_enabled_ui(!self.running, |ui| {
                        toggle(ui, s, "renderer.enable_fullscreen", "Fullscreen", false, actions);
                        toggle(ui, s, "renderer.enable_vsync", "VSync", false, actions);
                        toggle(ui, s, "renderer.enable_shadows", "Shadows", true, actions);
                        toggle(ui, s, "renderer.enable_ssao", "Ambient occlusion (SSAO)", false, actions);
                        toggle(ui, s, "renderer.use_high_quality", "High quality renderer", true, actions);
                        let mut dist = s.get_i64("renderer.draw_distance").unwrap_or(750);
                        let slider = egui::Slider::new(&mut dist, 300..=1000).text("Draw distance");
                        if ui.add(slider).drag_stopped() {
                            actions.push(Action::SetSetting("renderer.draw_distance".into(), Value::from(dist)));
                        }
                    });
                    ui.label(RichText::new("Written to the game's user_settings.json. ReHearth keeps a copy of your original.").color(DIM).small());
                    if ui.button("Restore original user_settings.json").clicked() {
                        actions.push(Action::RestoreSettingsBackup);
                    }
                });
            }
        });
    }
}
