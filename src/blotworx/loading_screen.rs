use egui::{Color32, ColorImage, Rect, TextureHandle, TextureOptions, Ui, Vec2};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

const DEFAULT_ART: &str = "LoadingScreens/MickeyInitialScreen.dds";
const MINIMUM_PRESENTATION: Duration = Duration::from_millis(330);
const DOT_FRAME: Duration = Duration::from_millis(110);

pub(super) struct Screen {
    pending: Option<Receiver<Artwork>>,
    opened: Instant,
    background: Option<TextureHandle>,
    dots: Vec<TextureHandle>,
    spinner: Option<TextureHandle>,
    widescreen: bool,
    show_activity: bool,
    first_paint: Option<Instant>,
}

impl Screen {
    pub(super) fn load(
        ctx: &egui::Context,
        root: &Path,
        destination: Option<&str>,
    ) -> Self {
        let root = root.to_owned();
        let destination = destination.map(str::to_owned);
        Self::spawn(ctx, false, destination.is_some(), move || Artwork::load(&root, destination.as_deref()))
    }

    pub(super) fn em2(ctx: &egui::Context, root: &Path) -> Self {
        let root = root.to_owned();
        Self::spawn(ctx, true, true, move || Artwork::em2(&root))
    }

    fn spawn(ctx: &egui::Context, widescreen: bool, show_activity: bool, load: impl FnOnce() -> Artwork + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(load());
            ctx.request_repaint();
        });
        Self {
            pending: Some(receiver),
            opened: Instant::now(),
            background: None,
            dots: Vec::new(),
            spinner: None,
            widescreen,
            show_activity,
            first_paint: None,
        }
    }

    pub(super) fn has_presented(&self) -> bool {
        self.first_paint.is_some()
    }

    pub(super) fn ready_to_dismiss(&self) -> bool {
        self.first_paint.is_some_and(|start| start.elapsed() >= MINIMUM_PRESENTATION)
            || self.elapsed() >= Duration::from_secs(2)
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.opened.elapsed()
    }

    pub(super) fn paint(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx();
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(artwork) => {
                    let upload = |(name, image)| ctx.load_texture(name, image, TextureOptions::LINEAR);
                    self.background = artwork.background.map(upload);
                    self.dots = artwork.dots.into_iter().map(upload).collect();
                    self.spinner = artwork.spinner.map(upload);
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    log::warn!("Loading artwork worker stopped");
                    self.pending = None;
                }
            }
        }
        let viewport = ctx.content_rect();
        if self.pending.is_none() && viewport.is_positive() {
            self.first_paint.get_or_insert_with(Instant::now);
        }
        let elapsed = self.first_paint.unwrap_or(self.opened).elapsed();

        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("game_loading_screen"));
        ctx.move_to_top(layer);
        let painter = ctx.layer_painter(layer).with_clip_rect(viewport);
        // The EM2 loading art is stored as square textures for 16:9 quads.
        let size = if self.widescreen {
            Vec2::new(640., 360.)
        } else {
            self.background.as_ref().map_or(Vec2::new(640., 480.), TextureHandle::size_vec2)
        };
        let scale = (viewport.width() / size.x).min(viewport.height() / size.y);
        let image = Rect::from_center_size(viewport.center(), size * scale);
        let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
        painter.rect_filled(viewport, 0., Color32::BLACK);
        if let Some(background) = &self.background {
            painter.image(background.id(), image, uv, Color32::WHITE);
        }
        if !self.show_activity {
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }
        if let Some(spinner) = &self.spinner {
            let side = image.height() / 8.;
            let center = image.min + image.size() * 0.8;
            let square = Rect::from_center_size(center, Vec2::splat(side));
            let mut mesh = egui::Mesh::with_texture(spinner.id());
            mesh.add_rect_with_uv(square, uv, Color32::WHITE);
            mesh.rotate(egui::emath::Rot2::from_angle(elapsed.as_secs_f32() * std::f32::consts::TAU), center);
            painter.add(egui::Shape::mesh(mesh));
        } else if !self.dots.is_empty() {
            let frame = (elapsed.as_millis() / DOT_FRAME.as_millis()) as usize;
            let dot = &self.dots[frame % self.dots.len()];
            let center = egui::pos2(image.center().x, image.bottom() - image.height() * 0.1);
            let marker = Rect::from_center_size(center, dot.size_vec2() * (image.width() / 640.));
            painter.image(dot.id(), marker, uv, Color32::WHITE);
        } else {
            let radius = 10.;
            let start = elapsed.as_secs_f32() * std::f32::consts::TAU;
            let points = (0..=24).map(|segment| {
                let angle = start + (segment as f32 / 24.) * std::f32::consts::PI * 1.5;
                viewport.center() + Vec2::angled(angle) * radius
            }).collect();
            painter.add(egui::Shape::line(points, egui::Stroke::new(2., Color32::WHITE)));
        }
        let repaint = if self.dots.is_empty() { Duration::from_millis(16) } else { DOT_FRAME };
        ctx.request_repaint_after(repaint);
    }
}

pub(super) fn report_button(
    ctx: &egui::Context,
    level: Option<&str>,
    error: Option<&str>,
    elapsed: Option<Duration>,
) {
    if error.is_none() && elapsed.is_none_or(|time| time < Duration::from_secs(15)) {
        return;
    }
    let overlay = egui::Area::new(egui::Id::new("startup_diagnostics"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-20., -20.])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(340.);
                if error.is_some() {
                    ui.strong("Blotworx couldn’t finish loading");
                } else {
                    ui.label("Loading is taking longer than usual.");
                }
                #[cfg(not(any(target_os = "android", target_os = "horizon")))]
                if ui.button("Save diagnostic report…").clicked() {
                    let report = crate::startup_log::report(level, error, elapsed);
                    if let Err(error) = crate::startup_log::save_report(&report) {
                        rfd::MessageDialog::new()
                            .set_title("Could not save the report")
                            .set_description(error.to_string())
                            .set_level(rfd::MessageLevel::Error)
                            .show();
                    }
                }
                #[cfg(target_os = "android")]
                {
                    let id = ui.id().with("android_report");
                    if ui.button("View diagnostic report").clicked() {
                        let report = crate::startup_log::report(level, error, elapsed);
                        ui.ctx().data_mut(|data| data.insert_temp(id, report));
                    }
                    if let Some(report) = ui.ctx().data(|data| data.get_temp::<String>(id)) {
                        egui::ScrollArea::vertical().max_height(160.).show(ui, |ui| {
                            ui.label(egui::RichText::new(report).monospace());
                        });
                    }
                }
            });
        });
    ctx.move_to_top(overlay.response.layer_id);
}

type Texture = (String, ColorImage);

#[derive(Default)]
struct Artwork {
    background: Option<Texture>,
    dots: Vec<Texture>,
    spinner: Option<Texture>,
}

impl Artwork {
    fn load(root: &Path, destination: Option<&str>) -> Self {
        let mut store = match super::resource_store::Store::new(root, &[]) {
            Ok(store) => store,
            Err(error) => {
                log::warn!("Loading screens: {error:#}");
                return Self::default();
            }
        };
        let requested = destination
            .and_then(|level| super::assets::loading_artwork(root, level)
                .inspect_err(|error| log::warn!("Level loading artwork: {error:#}"))
                .ok()
                .flatten())
            .map(|path| path.replace('\\', "/"));
        let mut read = |path: &str| {
            texture(&mut store, path)
                .inspect_err(|error| log::warn!("Loading artwork {path}: {error:#}"))
                .ok()
        };
        let background = requested.as_deref().and_then(|path| {
            let Some(localized) = localized_path(path, &language(root)) else {
                return read(path);
            };
            read(&localized)
                .or_else(|| {
                    let english = localized_path(path, "English_NTSC")?;
                    if english == localized {
                        return None;
                    }
                    read(&english)
                })
                .or_else(|| read(path))
        }).or_else(|| read(DEFAULT_ART));
        let dots = if destination.is_some() {
            (1..=8).filter_map(|index| read(&format!("LoadingScreens/LoadingDots_{index:02}.tga"))).collect()
        } else {
            Vec::new()
        };
        Self { background, dots, spinner: None }
    }

    fn em2(root: &Path) -> Self {
        // Yen Sid's Lab uses the generic loading pool. The retail PC game
        // stores each backdrop and the spinner in loose texture NIFs.
        let load = |file: &str| {
            let path = root.join("LoadingScreens").join(file);
            let bytes = std::fs::read(&path).ok()?;
            let (rgba, width, height) = crate::render_support::nif::decode_image_bytes(&bytes)?;
            Some((file.to_owned(), ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba)))
        };
        let index = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.subsec_nanos() % 6);
        let background = (0..6).find_map(|offset| {
            load(&format!("loading_screen_random{:02}_tex.nif", (index + offset) % 6 + 1))
        });
        let spinner = load("loading_spinner_tex.nif");
        Self { background, dots: Vec::new(), spinner }
    }
}

fn language(root: &Path) -> String {
    use crate::render_support::game_config::setting_value;
    [("cmdline.txt", true), ("ConfigFiles.ini", false)]
        .into_iter()
        .find_map(|(file, command)| {
            let text = std::fs::read_to_string(root.join(file)).ok()?;
            setting_value(&text, "Language", command).filter(|value| {
                !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_')
            })
        })
        .unwrap_or_else(|| "English_NTSC".into())
}

fn localized_path(path: &str, language: &str) -> Option<String> {
    let (directory, name) = path.rsplit_once('/')?;
    (directory.eq_ignore_ascii_case("LoadingScreens")
        && ["GuardianSplashScreen.dds", "MickeySplashScreen.dds"]
            .iter()
            .any(|splash| splash.eq_ignore_ascii_case(name)))
        .then(|| format!("{directory}/Localize/{language}/{name}"))
}

fn texture(
    store: &mut super::resource_store::Store,
    path: &str,
) -> anyhow::Result<Texture> {
    use anyhow::Context as _;
    let bytes = store.read(path)?.data;
    let format = match path.rsplit_once('.') {
        Some((_, extension)) if extension.eq_ignore_ascii_case("tga") => image::ImageFormat::Tga,
        _ => image::ImageFormat::Dds,
    };
    let image = image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
        .decode()
        .with_context(|| format!("Decode {path}"))?
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok((path.to_owned(), ColorImage::from_rgba_unmultiplied(size, &image)))
}
