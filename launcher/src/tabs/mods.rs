//! Mods: health check with one-click fixes, contested files, and every mod in
//! the order the game will load it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui;
use egui::{FontId, RichText};

use super::{accent_button, level_icon, tag};
use crate::loadorder::{self, Level, SOFT_DEPENDENCIES};
use crate::mods::ModKind;
use crate::theme::*;
use crate::{Action, App, art};

impl App {
    pub(crate) fn mods_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            heading(ui, "Mods");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open mods folder").clicked() {
                    if let Some(p) = &self.paths {
                        actions.push(Action::OpenPath(p.mods()));
                    }
                }
                if ui.add_enabled(self.task.is_none(), egui::Button::new("Check for updates")).clicked() {
                    actions.push(Action::CheckUpdates);
                }
                ui.checkbox(&mut self.show_debug_mods, "Show test mods");
            });
        });
        if self.running {
            ui.label(RichText::new("Close the game to change mods; it rewrites its settings when it exits.").color(AMBER));
        }
        let Some(settings) = &self.settings else {
            ui.label(RichText::new(self.settings_error.clone().unwrap_or_else(|| "Game not found.".into())).color(BAD));
            return;
        };
        let enabled: Vec<bool> = self.mods.iter().map(|m| m.is_enabled(settings)).collect();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.health_card(ui, actions);
            ui.add_space(4.0);
            if !self.analysis.conflicts.is_empty() {
                self.conflicts_card(ui, actions);
                ui.add_space(4.0);
            }

            // conflict counts per mod, for the row tags
            let mut shared: HashMap<usize, usize> = HashMap::new();
            for c in &self.analysis.conflicts {
                for &i in &c.mods {
                    *shared.entry(i).or_default() += 1;
                }
            }
            let installed: HashSet<String> = self.mods.iter().map(|m| m.namespace.clone()).collect();
            let on: Vec<usize> = self.analysis.order.clone();
            let mut off: Vec<usize> = (0..self.mods.len())
                .filter(|i| !on.contains(i))
                .filter(|&i| self.show_debug_mods || !self.mods[i].debug)
                .filter(|&i| !loadorder::is_generated(&self.mods[i].path))
                .collect();
            off.sort_by_key(|&i| self.mods[i].name.to_lowercase());
            let generated = (0..self.mods.len()).find(|&i| loadorder::is_generated(&self.mods[i].path));

            section_title(ui, &format!("Load order ({} on)", on.len()));
            ui.label(RichText::new("The game loads these top to bottom; later mods win when two change the same thing.").color(DIM).small());
            for (pos, &i) in on.iter().enumerate() {
                self.mod_row(ui, i, Some(pos + 1), enabled[i], &installed, shared.get(&i).copied(), actions);
            }
            if let Some(g) = generated.filter(|&g| enabled[g]) {
                self.mod_row(ui, g, Some(on.len() + 1), true, &installed, None, actions);
            }
            if !off.is_empty() {
                ui.add_space(8.0);
                section_title(ui, &format!("Switched off ({})", off.len()));
                for &i in &off {
                    self.mod_row(ui, i, None, enabled[i], &installed, None, actions);
                }
            }
        });
    }

    fn health_card(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                card_title(ui, "Health check");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut auto = self.cfg.auto_fix;
                    if ui.checkbox(&mut auto, "Fix automatically when I press Play").changed() {
                        self.cfg.auto_fix = auto;
                        actions.push(Action::SaveConfig);
                    }
                });
            });
            let issues = self.analysis.issues.clone();
            if issues.is_empty() {
                let settled = self.analysis.conflicts.len();
                let extra = if settled > 0 { format!(" {settled} shared files are settled by load order.") } else { String::new() };
                ui.label(RichText::new(format!("✔  Everything loads cleanly.{extra}")).color(GOOD));
                return;
            }
            for issue in &issues {
                ui.horizontal(|ui| {
                    let (dot, color) = level_icon(issue.level);
                    ui.label(RichText::new(dot).color(color));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&issue.title).font(FontId::new(15.5, art::medium_family())));
                        ui.label(RichText::new(&issue.detail).color(DIM).small());
                    });
                    if let Some((label, fix)) = &issue.fix {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add_enabled(!self.running, egui::Button::new(label)).clicked() {
                                actions.push(Action::ApplyFixes(vec![(label.clone(), fix.clone())]));
                            }
                        });
                    }
                });
            }
            let all: Vec<_> = issues.iter().filter_map(|i| i.fix.clone()).collect();
            if all.len() > 1 {
                ui.add_space(4.0);
                if accent_button(ui, "Fix everything", !self.running) {
                    actions.push(Action::ApplyFixes(all));
                }
            }
            if issues.iter().any(|i| i.level == Level::Problem && i.fix.is_none()) {
                ui.label(RichText::new("Problems without a button need a choice from you: switch the mod off below or update it.").color(DIM).small());
            }
        });
    }

    fn conflicts_card(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let unsettled = self.analysis.conflicts.iter().filter(|c| !c.settled).count();
        let title = if unsettled > 0 {
            format!("Shared files ({} contested, {} settled)", unsettled, self.analysis.conflicts.len() - unsettled)
        } else {
            format!("Shared files ({} settled)", self.analysis.conflicts.len())
        };
        card(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new(title).font(FontId::new(16.0, art::bold_family())).color(AMBER))
                .id_salt("conflicts")
                .default_open(unsettled > 0)
                .show(ui, |ui| {
                    ui.label(RichText::new("When two mods replace the same file only one copy is used. Settled ones follow the load order; contested ones are pinned by ReHearth Load Order, and you can change the pick.").color(DIM).small());
                    for c in self.analysis.conflicts.clone() {
                        ui.separator();
                        ui.label(RichText::new(loadorder::pretty_target(&c.target)).monospace().size(13.0));
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Replaced by").color(DIM).small());
                            for &i in &c.mods {
                                let name = &self.mods[i].name;
                                if i == c.winner {
                                    tag(ui, &format!("{name} ✔"), GOOD);
                                } else {
                                    tag(ui, name, DIM);
                                }
                            }
                        });
                        if c.settled {
                            ui.label(RichText::new(format!("{} loads after the others, so its copy is used.", self.mods[c.winner].name)).color(DIM).small());
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Use the copy from").small());
                                let mut pick = self.mods[c.winner].namespace.clone();
                                egui::ComboBox::from_id_salt(("winner", &c.target))
                                    .selected_text(self.mods[c.winner].name.clone())
                                    .show_ui(ui, |ui| {
                                        for &i in &c.mods {
                                            ui.selectable_value(&mut pick, self.mods[i].namespace.clone(), &self.mods[i].name);
                                        }
                                    });
                                if pick != self.mods[c.winner].namespace {
                                    actions.push(Action::PinWinner(c.target.clone(), pick));
                                }
                            });
                        }
                    }
                });
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn mod_row(
        &mut self,
        ui: &mut egui::Ui,
        i: usize,
        position: Option<usize>,
        enabled: bool,
        installed: &HashSet<String>,
        shared: Option<usize>,
        actions: &mut Vec<Action>,
    ) {
        let m = self.mods[i].clone();
        let generated = loadorder::is_generated(&m.path);
        let workshop_id: Option<u64> = m.workshop_id.as_deref().and_then(|id| id.parse().ok());
        egui::Frame::NONE
            .fill(CARD)
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut on = enabled;
                    let can_toggle = !m.required() && !self.running && !generated;
                    if ui.add_enabled(can_toggle, egui::Checkbox::without_text(&mut on)).changed() {
                        actions.push(Action::ToggleMod(i, on));
                    }
                    let num = position.map(|p| format!("{p:>2}")).unwrap_or_else(|| "  ".into());
                    ui.label(RichText::new(num).monospace().color(DIM));
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&m.name).font(FontId::new(15.5, art::medium_family())));
                            tag(ui, m.kind.label(), DIM);
                            if let Some(v) = &m.mod_version {
                                tag(ui, &format!("v{v}"), DIM);
                            }
                            if m.required() {
                                tag(ui, "required", DIM);
                            }
                            if m.client_only {
                                tag(ui, "interface only", DIM);
                            }
                            if generated {
                                tag(ui, "generated by ReHearth", AMBER);
                            } else if m.managed.is_some() {
                                tag(ui, "installed by ReHearth", AMBER);
                            }
                            if self.updates.contains(&m.namespace) {
                                tag(ui, "update available", GOOD);
                            }
                            if let Some(n) = shared {
                                tag(ui, &format!("{n} shared file{}", if n == 1 { "" } else { "s" }), AMBER);
                            }
                            if m.manifest_error.is_some() {
                                tag(ui, "can't be read", BAD);
                            }
                        });
                        if let Some(err) = &m.manifest_error {
                            ui.label(RichText::new(err).color(BAD).small());
                        }
                        let missing: Vec<&str> = m
                            .dependencies
                            .iter()
                            .map(String::as_str)
                            .filter(|d| !installed.contains(*d) && !SOFT_DEPENDENCIES.contains(d))
                            .collect();
                        if enabled && !missing.is_empty() {
                            ui.label(RichText::new(format!("Needs {}, which isn't installed", missing.join(", "))).color(BAD).small());
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let removable = !generated && (m.managed.is_some() || (m.kind == ModKind::Workshop && workshop_id.is_some()));
                        if removable {
                            let id = format!("remove:{}:{}", m.namespace, i);
                            if self.confirm.as_deref() == Some(id.as_str()) {
                                if ui.button(RichText::new("Really remove?").color(BAD)).clicked() {
                                    actions.push(Action::RemoveMod(i));
                                    self.confirm = None;
                                }
                            } else if ui.add_enabled(!self.running, egui::Button::new("Remove")).clicked() {
                                self.confirm = Some(id);
                            }
                        }
                        if let Some(id) = workshop_id.filter(|_| m.kind == ModKind::Workshop) {
                            if ui.button("Details").clicked() {
                                actions.push(Action::ShowWorkshopItem(id));
                            }
                        }
                        if ui.button("Folder").clicked() {
                            let dir = if m.path.is_dir() { m.path.clone() } else { m.path.parent().map(PathBuf::from).unwrap_or_default() };
                            actions.push(Action::OpenPath(dir));
                        }
                    });
                });
            });
        ui.add_space(2.0);
    }
}

