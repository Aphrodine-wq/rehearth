//! The look: Stonehearth's own title-screen art, logo and fonts, read at runtime
//! from the player's install (nothing from the game is shipped with ReHearth).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, ColorImage, FontData, FontDefinitions, FontFamily, Pos2, Rect, TextureHandle, TextureOptions, pos2};

const TITLE_DIR: &str = "stonehearth/ui/shell/title/images";
/// The four seasons of the title-screen town: day, evening, rain, snow.
const BACKGROUNDS: [&str; 4] = ["foreground_1.jpg", "foreground_2.jpg", "foreground_3.jpg", "foreground_4.jpg"];
const LOGO: &str = "stonehearth_logo.png";

// Google Sans (SIL Open Font License, see assets/fonts/OFL.txt), cut down to
// static Latin weights so the binary stays small.
const SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/GoogleSans-Regular.ttf");
const SANS_MEDIUM: &[u8] = include_bytes!("../assets/fonts/GoogleSans-Medium.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/GoogleSans-Bold.ttf");
const SANS_CODE: &[u8] = include_bytes!("../assets/fonts/GoogleSansCode-Regular.ttf");

/// How long each season stays up, and how long the crossfade takes.
const HOLD: Duration = Duration::from_secs(14);
const FADE: Duration = Duration::from_millis(2200);

/// Google Sans Bold: screen titles and card titles.
pub fn heading_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// Google Sans Medium: menu, buttons, emphasis.
pub fn medium_family() -> FontFamily {
    FontFamily::Name("medium".into())
}

struct Loaded {
    backgrounds: Vec<ColorImage>,
    logo: Option<ColorImage>,
}

pub struct Art {
    rx: Option<Receiver<Loaded>>,
    backgrounds: Vec<TextureHandle>,
    pub logo: Option<TextureHandle>,
    started: Instant,
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("rehearth/art")
}

/// Steam's own artwork for the game, kept by the client for its library view.
fn steam_library_art(file: &str) -> Option<Vec<u8>> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [".local/share/Steam", ".steam/steam"]
        .iter()
        .map(|root| home.join(root).join("appcache/librarycache").join(crate::game::APP_ID).join(file))
        .find_map(|p| std::fs::read(p).ok())
}

/// A file from the game's art, read from our cache first so the launcher keeps
/// its look even while the game itself is missing or being repaired.
fn game_file(smod: Option<&Path>, inner: &str) -> Option<Vec<u8>> {
    let cached = cache_dir().join(inner.rsplit('/').next().unwrap_or(inner));
    if let Ok(bytes) = std::fs::read(&cached) {
        return Some(bytes);
    }
    let bytes = from_smod(smod?, inner)?;
    let _ = std::fs::create_dir_all(cache_dir());
    let _ = std::fs::write(&cached, &bytes);
    Some(bytes)
}

fn from_smod(smod: &Path, inner: &str) -> Option<Vec<u8>> {
    let out = Command::new("unzip").arg("-p").arg(smod).arg(inner).output().ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

impl Art {
    /// Starts reading the art in the background; the window opens immediately.
    pub fn load(game: Option<PathBuf>, ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let smod = game.map(|g| g.join("mods/stonehearth.smod")).filter(|p| p.is_file());
            let get = |inner: &str| game_file(smod.as_deref(), inner);
            let mut backgrounds: Vec<ColorImage> = BACKGROUNDS
                .iter()
                .filter_map(|f| get(&format!("{TITLE_DIR}/{f}")).and_then(|b| decode(&b)))
                .collect();
            if backgrounds.is_empty() {
                backgrounds.extend(steam_library_art("library_hero.jpg").and_then(|b| decode(&b)));
            }
            let logo = get(&format!("{TITLE_DIR}/{LOGO}"))
                .or_else(|| steam_library_art("logo.png"))
                .and_then(|b| decode(&b));
            let loaded = Loaded { backgrounds, logo };
            let _ = tx.send(loaded);
            ctx.request_repaint();
        });
        Self { rx: Some(rx), backgrounds: Vec::new(), logo: None, started: Instant::now() }
    }

    /// Picks up the art once it's read; call every frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        let Ok(loaded) = rx.try_recv() else { return };
        self.rx = None;
        let opts = TextureOptions::LINEAR;
        self.backgrounds = loaded
            .backgrounds
            .into_iter()
            .enumerate()
            .map(|(i, img)| ctx.load_texture(format!("bg{i}"), img, opts))
            .collect();
        self.logo = loaded.logo.map(|img| ctx.load_texture("logo", img, opts));
        self.started = Instant::now();
    }

    /// Paints the current season (crossfading into the next) across `rect`, plus
    /// the shading that keeps text readable on top of it.
    pub fn paint_background(&self, ctx: &egui::Context, painter: &egui::Painter, rect: Rect) {
        painter.rect_filled(rect, 0.0, Color32::from_rgb(0x14, 0x10, 0x0c));
        let n = self.backgrounds.len();
        if n > 0 {
            let cycle = HOLD + FADE;
            let t = self.started.elapsed();
            let idx = (t.as_millis() / cycle.as_millis()) as usize % n;
            let into = Duration::from_millis((t.as_millis() % cycle.as_millis()) as u64);
            paint_cover(painter, &self.backgrounds[idx], rect, 255);
            if into > HOLD && n > 1 {
                let f = (into - HOLD).as_secs_f32() / FADE.as_secs_f32();
                let f = f * f * (3.0 - 2.0 * f); // smoothstep
                paint_cover(painter, &self.backgrounds[(idx + 1) % n], rect, (f * 255.0) as u8);
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(HOLD.saturating_sub(into));
            }
        }
        // left-to-right shade behind the menu, and a soft floor at the bottom
        gradient(painter, rect, Color32::from_black_alpha(215), Color32::from_black_alpha(40), true, 0.55);
        gradient(painter, rect, Color32::TRANSPARENT, Color32::from_black_alpha(150), false, 1.0);
    }
}

/// Draws a texture scaled to cover `rect`, cropping whichever side overflows.
fn paint_cover(painter: &egui::Painter, tex: &TextureHandle, rect: Rect, alpha: u8) {
    let [w, h] = tex.size();
    let img_aspect = w as f32 / h as f32;
    let aspect = rect.width() / rect.height();
    let uv = if img_aspect > aspect {
        let span = aspect / img_aspect;
        Rect::from_min_max(pos2((1.0 - span) / 2.0, 0.0), pos2((1.0 + span) / 2.0, 1.0))
    } else {
        let span = img_aspect / aspect;
        // keep the town (lower part of the art) in view when cropping vertically
        Rect::from_min_max(pos2(0.0, 1.0 - span), pos2(1.0, 1.0))
    };
    painter.image(tex.id(), rect, uv, Color32::from_white_alpha(alpha));
}

/// A linear gradient across `rect`; `horizontal` runs left→right, otherwise top→bottom.
/// `extent` is the fraction of the rect the gradient covers before it's fully `to`.
fn gradient(painter: &egui::Painter, rect: Rect, from: Color32, to: Color32, horizontal: bool, extent: f32) {
    let mut mesh = egui::Mesh::default();
    let corners: [Pos2; 4] = if horizontal {
        let mid = rect.left() + rect.width() * extent;
        [rect.left_top(), pos2(mid, rect.top()), pos2(mid, rect.bottom()), rect.left_bottom()]
    } else {
        let top = rect.bottom() - rect.height() * extent * 0.45;
        [pos2(rect.left(), top), pos2(rect.right(), top), rect.right_bottom(), rect.left_bottom()]
    };
    let colors = if horizontal { [from, to, to, from] } else { [from, from, to, to] };
    for (p, c) in corners.into_iter().zip(colors) {
        mesh.colored_vertex(p, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    if horizontal && extent < 1.0 {
        // past the gradient, keep the faint tint so the art isn't harsh against panels
        painter.rect_filled(Rect::from_min_max(pos2(corners[1].x, rect.top()), rect.right_bottom()), 0.0, to);
    }
}

/// Google Sans everywhere, with egui's own fonts behind it for symbols.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (name, bytes) in [("sans", SANS_REGULAR), ("sans-medium", SANS_MEDIUM), ("sans-bold", SANS_BOLD), ("sans-code", SANS_CODE)] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let family = |primary: &str| std::iter::once(primary.to_string()).chain(fallback.iter().cloned()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, family("sans"));
    fonts.families.insert(medium_family(), family("sans-medium"));
    fonts.families.insert(bold_family(), family("sans-bold"));
    let mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    fonts.families.insert(FontFamily::Monospace, std::iter::once("sans-code".to_string()).chain(mono).collect());
    ctx.set_fonts(fonts);
}
