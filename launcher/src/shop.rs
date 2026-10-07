//! The Workshop tab: browse, search and install Stonehearth Workshop mods
//! without leaving the launcher.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;
use egui::{Align2, Color32, ColorImage, CornerRadius, FontId, Rect, RichText, Sense, Stroke, TextureHandle, Vec2, pos2, vec2};

use crate::art;
use crate::steam::{self, Event};
use crate::theme::*;
use crate::workshop::{self, Item, Page, Sort};

const THUMB: u32 = 384;
const CARD_W: f32 = 188.0;
const CARD_H: f32 = 262.0;

pub enum Thumb {
    Loading,
    Ready(TextureHandle),
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Install,
    Remove,
}

#[derive(Clone)]
pub struct Job {
    pub id: u64,
    pub title: String,
    pub kind: JobKind,
}

struct Active {
    job: Job,
    rx: Receiver<Event>,
    progress: Option<(u64, u64)>,
}

/// What the tab wants the rest of the app to do after a frame.
pub enum ShopAction {
    InstallPatch,
    InstallAceFromGit,
}

pub struct Shop {
    pub search: String,
    sort: Sort,
    page: u32,
    result: Option<Page>,
    loading: Option<(u64, Receiver<Result<Page, String>>)>,
    generation: u64,
    error: Option<String>,

    thumbs: HashMap<u64, Thumb>,
    thumb_tx: Sender<(u64, String)>,
    thumb_rx: Receiver<(u64, Option<ColorImage>)>,

    /// Everything we've seen, so required items and featured cards have titles.
    known: HashMap<u64, Item>,
    descriptions: HashMap<u64, String>,
    details_rx: Vec<Receiver<Vec<(Item, String)>>>,
    details_requested: HashSet<u64>,

    pub open: Option<u64>,
    queue: VecDeque<Job>,
    active: Option<Active>,
    /// Set when something was installed or removed, so the app rescans mods.
    pub changed: bool,
    /// Jobs that ended since the app last looked, and whether they worked.
    pub finished: Vec<(Job, bool)>,
    pub message: Option<(String, bool)>,
    confirm_remove: Option<u64>,
}

impl Shop {
    pub fn new(ctx: &egui::Context) -> Self {
        let (thumb_tx, worker_rx) = channel::<(u64, String)>();
        let (result_tx, thumb_rx) = channel();
        let worker_rx = Arc::new(Mutex::new(worker_rx));
        for _ in 0..4 {
            let rx = Arc::clone(&worker_rx);
            let tx = result_tx.clone();
            let ctx = ctx.clone();
            thread::spawn(move || {
                loop {
                    let next = rx.lock().ok().and_then(|r| r.recv().ok());
                    let Some((id, url)) = next else { break };
                    let image = workshop::thumbnail(id, &url, THUMB).ok().and_then(|bytes| {
                        let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
                        Some(ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
                    });
                    if tx.send((id, image)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
        let mut shop = Self {
            search: String::new(),
            sort: Sort::Trending,
            page: 1,
            result: None,
            loading: None,
            generation: 0,
            error: None,
            thumbs: HashMap::new(),
            thumb_tx,
            thumb_rx,
            known: HashMap::new(),
            descriptions: HashMap::new(),
            details_rx: Vec::new(),
            details_requested: HashSet::new(),
            open: None,
            queue: VecDeque::new(),
            active: None,
            changed: false,
            finished: Vec::new(),
            message: None,
            confirm_remove: None,
        };
        shop.request_details(ctx, &FEATURED_WORKSHOP);
        shop
    }

    fn reload(&mut self, ctx: &egui::Context) {
        self.generation += 1;
        let (tx, rx) = channel();
        let (sort, search, page, ctx) = (self.sort, self.search.clone(), self.page, ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(workshop::browse(sort, &search, page).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.loading = Some((self.generation, rx));
        self.error = None;
    }

    fn request_details(&mut self, ctx: &egui::Context, ids: &[u64]) {
        let ids: Vec<u64> = ids.iter().copied().filter(|id| self.details_requested.insert(*id)).collect();
        if ids.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(workshop::details(&ids).unwrap_or_default());
            ctx.request_repaint();
        });
        self.details_rx.push(rx);
    }

    fn thumb(&mut self, item: &Item) -> Option<&TextureHandle> {
        if item.preview_url.is_empty() {
            // details may still be on the way; ask again once they bring a URL
            return None;
        }
        let state = self.thumbs.entry(item.id).or_insert_with(|| {
            let _ = self.thumb_tx.send((item.id, item.preview_url.clone()));
            Thumb::Loading
        });
        match state {
            Thumb::Ready(t) => Some(t),
            _ => None,
        }
    }

    pub fn busy_with(&self, id: u64) -> Option<(JobKind, Option<(u64, u64)>)> {
        if let Some(a) = &self.active {
            if a.job.id == id {
                return Some((a.job.kind, a.progress));
            }
        }
        self.queue.iter().find(|j| j.id == id).map(|j| (j.kind, None))
    }

    pub fn downloading(&self) -> Option<(&str, Option<(u64, u64)>, usize)> {
        self.active.as_ref().map(|a| (a.job.title.as_str(), a.progress, self.queue.len()))
    }

    pub fn enqueue(&mut self, job: Job) {
        if self.busy_with(job.id).is_none() {
            self.queue.push_back(job);
        }
    }

    /// Installs an item and any required items that aren't there yet.
    pub fn install(&mut self, id: u64, installed: &HashSet<u64>) {
        let requires = self.known.get(&id).map(|i| i.requires.clone()).unwrap_or_default();
        for dep in requires {
            if !installed.contains(&dep) {
                let title = self.known.get(&dep).map(|i| i.title.clone()).unwrap_or_else(|| format!("Item {dep}"));
                self.enqueue(Job { id: dep, title, kind: JobKind::Install });
            }
        }
        let title = self.known.get(&id).map(|i| i.title.clone()).unwrap_or_else(|| format!("Item {id}"));
        self.enqueue(Job { id, title, kind: JobKind::Install });
    }

    /// Makes an item found elsewhere (e.g. a needed-mod lookup) known here,
    /// so installs get its title and required items.
    pub fn remember(&mut self, item: &Item) {
        self.known.entry(item.id).or_insert_with(|| item.clone());
    }

    /// Opens the Workshop screen's search on `text`.
    pub fn search_for(&mut self, ctx: &egui::Context, text: &str) {
        self.search = text.to_string();
        self.page = 1;
        self.open = None;
        self.reload(ctx);
    }

    pub fn remove(&mut self, id: u64, title: String) {
        self.enqueue(Job { id, title, kind: JobKind::Remove });
    }

    /// Advances background work; call every frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some((generation, rx)) = &self.loading {
            if let Ok(result) = rx.try_recv() {
                if *generation == self.generation {
                    match result {
                        Ok(page) => {
                            for item in &page.items {
                                self.known.insert(item.id, item.clone());
                            }
                            self.result = Some(page);
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                self.loading = None;
            }
        }
        while let Ok((id, image)) = self.thumb_rx.try_recv() {
            let state = match image {
                Some(img) => Thumb::Ready(ctx.load_texture(format!("thumb{id}"), img, egui::TextureOptions::LINEAR)),
                None => Thumb::Missing,
            };
            self.thumbs.insert(id, state);
        }
        self.details_rx.retain(|rx| match rx.try_recv() {
            Ok(list) => {
                for (item, long) in list {
                    self.descriptions.insert(item.id, long);
                    // keep "requires" from the browse data if the details call didn't carry it
                    let requires = self.known.get(&item.id).map(|k| k.requires.clone()).unwrap_or_default();
                    let mut item = item;
                    if item.requires.is_empty() {
                        item.requires = requires;
                    }
                    self.known.insert(item.id, item);
                }
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });

        if self.active.is_none() {
            if let Some(job) = self.queue.pop_front() {
                let cmd = match job.kind {
                    JobKind::Install => "subscribe",
                    JobKind::Remove => "unsubscribe",
                };
                match steam::spawn(cmd, job.id) {
                    Ok(rx) => self.active = Some(Active { job, rx, progress: None }),
                    Err(e) => self.message = Some((format!("{e:#}"), true)),
                }
            }
        }
        if let Some(active) = &mut self.active {
            let mut finished = None;
            while let Ok(event) = active.rx.try_recv() {
                match event {
                    Event::Subscribed => {}
                    Event::Progress(done, total) => active.progress = Some((done, total)),
                    Event::Done => finished = Some(Ok(())),
                    Event::Failed(e) => finished = Some(Err(e)),
                }
            }
            if let Some(result) = finished {
                let job = self.active.take().expect("active job").job;
                self.changed = true;
                self.finished.push((job.clone(), result.is_ok()));
                self.message = Some(match (result, job.kind) {
                    (Ok(()), JobKind::Install) => (format!("Installed {}", job.title), false),
                    (Ok(()), JobKind::Remove) => (format!("Removed {}", job.title), false),
                    (Err(e), _) => (format!("{}: {e}", job.title), true),
                });
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, installed: &HashSet<u64>, has_patch: bool, has_ace: bool, game_running: bool, out: &mut Vec<ShopAction>) {
        let ctx = ui.ctx().clone();
        if self.result.is_none() && self.loading.is_none() && self.error.is_none() {
            self.reload(&ctx);
        }
        if let Some(id) = self.open {
            self.detail_ui(ui, id, installed, has_ace, game_running, out);
            return;
        }

        // search + sort bar
        ui.horizontal(|ui| {
            let hint = match &self.result {
                Some(p) if self.search.is_empty() => format!("Search {} mods…", p.total_count),
                _ => "Search mods…".to_string(),
            };
            let edit = egui::TextEdit::singleline(&mut self.search)
                .hint_text(hint)
                .desired_width(260.0)
                .margin(vec2(10.0, 7.0));
            let response = ui.add(edit);
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.page = 1;
                self.reload(&ctx);
            }
            if !self.search.is_empty() && ui.button("✕").on_hover_text("Clear search").clicked() {
                self.search.clear();
                self.page = 1;
                self.reload(&ctx);
            }
            ui.add_space(8.0);
            for sort in Sort::ALL {
                let selected = self.sort == sort && self.search.is_empty();
                if chip(ui, sort.label(), selected).clicked() {
                    self.sort = sort;
                    self.search.clear();
                    self.page = 1;
                    self.reload(&ctx);
                }
            }
        });
        ui.add_space(6.0);

        let pages = self.result.as_ref().map(|p| (p.page, p.total_pages.max(1)));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.search.is_empty() && self.page == 1 {
                section_title(ui, "Recommended");
                self.featured_ui(ui, installed, has_patch, has_ace, game_running, out);
                ui.add_space(10.0);
                section_title(ui, self.sort.label());
            } else if !self.search.is_empty() {
                section_title(ui, &format!("Results for “{}”", self.search.trim()));
            }

            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(BAD));
                if ui.button("Try again").clicked() {
                    self.reload(&ctx);
                }
            } else if self.loading.is_some() && self.result.is_none() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading the Workshop…").color(DIM));
                });
            }
            let items: Vec<Item> = self.result.as_ref().map(|p| p.items.clone()).unwrap_or_default();
            if items.is_empty() && self.loading.is_none() && self.error.is_none() {
                ui.label(RichText::new("Nothing found.").color(DIM));
            }
            let cols = ((ui.available_width() + 12.0) / (CARD_W + 12.0)).floor().max(1.0) as usize;
            for row in items.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for item in row {
                        if self.card(ui, item, installed).clicked() {
                            self.open = Some(item.id);
                            self.request_details(&ctx, &[item.id]);
                            let deps = item.requires.clone();
                            self.request_details(&ctx, &deps);
                        }
                    }
                });
                ui.add_space(4.0);
            }

            if let Some((page, total)) = pages {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(page > 1 && self.loading.is_none(), egui::Button::new("‹ Previous")).clicked() {
                        self.page = page - 1;
                        self.reload(&ctx);
                    }
                    ui.label(RichText::new(format!("Page {page} of {total}")).color(DIM));
                    if ui.add_enabled(page < total && self.loading.is_none(), egui::Button::new("Next ›")).clicked() {
                        self.page = page + 1;
                        self.reload(&ctx);
                    }
                    if self.loading.is_some() {
                        ui.spinner();
                    }
                });
            }
        });
    }

    fn featured_ui(&mut self, ui: &mut egui::Ui, installed: &HashSet<u64>, has_patch: bool, has_ace: bool, game_running: bool, out: &mut Vec<ShopAction>) {
        let ctx = ui.ctx().clone();
        let width = ((ui.available_width() - 24.0) / 3.0).max(200.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            for id in FEATURED_WORKSHOP {
                let item = self.known.get(&id).cloned().unwrap_or_else(|| Item {
                    id,
                    title: if id == ACE_ID { "Authorized Community Expansion (ACE)".into() } else { "Performance Mod for ACE".into() },
                    ..Default::default()
                });
                let blurb = if id == ACE_ID {
                    "The community continuation of Stonehearth: new jobs, crafting, farming, building tools and hundreds of fixes."
                } else {
                    "Caches storage-filter checks and spreads out garbage collection. Pairs with ReHearth Patch."
                };
                let tex = self.thumb(&item).cloned();
                let status = self.status_of(id, installed);
                if featured_card(ui, width, Some(tex.as_ref()), &item.title, blurb, status).clicked() {
                    self.open = Some(id);
                    let deps = item.requires.clone();
                    self.request_details(&ctx, &deps);
                }
            }
            // ReHearth Patch ships with the launcher rather than the Workshop
            let status = if has_patch { Status::Installed } else { Status::Available };
            let r = featured_card(
                ui,
                width,
                None,
                "ReHearth Patch",
                "Lag fixes for big towns, built into this launcher. Needs ACE. Click to install.",
                status,
            );
            if r.clicked() && !game_running {
                if has_ace {
                    out.push(ShopAction::InstallPatch);
                } else {
                    self.message = Some(("Install ACE first; the patch builds on it.".into(), true));
                }
            }
        });
    }

    fn status_of(&self, id: u64, installed: &HashSet<u64>) -> Status {
        match self.busy_with(id) {
            Some((JobKind::Install, Some((done, total)))) if total > 0 => Status::Downloading(done as f32 / total as f32),
            Some((JobKind::Install, _)) => Status::Queued,
            Some((JobKind::Remove, _)) => Status::Removing,
            None if installed.contains(&id) => Status::Installed,
            None => Status::Available,
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, item: &Item, installed: &HashSet<u64>) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, CARD_H), Sense::click());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let tex = self.thumb(item).cloned();
        let status = self.status_of(item.id, installed);
        let painter = ui.painter_at(rect.expand(2.0));
        let hovered = response.hovered();
        let lift = if hovered { -2.0 } else { 0.0 };
        let rect = rect.translate(vec2(0.0, lift));
        painter.rect(rect, 10, CARD_BG, Stroke::new(1.0, if hovered { AMBER } else { LINE }), egui::StrokeKind::Inside);

        let img_rect = Rect::from_min_size(rect.min + vec2(6.0, 6.0), Vec2::splat(CARD_W - 12.0));
        paint_thumb(&painter, img_rect, tex.as_ref(), &item.title);
        paint_status(&painter, img_rect, status);

        let text_rect = Rect::from_min_max(pos2(rect.left() + 10.0, img_rect.bottom() + 6.0), rect.right_bottom() - vec2(10.0, 8.0));
        let mut job = egui::text::LayoutJob::simple(item.title.clone(), FontId::new(15.0, art::bold_family()), TEXT, text_rect.width());
        job.wrap.max_rows = 2;
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(text_rect.min, galley, TEXT);
        let mut meta = format!("{} subscribers", workshop::human_count(item.subscriptions));
        if let Some(stars) = item.stars {
            meta = format!("{}  ·  {meta}", stars_text(stars));
        }
        painter.text(pos2(text_rect.left(), text_rect.bottom()), Align2::LEFT_BOTTOM, meta, FontId::proportional(12.5), DIM);
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response
    }

    fn detail_ui(&mut self, ui: &mut egui::Ui, id: u64, installed: &HashSet<u64>, has_ace: bool, game_running: bool, out: &mut Vec<ShopAction>) {
        let ctx = ui.ctx().clone();
        if ui.button("‹  Back to the Workshop").clicked() {
            self.open = None;
            self.confirm_remove = None;
            return;
        }
        ui.add_space(6.0);
        let Some(item) = self.known.get(&id).cloned() else {
            self.request_details(&ctx, &[id]);
            ui.spinner();
            return;
        };
        // the browse data has no long description; fetch it once
        self.request_details(&ctx, &[id]);
        let tex = self.thumb(&item).cloned();
        let status = self.status_of(id, installed);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let side = 260.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                paint_thumb(ui.painter(), rect, tex.as_ref(), &item.title);
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&item.title).font(FontId::new(28.0, art::heading_family())).color(TEXT));
                    let mut facts = vec![
                        format!("{} subscribers", workshop::human_count(item.subscriptions)),
                        workshop::human_size(item.file_size),
                    ];
                    if item.time_updated > 0 {
                        facts.push(format!("updated {}", crate::format_date(item.time_updated)));
                    }
                    if let Some(s) = item.stars {
                        facts.push(stars_text(s));
                    }
                    ui.label(RichText::new(facts.join("  ·  ")).color(DIM));
                    if !item.tags.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for t in &item.tags {
                                ui.label(RichText::new(t).small().color(DIM).background_color(CHIP_BG));
                            }
                        });
                    }
                    if !item.requires.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new("Requires").strong());
                        for dep in &item.requires {
                            let name = self.known.get(dep).map(|i| i.title.clone()).unwrap_or_else(|| format!("Item {dep}"));
                            let have = installed.contains(dep);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(if have { "✔" } else { "•" }).color(if have { GOOD } else { AMBER }));
                                if ui.link(name).clicked() {
                                    self.open = Some(*dep);
                                    self.request_details(&ctx, &[*dep]);
                                }
                            });
                        }
                    }
                    ui.add_space(10.0);
                    match status {
                        Status::Installed => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("✔ Installed").color(GOOD).size(18.0));
                                ui.add_space(10.0);
                                if self.confirm_remove == Some(id) {
                                    if ui.button(RichText::new("Really remove?").color(BAD)).clicked() {
                                        self.remove(id, item.title.clone());
                                        self.confirm_remove = None;
                                    }
                                } else if ui.add_enabled(!game_running, egui::Button::new("Remove")).clicked() {
                                    self.confirm_remove = Some(id);
                                }
                            });
                        }
                        Status::Available => {
                            if action_button(ui, "INSTALL", !game_running).clicked() {
                                self.install(id, installed);
                            }
                            if game_running {
                                ui.label(RichText::new("Close the game to install mods.").color(DIM).small());
                            } else if item.requires.iter().any(|d| !installed.contains(d)) {
                                ui.label(RichText::new("Its required items are installed too.").color(DIM).small());
                            }
                        }
                        Status::Downloading(f) => {
                            ui.add(egui::ProgressBar::new(f).desired_width(280.0).show_percentage().fill(AMBER));
                        }
                        Status::Queued => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Waiting for Steam…");
                            });
                        }
                        Status::Removing => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Removing…");
                            });
                        }
                    }
                    if id == ACE_ID && status == Status::Available && !has_ace {
                        ui.add_space(4.0);
                        if ui.small_button("or install the same build from GitHub (no Steam needed)").clicked() {
                            out.push(ShopAction::InstallAceFromGit);
                        }
                    }
                });
            });
            ui.add_space(14.0);
            ui.label(RichText::new("About").font(FontId::new(18.0, art::heading_family())).color(AMBER));
            match self.descriptions.get(&id) {
                Some(d) if !d.is_empty() => {
                    ui.label(RichText::new(d).size(15.0));
                }
                Some(_) => {
                    ui.label(RichText::new(&item.summary).size(15.0));
                }
                None => {
                    ui.spinner();
                }
            }
        });
    }
}

pub const ACE_ID: u64 = 1577375188;
pub const PERF_ID: u64 = 3669297036;
const FEATURED_WORKSHOP: [u64; 2] = [ACE_ID, PERF_ID];

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Available,
    Installed,
    Queued,
    Downloading(f32),
    Removing,
}

fn stars_text(stars: u8) -> String {
    (0..5).map(|i| if i < stars { '★' } else { '☆' }).collect()
}

fn paint_thumb(painter: &egui::Painter, rect: Rect, tex: Option<&TextureHandle>, title: &str) {
    let radius = CornerRadius::same(8);
    match tex {
        Some(t) => {
            painter.add(egui::epaint::RectShape::filled(rect, radius, Color32::BLACK));
            let shape = egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
                .with_texture(t.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
            painter.add(shape);
        }
        None => {
            painter.rect_filled(rect, radius, CHIP_BG);
            let initial: String = title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
            painter.text(rect.center(), Align2::CENTER_CENTER, initial, FontId::new(rect.width() * 0.32, art::heading_family()), LINE);
        }
    }
}

fn paint_status(painter: &egui::Painter, img: Rect, status: Status) {
    let pill = |text: &str, color: Color32| {
        let font = FontId::new(11.5, art::bold_family());
        let galley = painter.layout_no_wrap(text.to_string(), font, Color32::BLACK);
        let size = galley.size() + vec2(14.0, 6.0);
        let r = Rect::from_min_size(pos2(img.right() - size.x - 6.0, img.top() + 6.0), size);
        painter.rect_filled(r, 20, color);
        painter.galley(r.min + vec2(7.0, 3.0), galley, Color32::BLACK);
    };
    match status {
        Status::Installed => pill("INSTALLED", GOOD),
        Status::Queued => pill("QUEUED", AMBER),
        Status::Removing => pill("REMOVING", BAD),
        Status::Downloading(f) => {
            let bar = Rect::from_min_max(pos2(img.left() + 8.0, img.bottom() - 14.0), pos2(img.right() - 8.0, img.bottom() - 8.0));
            painter.rect_filled(bar, 3, Color32::from_black_alpha(180));
            let mut done = bar;
            done.set_right(bar.left() + bar.width() * f.clamp(0.0, 1.0));
            painter.rect_filled(done, 3, AMBER);
            pill(&format!("{:.0}%", f * 100.0), AMBER);
        }
        Status::Available => {}
    }
}

/// `art` is `Some(texture or still loading)` for Workshop items, `None` for the bundled patch.
fn featured_card(ui: &mut egui::Ui, width: f32, art: Option<Option<&TextureHandle>>, title: &str, blurb: &str, status: Status) -> egui::Response {
    let height = 118.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter_at(rect);
    let hovered = response.hovered();
    painter.rect(rect, 12, FEATURE_BG, Stroke::new(if hovered { 2.0 } else { 1.0 }, if hovered { AMBER } else { AMBER_DIM }), egui::StrokeKind::Inside);
    let img = Rect::from_min_size(rect.min + vec2(9.0, 9.0), Vec2::splat(height - 18.0));
    match art {
        Some(tex) => paint_thumb(&painter, img, tex, title),
        None => {
            // the bundled patch gets a hearth-coloured tile instead of a preview
            painter.rect_filled(img, 8, AMBER_DIM);
            painter.text(img.center(), Align2::CENTER_CENTER, "RH", FontId::new(34.0, art::heading_family()), TEXT);
        }
    }
    paint_status(&painter, img, status);
    let x = img.right() + 12.0;
    let w = rect.right() - x - 10.0;
    let mut job = egui::text::LayoutJob::simple(title.to_string(), FontId::new(17.0, art::heading_family()), TEXT, w);
    job.wrap.max_rows = 1;
    let title_galley = ui.fonts_mut(|f| f.layout_job(job));
    let title_h = title_galley.size().y;
    painter.galley(pos2(x, rect.top() + 10.0), title_galley, TEXT);
    let mut job = egui::text::LayoutJob::simple(blurb.to_string(), FontId::proportional(13.5), DIM, w);
    job.wrap.max_rows = 4;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    painter.galley(pos2(x, rect.top() + 14.0 + title_h), galley, DIM);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}
