//! Saves: towns on disk, one-click backups, and restoring them.

use eframe::egui;
use egui::{FontId, RichText};

use crate::theme::*;
use crate::{Action, App, art, saves};

impl App {
    pub(crate) fn saves_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            heading(ui, "Saves");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open backups folder").clicked() {
                    actions.push(Action::OpenPath(saves::backup_dir()));
                }
                if let Some(p) = &self.paths {
                    if ui.button("Open saves folder").clicked() {
                        actions.push(Action::OpenPath(p.saves()));
                    }
                }
                if ui.add_enabled(self.task.is_none() && !self.saves.is_empty(), egui::Button::new("Back up all saves")).clicked() {
                    actions.push(Action::BackupSaves);
                }
            });
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                card_title(ui, "Towns");
                if self.saves.is_empty() {
                    ui.label(RichText::new("No saves yet.").color(DIM));
                }
                for (s, when) in &self.saves {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&s.title).font(FontId::new(15.5, art::medium_family())));
                        ui.label(RichText::new(&s.detail).color(DIM));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Folder").clicked() {
                                actions.push(Action::OpenPath(s.dir.clone()));
                            }
                            ui.label(RichText::new(when).color(DIM));
                        });
                    });
                }
            });
            ui.add_space(4.0);
            card(ui, |ui| {
                card_title(ui, "Backups");
                if self.backups.is_empty() {
                    ui.label(RichText::new("No backups yet.").color(DIM));
                }
                for b in &self.backups {
                    ui.horizontal(|ui| {
                        ui.label(&b.name);
                        ui.label(RichText::new(format!("{:.1} MB", b.size_mb)).color(DIM));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let id = format!("restore:{}", b.name);
                            if self.confirm.as_deref() == Some(id.as_str()) {
                                if ui.button(RichText::new("Replace current saves?").color(BAD)).clicked() {
                                    actions.push(Action::RestoreBackup(b.path.clone()));
                                    self.confirm = None;
                                }
                            } else if ui.add_enabled(!self.running && self.task.is_none(), egui::Button::new("Restore")).clicked() {
                                self.confirm = Some(id);
                            }
                        });
                    });
                }
            });
        });
    }
}
