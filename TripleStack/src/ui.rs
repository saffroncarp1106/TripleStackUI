use crate::window_motion::WindowMotion;
use anyhow::{Result, anyhow};
use eframe::egui::{
    self, Align2, Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId, Id,
    Key, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use image::RgbaImage;
use rfd::{FileDialog, MessageDialog, MessageLevel};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use triplestack::{MissingCircle, Options};

const TITLE: &str = "Triple Stack Skin Maker";
const BACKGROUND: Color32 = Color32::from_rgb(46, 46, 46);
const PANEL: Color32 = Color32::from_rgb(26, 26, 26);
const PINK: Color32 = Color32::from_rgb(255, 102, 204);
const DEEP_PINK: Color32 = Color32::from_rgb(224, 85, 179);
const PURPLE: Color32 = Color32::from_rgb(107, 77, 168);
const WHITE: Color32 = Color32::WHITE;
const ANIMATION_INTERVAL: Duration = Duration::from_millis(30);

pub fn run() -> Result<()> {
    eframe::run_native(
        TITLE,
        native_options(),
        Box::new(|creation| {
            let motion = WindowMotion::attach(creation)?;
            Ok(Box::new(App::new(&creation.egui_ctx, motion)))
        }),
    )
    .map_err(|error| anyhow!("Could not start the OpenGL window: {error}"))
}

fn native_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([700.0, 480.0])
        .with_min_inner_size([300.0, 200.0]);
    if let Ok(image) = image::load_from_memory_with_format(
        include_bytes!("../../icon.ico"),
        image::ImageFormat::Ico,
    ) {
        let rgba = image.into_rgba8();
        viewport = viewport.with_icon(egui::IconData {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
        });
    }
    eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        // The animation has its own frame limit. Keep native moves and resizes
        // independent of the driver's vertical-blank wait.
        vsync: false,
        ..Default::default()
    }
}

struct App {
    motion: WindowMotion,
    skin: Option<PathBuf>,
    options: Options,
    options_open: bool,
    offset: u32,
    offset_label: String,
    preview: Option<PreviewTextures>,
    button_pattern: TextureHandle,
    animation: Animation,
}

enum Action {
    Select,
    Confirm,
}

struct PreviewTextures {
    circle: TextureHandle,
    numbered: Option<TextureHandle>,
}

#[derive(Default)]
struct Animation {
    time: Duration,
    last_tick: Option<Instant>,
    triangles: Option<(Rect, Duration, Arc<Mesh>)>,
}

impl Animation {
    fn update(&mut self, ctx: &Context, paused: bool) {
        if paused {
            self.last_tick = None;
            return;
        }
        let now = Instant::now();
        if let Some(previous) = self.last_tick {
            let elapsed = now.saturating_duration_since(previous);
            if elapsed < ANIMATION_INTERVAL {
                ctx.request_repaint_after(ANIMATION_INTERVAL - elapsed);
                return;
            }
            self.time += elapsed.min(ANIMATION_INTERVAL * 2);
        }
        self.last_tick = Some(now);
        ctx.request_repaint_after(ANIMATION_INTERVAL);
    }

    fn paint(&mut self, painter: &Painter, rect: Rect) {
        let mesh = match &self.triangles {
            Some((old_rect, time, mesh)) if *old_rect == rect && *time == self.time => {
                Arc::clone(mesh)
            }
            _ => {
                let mesh = Arc::new(background_mesh(rect, self.time.as_secs_f32()));
                self.triangles = Some((rect, self.time, Arc::clone(&mesh)));
                mesh
            }
        };
        painter.add(Shape::Mesh(mesh));
    }
}

impl App {
    fn new(ctx: &Context, motion: WindowMotion) -> Self {
        install_fonts(ctx);
        Self {
            motion,
            skin: None,
            options: Options::default(),
            options_open: false,
            offset: 160,
            offset_label: "Native".to_owned(),
            preview: None,
            button_pattern: upload(ctx, "button-pattern", &button_pattern()),
            animation: Animation::default(),
        }
    }

    fn draw(&mut self, ui: &mut Ui) -> Option<Action> {
        let area = ui.max_rect();
        let painter = ui.painter();
        let center = area.center().x;
        let time = self.animation.time.as_secs_f32();
        let pulse = 0.85 + 0.15 * time.sin();
        let title_color = Color32::from_rgb(
            (255.0 * pulse) as u8,
            (102.0 * pulse) as u8,
            (204.0 * pulse) as u8,
        );
        painter.text(
            pos2(center, 27.0),
            Align2::CENTER_CENTER,
            TITLE,
            bold(21.33),
            title_color,
        );
        let warning = painter.layout(
            "Back up first! Deletes @2x, stacks defaults & hitcircle, updates skin.ini.".to_owned(),
            font(12.0),
            WHITE,
            (area.width() - 20.0).clamp(1.0, 500.0),
        );
        let warning_bottom = 50.0 + warning.size().y.max(19.0);
        let warning_pos = pos2(
            center - warning.size().x / 2.0,
            (50.0 + warning_bottom - warning.size().y) / 2.0,
        );
        painter.galley(warning_pos, warning, WHITE);

        let options_rect =
            Rect::from_center_size(pos2(center, warning_bottom + 28.5), vec2(82.0, 31.0));
        let options_label = if self.options_open {
            "Options ▲"
        } else {
            "Options ▼"
        };
        let options = button(ui, options_rect, "options", options_label, true);
        let fill = if options.is_pointer_button_down_on() {
            Color32::from_gray(70)
        } else if options.hovered() {
            Color32::from_gray(67)
        } else {
            BACKGROUND
        };
        ui.painter().rect(
            options_rect.shrink(0.5),
            2,
            fill,
            Stroke::new(1.0_f32, Color32::from_gray(99)),
            StrokeKind::Inside,
        );
        ui.painter().text(
            options_rect.center(),
            Align2::CENTER_CENTER,
            options_label,
            font(12.0),
            WHITE,
        );
        focus_ring(ui.painter(), &options, options_rect);
        if options.clicked() {
            self.options_open = !self.options_open;
        }

        let mut actions_y = options_rect.bottom() + 13.0;
        if self.options_open {
            self.draw_options(
                ui,
                Rect::from_min_size(
                    pos2(30.0, actions_y - 2.0),
                    vec2((area.width() - 60.0).max(0.0), 163.0),
                ),
            );
            actions_y += 175.0;
        }

        let select = self.action_button(
            ui,
            pos2(center - 70.0, actions_y + 22.0),
            "Select Skin",
            true,
        );
        let confirm = self.action_button(
            ui,
            pos2(center + 70.0, actions_y + 22.0),
            "Confirm",
            self.skin.is_some(),
        );

        let preview_y = actions_y + 56.0;
        let preview_area = Rect::from_min_max(
            pos2(10.0, preview_y),
            pos2(
                (area.right() - 10.0).max(10.0),
                (area.bottom() - 10.0).max(preview_y),
            ),
        );
        if preview_area.height() >= 4.0 && preview_area.width() >= 4.0 {
            self.draw_preview(ui.painter(), preview_area);
        }
        if select {
            Some(Action::Select)
        } else if confirm {
            Some(Action::Confirm)
        } else {
            None
        }
    }

    fn draw_options(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect(
            rect,
            0,
            PANEL,
            Stroke::new(1.0_f32, Color32::from_gray(88)),
            StrokeKind::Inside,
        );
        let origin = rect.min;
        let y = origin.y + 32.0;
        checkbox(
            ui,
            &painter,
            pos2(origin.x + 26.0, y),
            "Copy & Process",
            &mut self.options.copy,
        );
        let mut force = self.options.force_resolution.is_some();
        let mut refresh = checkbox(
            ui,
            &painter,
            pos2(origin.x + 161.0, y),
            "Force Overlay Res",
            &mut force,
        );
        self.options.force_resolution = force.then_some(self.offset);
        painter.text(
            pos2(origin.x + 305.0, y),
            Align2::LEFT_CENTER,
            "Offset:",
            font(12.0),
            Color32::from_gray(170),
        );
        let slider_rect =
            Rect::from_min_size(pos2(origin.x + 351.0, origin.y + 11.0), vec2(122.0, 38.0));
        if slider(ui, &painter, slider_rect, &mut self.offset) {
            self.options.force_resolution = force.then_some(self.offset);
            self.offset_label = if force {
                self.offset.to_string()
            } else {
                self.skin
                    .as_deref()
                    .and_then(|path| triplestack::native_resolution(path).ok().flatten())
                    .map_or_else(|| "Native".to_owned(), |(w, h)| format!("Native ({w}x{h})"))
            };
            refresh = true;
        }
        painter.text(
            pos2(origin.x + 485.0, y),
            Align2::LEFT_CENTER,
            &self.offset_label,
            bold(12.0),
            PINK,
        );
        let x = origin.x + 26.0;
        refresh |= checkbox(
            ui,
            &painter,
            pos2(x, origin.y + 80.5),
            "Use Rafis hitcircle",
            &mut self.options.use_rafis,
        );
        refresh |= checkbox(
            ui,
            &painter,
            pos2(x, origin.y + 111.5),
            "Use existing + Rafis circle",
            &mut self.options.overlay_on_top,
        );
        checkbox(
            ui,
            &painter,
            pos2(x, origin.y + 142.5),
            "Brighter Follow Point",
            &mut self.options.brighter_followpoint,
        );
        if refresh {
            self.update_preview(ui.ctx());
        }
    }

    fn action_button(&self, ui: &Ui, center: Pos2, label: &str, enabled: bool) -> bool {
        let rect = Rect::from_center_size(center, vec2(124.0, 44.0));
        let response = button(ui, rect, label, label, enabled);
        let painter = ui.painter();
        let pattern = Rect::from_center_size(center, vec2(120.0, 40.0));
        painter.image(self.button_pattern.id(), pattern, uv(), WHITE);
        if response.is_pointer_button_down_on() || response.hovered() {
            painter.rect_filled(pattern, 0, Color32::from_white_alpha(20));
        }
        let text_color = if enabled {
            WHITE
        } else {
            Color32::from_gray(210)
        };
        painter.text(
            center,
            Align2::CENTER_CENTER,
            label,
            bold(13.33),
            text_color,
        );
        focus_ring(painter, &response, rect);
        response.clicked()
    }

    fn draw_preview(&mut self, painter: &Painter, rect: Rect) {
        painter.rect(
            rect,
            0,
            BACKGROUND,
            Stroke::new(2.0_f32, PINK),
            StrokeKind::Inside,
        );
        let inner = rect.shrink(2.0);
        let painter = painter.with_clip_rect(inner.intersect(painter.clip_rect()));
        self.animation.paint(&painter, rect);
        let Some(preview) = &self.preview else {
            return;
        };
        let columns = if preview.numbered.is_some() { 4.0 } else { 3.0 };
        for (index, (label, texture)) in [
            ("Base", Some(&preview.circle)),
            ("Overlayed", Some(&preview.circle)),
            ("Numbered", preview.numbered.as_ref()),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(texture) = texture {
                let center = pos2(
                    rect.left() + rect.width() * (index + 1) as f32 / columns,
                    rect.center().y,
                );
                let bounds = Rect::from_center_size(center, texture.size_vec2());
                painter.image(texture.id(), bounds, uv(), WHITE);
                painter.text(
                    pos2(center.x, bounds.top() - 8.0),
                    Align2::CENTER_CENTER,
                    label,
                    font(12.0),
                    WHITE,
                );
            }
        }
    }

    fn select_skin(&mut self, ctx: &Context, frame: &eframe::Frame) {
        let mut dialog = FileDialog::new()
            .set_parent(frame)
            .set_title("Select osu! Skin Folder");
        if let Some(skin) = &self.skin {
            dialog = dialog.set_directory(skin);
        }
        if let Some(folder) = dialog.pick_folder() {
            self.skin = Some(folder);
            self.update_preview(ctx);
            ctx.request_repaint();
        }
    }

    fn update_preview(&mut self, ctx: &Context) {
        let Some(path) = &self.skin else { return };
        if let Ok(preview) = triplestack::preview(path, self.options) {
            self.preview = Some(PreviewTextures {
                circle: upload(ctx, "preview-circle", &preview.circle),
                numbered: preview
                    .numbered
                    .as_ref()
                    .map(|image| upload(ctx, "preview-number", image)),
            });
        }
    }

    fn confirm(&self, frame: &eframe::Frame) {
        let Some(skin) = &self.skin else { return };
        let destination = match triplestack::process(skin, self.options) {
            Ok(destination) => destination,
            Err(error) => {
                let title = if error.is::<MissingCircle>() {
                    "Missing asset"
                } else {
                    "Processing failed"
                };
                message(frame, title, &format!("{error:#}"), MessageLevel::Error);
                return;
            }
        };

        let name = format!(
            "{}@3xStack.osk",
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        let mut dialog = FileDialog::new()
            .set_parent(frame)
            .set_file_name(name)
            .add_filter("OSK Files", &["osk"])
            .add_filter("All Files", &["*"]);
        if let Some(home) = std::env::var_os("USERPROFILE") {
            dialog = dialog.set_directory(PathBuf::from(home).join("Downloads"));
        }
        let Some(mut file) = dialog.save_file() else {
            return;
        };
        if file.extension().is_none() {
            file.set_extension("osk");
        }
        match triplestack::export_osk(&destination, &file) {
            Ok(()) => message(
                frame,
                "Success",
                &format!("Skin exported to:\n{}", file.display()),
                MessageLevel::Info,
            ),
            Err(error) => message(
                frame,
                "Error",
                &format!("Export failed: {error:#}"),
                MessageLevel::Error,
            ),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        let paused =
            self.motion.active() || ctx.input(|input| input.viewport().minimized == Some(true));
        self.animation.update(ctx, paused);
        let response = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND))
            .show(ctx, |ui| self.draw(ui));
        match response.inner {
            Some(Action::Select) => self.select_skin(ctx, frame),
            Some(Action::Confirm) => self.confirm(frame),
            None => {}
        }
    }
}

fn message(frame: &eframe::Frame, title: &str, text: &str, level: MessageLevel) {
    MessageDialog::new()
        .set_parent(frame)
        .set_title(title)
        .set_description(text)
        .set_level(level)
        .show();
}

fn button(ui: &Ui, rect: Rect, id: &str, label: &str, enabled: bool) -> Response {
    let response = ui.interact(
        rect,
        Id::new(id),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    response
}

fn checkbox(ui: &Ui, painter: &Painter, pos: Pos2, label: &str, checked: &mut bool) -> bool {
    let galley = painter.layout_no_wrap(label.to_owned(), font(12.0), WHITE);
    let rect = Rect::from_min_size(pos - vec2(0.0, 12.5), vec2(galley.size().x + 25.0, 25.0));
    let response = ui.interact(
        rect.intersect(painter.clip_rect()),
        Id::new(label),
        Sense::click(),
    );
    if response.clicked() {
        *checked = !*checked;
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, *checked, label));

    let square = Rect::from_center_size(pos + vec2(8.0, 0.0), vec2(13.0, 13.0));
    let fill = if *checked {
        PURPLE
    } else {
        Color32::from_gray(51)
    };
    painter.rect(
        square,
        0,
        fill,
        Stroke::new(1.0_f32, Color32::from_gray(100)),
        StrokeKind::Inside,
    );
    if *checked {
        let center = square.center();
        painter.line_segment(
            [center + vec2(-3.0, 0.0), center + vec2(-1.0, 2.5)],
            Stroke::new(1.4_f32, WHITE),
        );
        painter.line_segment(
            [center + vec2(-1.0, 2.5), center + vec2(3.5, -2.5)],
            Stroke::new(1.4_f32, WHITE),
        );
    }
    let color = if response.hovered() { PINK } else { WHITE };
    painter.galley_with_override_text_color(
        pos + vec2(20.0, -galley.size().y / 2.0),
        galley,
        color,
    );
    focus_ring(painter, &response, rect);
    response.clicked()
}

fn slider(ui: &Ui, painter: &Painter, rect: Rect, value: &mut u32) -> bool {
    let response = ui.interact(
        rect.intersect(painter.clip_rect()),
        Id::new("offset-slider"),
        Sense::click_and_drag(),
    );
    let previous = *value;
    let rail = Rect::from_min_size(rect.min + vec2(1.0, 19.0), vec2(120.0, 17.0));
    let first = rail.left() + 15.0;
    let last = rail.right() - 15.0;
    if (response.is_pointer_button_down_on() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        *value =
            100 + (60.0 * ((pointer.x - first) / (last - first)).clamp(0.0, 1.0)).round() as u32;
    }
    if response.has_focus() {
        if ui.input(|input| input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::ArrowDown))
        {
            *value = value.saturating_sub(1).max(100);
        }
        if ui.input(|input| input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::ArrowUp)) {
            *value = (*value + 1).min(160);
        }
    }
    response.widget_info(|| WidgetInfo::slider(true, f64::from(*value), "Offset"));

    painter.rect(
        rail,
        0,
        Color32::from_gray(56),
        Stroke::new(1.0_f32, Color32::from_gray(83)),
        StrokeKind::Inside,
    );
    let x = egui::lerp(first..=last, (*value - 100) as f32 / 60.0);
    let thumb = Rect::from_center_size(pos2(x, rail.center().y), vec2(28.0, 15.0));
    painter.rect(
        thumb,
        0,
        BACKGROUND,
        Stroke::new(1.0_f32, Color32::from_gray(100)),
        StrokeKind::Inside,
    );
    painter.text(
        pos2(x, rail.top() - 2.0),
        Align2::CENTER_BOTTOM,
        value.to_string(),
        font(12.0),
        PINK,
    );
    focus_ring(painter, &response, rect);
    *value != previous
}

fn focus_ring(painter: &Painter, response: &Response, rect: Rect) {
    if response.has_focus() && !response.hovered() {
        painter.rect_stroke(
            rect.shrink(1.0),
            1,
            Stroke::new(1.0_f32, PINK),
            StrokeKind::Inside,
        );
    }
}

fn background_mesh(rect: Rect, time: f32) -> Mesh {
    let mut mesh = Mesh::default();
    let (width, height) = (rect.width().ceil() as u32, rect.height().ceil() as u32);
    for x in (0..width).step_by(20) {
        let wave = 2.0 * (x as f32 * 0.02 + time * (0.04 / 0.03)).sin();
        for y in (0..height).step_by(20) {
            let intensity = ((x + y) as f32 * 0.01 + time * (0.05 / 0.03)).sin();
            let color = if intensity > 0.2 {
                PURPLE
            } else {
                Color32::from_gray(68)
            };
            let (x, y) = (rect.left() + x as f32, rect.top() + y as f32 + wave);
            let first = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(x + 0.5, y + 19.5), color);
            mesh.colored_vertex(pos2(x + 10.0, y + 0.5), color);
            mesh.colored_vertex(pos2(x + 19.5, y + 19.5), color);
            mesh.add_triangle(first, first + 1, first + 2);
        }
    }
    mesh
}

fn upload(ctx: &Context, name: &str, rgba: &RgbaImage) -> TextureHandle {
    let size = [rgba.width() as usize, rgba.height() as usize];
    ctx.load_texture(
        name,
        ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        TextureOptions::NEAREST,
    )
}

fn uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0))
}

fn button_pattern() -> RgbaImage {
    RgbaImage::from_fn(120, 40, |x, y| {
        let column = x % 10;
        let row = y % 10;
        let spread = row / 2;
        let color = if x == 0
            || x == 119
            || row == 0
            || y == 39
            || (5 - spread..=5 + spread).contains(&column)
        {
            DEEP_PINK
        } else {
            PINK
        };
        image::Rgba(color.to_array())
    })
}

fn install_fonts(ctx: &Context) {
    let mut definitions = FontDefinitions::default();
    let mut bold_family = definitions.families[&FontFamily::Proportional].clone();
    if let Some(windows) = std::env::var_os("WINDIR") {
        let fonts = PathBuf::from(windows).join("Fonts");
        if let Ok(data) = std::fs::read(fonts.join("segoeui.ttf")) {
            definitions
                .font_data
                .insert("segoe".into(), Arc::new(FontData::from_owned(data)));
            definitions
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe".into());
        }
        if let Ok(data) = std::fs::read(fonts.join("segoeuib.ttf")) {
            definitions
                .font_data
                .insert("segoe-bold".into(), Arc::new(FontData::from_owned(data)));
            bold_family.insert(0, "segoe-bold".into());
        }
    }
    definitions
        .families
        .insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(definitions);
}

fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}
