//! Doctor: what went wrong last session, who caused it, what it means, and
//! the tools to dig further (live log, safe mode, verbose logging, and the
//! "find the problem mod" helper).

use std::path::PathBuf;
use std::process::Command;

use eframe::egui;
use egui::{FontId, RichText};

use super::{accent_button, tag};
use crate::bisect::Target;
use crate::doctor::{self, ErrorKind, ScriptError};
use crate::mods::ModKind;
use crate::theme::*;
use crate::{Action, App, LaunchMode, art};

/// Debug-level logging for a mod (the guide's levels: 1 error, 3 warning, 5 info, 7 debug).
const VERBOSE_LEVEL: i64 = 7;

impl App {
    pub(crate) fn doctor_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            heading(ui, "Doctor");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(p) = &self.paths {
                    if ui.button("Open log").clicked() {
                        actions.push(Action::OpenPath(p.log()));
                    }
                }
                if ui.button("Copy report").clicked() {
                    actions.push(Action::Copy(doctor::to_text(&self.report), "the session report"));
                }
                if ui.button("Refresh").clicked() {
                    actions.push(Action::Refresh);
                }
            });
        });

        egui::ScrollArea::vertical().id_salt("doctor").auto_shrink([false, false]).show(ui, |ui| {
            if self.cfg.bisect.is_some() {
                self.bisect_card(ui, actions);
                ui.add_space(4.0);
            }
            if self.running {
                self.live_card(ui);
                ui.add_space(4.0);
            }
            if !self.report.found {
                card(ui, |ui| {
                    ui.label(RichText::new("No stonehearth.log yet. Play once, then check back.").color(DIM));
                });
            } else {
                self.session_card(ui, actions);
                ui.add_space(4.0);
                self.errors_section(ui, actions);
                if !self.report.noisy.is_empty() {
                    ui.add_space(4.0);
                    self.noise_card(ui);
                }
            }
            ui.add_space(4.0);
            self.tools_card(ui, actions);
        });
    }

    fn session_card(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let r = &self.report;
        card(ui, |ui| {
            card_title(ui, "Last session");
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "Stonehearth {} · {} → {}",
                    r.version.as_deref().unwrap_or("?").trim_end_matches(" (x64)"),
                    r.started.as_deref().unwrap_or("?"),
                    r.ended.as_deref().map(|e| e.get(11..).unwrap_or(e)).unwrap_or("?")
                ));
                if let Some(gpu) = &r.gpu {
                    tag(ui, gpu, DIM);
                }
            });
            let crashed = !r.clean_exit && !self.running;
            if r.clean_exit {
                ui.label(RichText::new("✔  Closed normally").color(GOOD));
            } else if self.running {
                ui.label(RichText::new("Running now; this updates when it closes.").color(AMBER));
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("✖  The game didn't close normally: it crashed, froze, or was killed.").color(BAD));
                    if self.cfg.bisect.is_none() && ui.button("Find which mod causes it").clicked() {
                        actions.push(Action::StartBisect(Target::Crash));
                    }
                });
            }
            match &r.patch_loaded {
                Some(v) => ui.label(RichText::new(format!("✔  ReHearth Patch {v} was active · duplicate item searches: {}", r.duplicate_think_outputs)).color(DIM)),
                None => ui.label(RichText::new(format!("ReHearth Patch wasn't active · duplicate item searches: {}", r.duplicate_think_outputs)).color(DIM)),
            };
            if crashed && !r.tail.is_empty() {
                egui::CollapsingHeader::new("What the game was doing last")
                    .id_salt("tail")
                    .default_open(true)
                    .show(ui, |ui| {
                        let text: String = r.tail.iter().rev().take(25).rev().map(|l| short_line(l)).collect::<Vec<_>>().join("\n");
                        log_box(ui, &text, 220.0);
                    });
            }
        });
    }

    fn errors_section(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let errors = self.report.errors.clone();
        if errors.is_empty() {
            card(ui, |ui| {
                ui.label(RichText::new("✔  No errors last session.").color(GOOD));
            });
            return;
        }
        let total: usize = errors.iter().map(|e| e.count).sum();
        section_title(ui, &format!("Errors ({} kind{}, {total} in total)", errors.len(), if errors.len() == 1 { "" } else { "s" }));
        for (n, e) in errors.iter().enumerate() {
            self.error_card(ui, n, e, actions);
            ui.add_space(3.0);
        }
    }

    /// Who's most likely behind an error: a mod in the traceback, or else the
    /// mod that replaced the file the error happened in.
    fn blame(&self, e: &ScriptError) -> Option<(String, String)> {
        if let Some(ns) = e.mods.first() {
            return Some((ns.clone(), "its code is in the traceback".into()));
        }
        let file = e.file()?;
        let winner = self.analysis.conflicts.iter().find(|c| c.target == file).map(|c| self.mods[c.winner].namespace.clone());
        winner
            .or_else(|| {
                self.analysis
                    .order
                    .iter()
                    .rev()
                    .map(|&i| &self.mods[i])
                    .find(|m| m.overrides.iter().any(|(t, _)| t == file))
                    .map(|m| m.namespace.clone())
            })
            .map(|ns| (ns, format!("it replaces {file}")))
    }

    fn error_card(&mut self, ui: &mut egui::Ui, n: usize, e: &ScriptError, actions: &mut Vec<Action>) {
        let blame = self.blame(e);
        // open the blamed mod's own line when the traceback has one, else where it failed
        let open_at = e.mod_location.clone().or_else(|| e.location.clone());
        let source = open_at.as_deref().and_then(|l| self.source_file(l));
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let color = match e.kind {
                    ErrorKind::Script => BAD,
                    ErrorKind::Ui => AMBER,
                    ErrorKind::Loading => BAD,
                };
                tag(ui, e.kind.label(), color);
                ui.label(RichText::new(format!("×{}", e.count)).font(FontId::new(15.0, art::bold_family())).color(AMBER));
                ui.label(RichText::new(format!("first at {} · {}", e.first_seen, if e.side == "client" { "interface" } else { "game logic" })).color(DIM).small());
            });
            ui.label(RichText::new(&e.message).font(FontId::new(15.0, art::medium_family())));
            ui.label(RichText::new(doctor::explain(e)).color(DIM));
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                match &blame {
                    Some((ns, why)) => {
                        let name = self.display_name(ns);
                        ui.label(RichText::new(format!("Likely from {name}")).color(TEXT));
                        ui.label(RichText::new(format!("({why})")).color(DIM).small());
                        if let Some(i) = self.mod_index(ns) {
                            let m = &self.mods[i];
                            if !m.required() && m.kind != ModKind::Base && ui.add_enabled(!self.running, egui::Button::new(format!("Switch {name} off"))).clicked() {
                                actions.push(Action::ToggleMod(i, false));
                            }
                        }
                    }
                    None => {
                        ui.label(RichText::new("In the game's own code; a mod's data may still have triggered it.").color(DIM));
                    }
                }
                if self.cfg.bisect.is_none() && ui.button("Find which mod causes it").clicked() {
                    actions.push(Action::StartBisect(Target::Error { message: e.message.clone(), key: doctor::error_key(e) }));
                }
            });
            ui.horizontal(|ui| {
                if ui.small_button("Copy").clicked() {
                    actions.push(Action::Copy(doctor::error_text(e), "the error"));
                }
                if let (Some(path), Some(loc)) = (&source, &open_at) {
                    if ui.small_button(format!("Open {}", loc.rsplit('/').next().unwrap_or(loc))).clicked() {
                        actions.push(Action::OpenPath(path.clone()));
                    }
                }
            });
            if !e.traceback.is_empty() {
                egui::CollapsingHeader::new(RichText::new("Traceback").small()).id_salt(("tb", n)).show(ui, |ui| {
                    log_box(ui, &e.traceback.join("\n"), 180.0);
                });
            }
        });
    }

    /// A local copy of a mod file named in an error ("ns/path.lua:12"),
    /// extracted from the .smod when the mod is zipped.
    fn source_file(&self, location: &str) -> Option<PathBuf> {
        let file = location.split(':').next()?;
        let (ns, rest) = file.split_once('/')?;
        let m = self.mods.iter().find(|m| m.namespace == ns)?;
        if m.path.is_dir() {
            let p = m.path.join(rest);
            return p.is_file().then_some(p);
        }
        // zipped: files live under "<root>/<rest>" inside the .smod
        let cache = PathBuf::from(std::env::var_os("HOME")?).join(".cache/rehearth/source").join(file);
        if cache.is_file() {
            return Some(cache);
        }
        let out = Command::new("unzip").arg("-p").arg(&m.path).arg(format!("*/{rest}")).output().ok()?;
        if !out.status.success() || out.stdout.is_empty() {
            return None;
        }
        std::fs::create_dir_all(cache.parent()?).ok()?;
        std::fs::write(&cache, out.stdout).ok()?;
        Some(cache)
    }

    fn noise_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("Most repeated warnings").font(FontId::new(16.0, art::bold_family())).color(AMBER))
                .id_salt("noise")
                .show(ui, |ui| {
                    ui.label(RichText::new("Not errors, but something firing thousands of times can mean wasted work and lag.").color(DIM).small());
                    for (cat, msg, count) in &self.report.noisy {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("{count}×")).color(AMBER).monospace());
                            tag(ui, cat, DIM);
                            ui.label(RichText::new(msg).small());
                        });
                    }
                });
        });
    }

    fn live_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                card_title(ui, "Live log");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.live_errors_only, "Errors and warnings only");
                });
            });
            let kept: Vec<&String> = self
                .live_log
                .iter()
                .filter(|l| {
                    !self.live_errors_only || {
                        let level = l.splitn(4, '|').nth(2).map(str::trim).unwrap_or("9");
                        level == "0" || level == "1" || l.contains("Error")
                    }
                })
                .collect();
            let lines: Vec<String> = kept[kept.len().saturating_sub(200)..].iter().map(|l| short_line(l)).collect();
            let text = if lines.is_empty() { "Waiting for the game to write its log…".to_string() } else { lines.join("\n") };
            egui::ScrollArea::vertical().id_salt("live").max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0).color(TEXT)).wrap());
            });
        });
    }

    fn bisect_card(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(b) = self.cfg.bisect.clone() else { return };
        let name = |ns: &String| self.display_name(ns);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                card_title(ui, "Finding the problem mod");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Stop").clicked() {
                        actions.push(Action::StopBisect);
                    }
                });
            });
            ui.label(RichText::new(format!("Looking for {}.", b.target.describe())).color(DIM));
            if let Some(found) = &b.found {
                match found.as_slice() {
                    [one] => {
                        ui.label(RichText::new(format!("Found it: {}", name(one))).font(FontId::new(18.0, art::bold_family())).color(AMBER));
                        ui.label("With every other mod on, the problem only shows up when this one is on too. Switch it off, look for an update, or report it to its author with the copied error.");
                        if let Some(i) = self.mod_index(one) {
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!self.running, egui::Button::new(format!("Switch {} off", name(one)))).clicked() {
                                    actions.push(Action::ToggleMod(i, false));
                                    actions.push(Action::StopBisect);
                                }
                                if ui.button("Done").clicked() {
                                    actions.push(Action::StopBisect);
                                }
                            });
                        }
                    }
                    [] => {
                        ui.label("Every mod was cleared, so the problem isn't caused by one mod on its own. It may be the base game, or two mods together.");
                        if ui.button("Done").clicked() {
                            actions.push(Action::StopBisect);
                        }
                    }
                    many => {
                        ui.label(format!("It's one of: {}.", many.iter().map(name).collect::<Vec<_>>().join(", ")));
                        if ui.button("Done").clicked() {
                            actions.push(Action::StopBisect);
                        }
                    }
                }
                return;
            }
            ui.label(format!(
                "Round {} of about {}: {} suspects left.",
                b.round,
                b.round + b.rounds_left().saturating_sub(1),
                b.suspects.len()
            ));
            if let Some(seen) = b.seen {
                let verdict = if seen { "The problem showed up again in that session." } else { "The problem did not show up in that session." };
                ui.label(RichText::new(verdict).font(FontId::new(15.5, art::medium_family())));
                ui.label(RichText::new("If that's not right (say you didn't play long enough), pick the other answer.").color(DIM).small());
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(if seen { "✔ Yes, it happened" } else { "It happened" })).clicked() {
                        actions.push(Action::BisectAnswer(true));
                    }
                    if ui.add(egui::Button::new(if seen { "It didn't happen" } else { "✔ No, it didn't happen" })).clicked() {
                        actions.push(Action::BisectAnswer(false));
                    }
                });
                return;
            }
            if b.launched {
                ui.label("A test session is running. Play until the problem would normally appear, then quit the game.");
                return;
            }
            ui.label(format!("Next test keeps on: {}", b.testing.iter().map(name).collect::<Vec<_>>().join(", ")));
            ui.label(RichText::new(format!("Switched off for that session only: {}", b.off().iter().map(|s| self.display_name(s)).collect::<Vec<_>>().join(", "))).color(DIM).small());
            ui.label(RichText::new("Your own mod settings don't change; only the test launch skips those mods. Use a save you don't mind, since mods being off can upset it.").color(DIM).small());
            if accent_button(ui, "Start test session", !self.running && self.paths.is_some()) {
                actions.push(Action::Launch(LaunchMode::Test));
            }
        });
    }

    fn tools_card(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        card(ui, |ui| {
            card_title(ui, "Debugging tools");
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.running && self.paths.is_some(), egui::Button::new("Play in safe mode")).clicked() {
                    actions.push(Action::Launch(LaunchMode::Safe));
                }
                ui.label(RichText::new("Base game only, for one session. If a problem goes away, a mod causes it.").color(DIM).small());
            });
            ui.separator();
            ui.label(RichText::new("Detailed logging").font(FontId::new(15.0, art::medium_family())));
            ui.label(RichText::new("Makes a mod write everything it does to the log next session. Useful for bug reports; turn it off afterwards, it slows things down.").color(DIM).small());
            let Some(settings) = &self.settings else { return };
            let verbose: Vec<String> = settings
                .get("logging.mods")
                .and_then(|v| v.as_object())
                .map(|o| o.iter().filter(|(_, v)| v.get("log_level").is_some()).map(|(k, _)| k.clone()).collect())
                .unwrap_or_default();
            for ns in &verbose {
                ui.horizontal(|ui| {
                    ui.label(format!("• {}", self.display_name(ns)));
                    if ui.small_button("Turn off").clicked() {
                        actions.push(Action::RemoveSetting(format!("logging.mods.{ns}.log_level")));
                    }
                });
            }
            let choices: Vec<(String, String)> = self
                .analysis
                .order
                .iter()
                .map(|&i| (self.mods[i].namespace.clone(), self.mods[i].name.clone()))
                .filter(|(ns, _)| !verbose.contains(ns))
                .collect();
            ui.horizontal(|ui| {
                let shown = choices.iter().find(|(ns, _)| *ns == self.verbose_pick).map(|(_, n)| n.clone()).unwrap_or_else(|| "Pick a mod…".into());
                egui::ComboBox::from_id_salt("verbose").selected_text(shown).width(240.0).show_ui(ui, |ui| {
                    for (ns, name) in &choices {
                        ui.selectable_value(&mut self.verbose_pick, ns.clone(), name);
                    }
                });
                let ok = choices.iter().any(|(ns, _)| *ns == self.verbose_pick);
                if ui.add_enabled(ok && !self.running, egui::Button::new("Turn on")).clicked() {
                    actions.push(Action::SetSetting(format!("logging.mods.{}.log_level", self.verbose_pick), VERBOSE_LEVEL.into()));
                    self.verbose_pick.clear();
                }
            });
        });
    }
}

/// "2026-10-06 21:17:11.160171 |  server |  1 |   app | text" -> "21:17:11 server  app: text"
fn short_line(line: &str) -> String {
    let parts: Vec<&str> = line.splitn(5, '|').map(str::trim).collect();
    match parts.as_slice() {
        [stamp, side, _level, cat, msg] => format!("{} {:<6} {}: {}", stamp.get(11..19).unwrap_or(stamp), side, cat, msg),
        _ => line.trim_end().to_string(),
    }
}

fn log_box(ui: &mut egui::Ui, text: &str, height: f32) {
    egui::Frame::NONE
        .fill(egui::Color32::from_black_alpha(90))
        .corner_radius(6)
        .inner_margin(8)
        .show(ui, |ui| {
            egui::ScrollArea::both().max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).extend());
            });
        });
}
