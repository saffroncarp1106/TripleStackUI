use crate::skin_export::{ExportPanel, ExportSelection};
use crate::window_motion::WindowMotion;
use anyhow::{Result, anyhow};
use eframe::egui::{
    self, Align2, Color32, ColorImage, Context, FontData, FontDefinitions, FontFamily, FontId, Id,
    Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use image::RgbaImage;
use rfd::{FileDialog, MessageDialog, MessageLevel};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use triplestack::{Options, skin_ini::SkinIni};

const TITLE: &str = "Triple Stack Skin Maker";
const BACKGROUND: Color32 = Color32::from_rgb(46, 46, 46);
const PANEL: Color32 = Color32::from_rgb(26, 26, 26);
const PINK: Color32 = Color32::from_rgb(255, 102, 204);
const DEEP_PINK: Color32 = Color32::from_rgb(224, 85, 179);
const PURPLE: Color32 = Color32::from_rgb(107, 77, 168);
const WHITE: Color32 = Color32::WHITE;
const ANIMATION_INTERVAL: Duration = Duration::from_millis(7);
const APPROACH_START_SCALE: f32 = 2.42;
const RING_PAUSE_MS: u64 = 1000; // Delay before the next approach ring; 0 disables it.
const COMBO_PREVIEW_LABELS: [&str; 7] = [
    "Before TripleStack",
    "After TripleStack",
    "Approach circle",
    "Hitcircle",
    "Hitcircle overlay",
    "Animated before",
    "Animated after",
];
const MIN_UI_ZOOM: f32 = 0.5;
const MAX_UI_ZOOM: f32 = 3.0;
const ZOOM_STEPS_PER_UNIT: f32 = 10.0;
const FONT_ATLAS_POINTS: f32 = 512.0;
const MIN_FONT_ATLAS_SIDE: usize = 1024; // epaint's minimum width in pixels.

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
        .with_inner_size([840.0, 540.0])
        .with_min_inner_size([620.0, 360.0]);
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
    preview: Option<PreviewTextures>,
    preview_scroll: f32,
    preview_scroll_x: f32,
    requested_zoom: f32,
	device_texture_limit: usize,
    approach_duration_ms: f32,
    approach_ring: ApproachRing,
    combo_preview_visible: [bool; 7],
    button_pattern: TextureHandle,
    animation: Animation,
    skin_ini: Option<SkinIni>,
    ini_error: Option<String>,
    ini_dirty: bool,
    editor_search: crate::editor_search::EditorSearch,
    active_menu: SidebarMenu,
    audio: crate::audio_tab::AudioTab,
    lazer_browser: crate::lazer_browser::LazerBrowser,
    export_panel: ExportPanel,
    number_font_panel: crate::number_font_panel::NumberFontPanel,
}

enum Action {
    Select,
    SelectLazer,
    SelectLazerSounds(Option<PathBuf>),
    LoadProject,
    SaveProject,
    Confirm,
    Export,
}

struct PreviewTextures {
    original_circle: TextureHandle,
    circle: TextureHandle,
    numbered: Option<TextureHandle>,
    followpoints: Vec<(usize, TextureHandle)>,
    assets: Vec<(&'static str, TextureHandle)>,
}

#[derive(Clone, Copy)]
enum SidebarMenu {
    TripleStack,
    SkinIni,
    Audio,
    Export,
}

const SIDEBAR_MENUS: &[(SidebarMenu, &str)] = &[
    (SidebarMenu::TripleStack, "TripleStack"),
    (SidebarMenu::SkinIni, "Skin.ini editor"),
    (SidebarMenu::Audio, "Audio / Hitsounds"),
    (SidebarMenu::Export, "Export"),
];

// Each cycle renders exact contact before the ring disappears, even if a frame
// arrives after the configured duration. The pause begins at that contact frame.
#[derive(Default)]
struct ApproachRing {
    started_at: Option<Duration>,
    contact_at: Option<Duration>,
}

impl ApproachRing {
    fn scale(&mut self, now: Duration, duration: Duration, pause: Duration) -> Option<f32> {
        let started_at = *self.started_at.get_or_insert(now);
        if let Some(contact_at) = self.contact_at {
            if now == contact_at {
                return Some(1.0);
            }
            if now.saturating_sub(contact_at) < pause {
                return None;
            }
            self.started_at = Some(now);
            self.contact_at = None;
            return Some(APPROACH_START_SCALE);
        }
        let elapsed = now.saturating_sub(started_at);
        if elapsed >= duration {
            self.contact_at = Some(now);
            return Some(1.0);
        }
        let progress = (elapsed.as_secs_f64() / duration.as_secs_f64()) as f32;
        Some(APPROACH_START_SCALE - (APPROACH_START_SCALE - 1.0) * progress)
    }
}

#[derive(Default)]
struct Animation {
    time: Duration,
    last_tick: Option<Instant>,
    triangles: Option<(Rect, Duration, f32, Arc<Mesh>)>,
}

impl Animation {
    fn update(&mut self, ctx: &Context, paused: bool) {
        if paused {
            self.last_tick = None;
            self.triangles = None;
            return;
        }
        let now = Instant::now();
        if let Some(previous) = self.last_tick {
            let elapsed = now.saturating_duration_since(previous);
            if elapsed < ANIMATION_INTERVAL {
                ctx.request_repaint_after(ANIMATION_INTERVAL - elapsed);
                return;
            }
            // Keep durations accurate even when rendering misses a frame.
            self.time += elapsed;
        }
        self.last_tick = Some(now);
        ctx.request_repaint_after(ANIMATION_INTERVAL);
    }

    fn paint(&mut self, painter: &Painter, rect: Rect, scale: f32) {
        let mesh = match &self.triangles {
            Some((old_rect, time, old_scale, mesh))
                if *old_rect == rect && *time == self.time && *old_scale == scale =>
            {
                Arc::clone(mesh)
            }
            _ => {
                let mesh = Arc::new(background_mesh(rect, self.time.as_secs_f32(), scale));
                self.triangles = Some((rect, self.time, scale, Arc::clone(&mesh)));
                mesh
            }
        };
        painter.add(Shape::Mesh(mesh));
    }
}

impl App {
    fn new(ctx: &Context, motion: WindowMotion) -> Self {
        install_fonts(ctx);
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        let audio = crate::audio_tab::AudioTab::default();
        audio.attach_context(ctx);
        Self {
            motion,
            skin: None,
            options: Options::default(),
            preview: None,
            preview_scroll: 0.0,
            preview_scroll_x: 0.0,
            requested_zoom: 1.0,
			device_texture_limit: MIN_FONT_ATLAS_SIDE,
            approach_duration_ms: 600.0,
            approach_ring: ApproachRing::default(),
            combo_preview_visible: [true; 7],
            button_pattern: upload(ctx, "button-pattern", &button_pattern()),
            animation: Animation::default(),
            skin_ini: None,
            ini_error: None,
            ini_dirty: false,
            editor_search: crate::editor_search::EditorSearch::default(),
            active_menu: SidebarMenu::TripleStack,
            audio,
            lazer_browser: crate::lazer_browser::LazerBrowser::default(),
            export_panel: ExportPanel::default(),
            number_font_panel: Default::default(),
        }
    }

    fn draw(&mut self, ui: &mut Ui) -> Option<Action> {
        if matches!(self.active_menu, SidebarMenu::Export) {
            return self
                .export_panel
                .draw(
                    ui,
                    self.skin.as_deref(),
                    self.skin_ini.is_some(),
                    self.audio.has_export_changes(),
                    &self.audio.export_summary(),
                    self.button_pattern.id(),
                )
                .then_some(Action::Export);
        }
        if matches!(self.active_menu, SidebarMenu::Audio) {
            self.audio.draw(ui);
            return None;
        }
        let area = ui.max_rect().shrink(10.0);
        if area.width() < 8.0 || area.height() < 8.0 {
            return None;
        }
        // Keep controls and the preview inside the available space at every zoom.
        let scale = (area.width() / 500.0)
            .min(area.height() / 400.0)
            .min(1.0)
            .max(0.1);
        let center = area.center().x;
        ui.painter().text(
            pos2(center, area.top() + 20.0 * scale),
            Align2::CENTER_CENTER,
            TITLE,
            bold(21.33 * scale),
            PINK,
        );
        let gap = 12.0 * scale;
        let button_width = ((area.width() - gap) / 2.0).min(225.0 * scale);
        let button_y = area.top() + 66.0 * scale;
        let lazer = self.action_button(
            ui,
            Rect::from_center_size(
                pos2(center - (button_width + gap) / 2.0, button_y),
                vec2(button_width, 44.0 * scale),
            ),
            "Load from lazer",
            true,
            scale,
        );
        let stable = self.action_button(
            ui,
            Rect::from_center_size(
                pos2(center + (button_width + gap) / 2.0, button_y),
                vec2(button_width, 44.0 * scale),
            ),
            "Load from directory\n(stable)",
            true,
            scale,
        );
        let mut preview_top = area.top() + 100.0 * scale;
        if matches!(self.active_menu, SidebarMenu::SkinIni) {
            let header_area =
                Rect::from_min_max(pos2(area.left(), preview_top), area.right_bottom());
            let header = ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt("combo-preview-header")
                    .max_rect(header_area),
                |ui| self.draw_combo_preview_header(ui),
            );
            preview_top = header.response.rect.bottom() + 12.0;
        }
        let preview_area = Rect::from_min_max(pos2(area.left(), preview_top), area.right_bottom());
        if preview_area.is_positive() {
            self.draw_preview(ui, preview_area);
            self.draw_skin_cursor(ui, preview_area);
        }
        if lazer {
            Some(Action::SelectLazer)
        } else if stable {
            Some(Action::Select)
        } else {
            None
        }
    }
    fn draw_settings(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 10.0;
        let mut force = self.options.force_resolution.is_some();
        let mut refresh = ui.checkbox(&mut force, "Force Overlay Res").changed();
        self.options.force_resolution = force.then_some(160);
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        refresh |= ui
            .checkbox(&mut self.options.use_rafis, "Use Rafis hitcircle")
            .changed();
        refresh |= ui
            .checkbox(
                &mut self.options.overlay_on_top,
                "Use existing + Rafis circle",
            )
            .changed();
        if refresh {
            self.update_preview(ui.ctx());
        }
        ui.add_space(16.0);
        ui.separator();
        ui.label(format!("UI zoom: {:.0}%", ui.ctx().zoom_factor() * 100.0));
        ui.label(
            egui::RichText::new("Ctrl+= / Ctrl+- / Ctrl+wheel")
                .size(11.0)
                .color(Color32::from_gray(170)),
        );
    }

    fn draw_sidebar(&mut self, ui: &mut Ui) -> Option<Action> {
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, font(13.0));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Button, font(13.0));
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.spacing_mut().button_padding = vec2(12.0, 8.0);
        for (menu, title) in SIDEBAR_MENUS {
            if ui
                .add_sized(
                    [ui.available_width(), 38.0],
                    egui::Button::new(egui::RichText::new(*title).size(14.0).strong()).selected(
                        std::mem::discriminant(menu) == std::mem::discriminant(&self.active_menu),
                    ),
                )
                .clicked()
            {
                if matches!(menu, SidebarMenu::Export) {
                    self.open_export();
                } else {
                    self.active_menu = *menu;
                }
            }
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(12.0);
        let mut select_source = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Load from directory (stable)").clicked() {
                select_source = Some(Action::Select);
            }
            if ui.button("Load from lazer").clicked() {
                select_source = Some(Action::SelectLazer);
            }
        });
        if select_source.is_some() {
            return select_source;
        }
        if ui
            .add_sized(
                [ui.available_width(), 32.0],
                egui::Button::new("Load project"),
            )
            .clicked()
        {
            return Some(Action::LoadProject);
        }
        if ui
            .add_enabled_ui(self.skin.is_some() || self.audio.has_content(), |ui| {
                ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::Button::new("Save project"),
                )
            })
            .inner
            .clicked()
        {
            return Some(Action::SaveProject);
        }
        let (_, confirm_rect) = ui.allocate_space(vec2(ui.available_width(), 44.0));
        if self.action_button(
            ui,
            confirm_rect,
            "Confirm",
            self.skin.is_some() || self.audio.has_content(),
            1.0,
        ) {
            return Some(Action::Confirm);
        }
        ui.small("Confirm opens Export options. Files are saved only when you export.");
        ui.separator();
        match self.active_menu {
            SidebarMenu::TripleStack => {
                ui.label(
                    egui::RichText::new("Project")
                        .size(15.0)
                        .strong()
                        .color(PINK),
                );
                if let Some(skin) = &self.skin {
                    ui.add(
                        egui::Label::new(skin.file_name().unwrap_or_default().to_string_lossy())
                            .truncate(),
                    )
                    .on_hover_text(skin.display().to_string());
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("Stack options")
                        .size(15.0)
                        .strong()
                        .color(PINK),
                );
                ui.add_space(4.0);
                self.draw_settings(ui);
                None
            }
            SidebarMenu::SkinIni => {
                self.draw_skin_ini_editor(ui);
                None
            }
            SidebarMenu::Export => {
                ui.label("Select which edits to include on the Export page, then choose where to save your .osk skin.");
                ui.small("Audio changes use the active After preset and added audio files.");
                None
            }
            SidebarMenu::Audio => {
                self.audio.sidebar(ui);
                self.audio
                    .take_lazer_request()
                    .map(|request| Action::SelectLazerSounds(request.source))
            }
        }
    }

    fn draw_skin_ini_editor(&mut self, ui: &mut Ui) {
        let Some(ini) = &mut self.skin_ini else {
            ui.label("Select a skin with a skin.ini file.");
            if let Some(error) = &self.ini_error {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
            return;
        };
        let source_before = ini.source().to_owned();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Skin.ini")
                    .size(15.0)
                    .strong()
                    .color(PINK),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(ini.can_undo(), egui::Button::new("Undo"))
                    .on_hover_text("Undo the last text or combo color change")
                    .clicked()
                    && ini.undo()
                {
                    self.ini_dirty = true;
                }
            });
        });
        ui.label(
            egui::RichText::new("Code and color controls update the preview.")
                .size(12.0)
                .color(Color32::from_gray(180)),
        );
        ui.add_space(8.0);
        let mut code = ini.source().to_owned();
        let code_changed = self.editor_search.show(ui, &mut code);
        if code_changed {
            ini.set_source(code);
            self.ini_dirty = true;
        }
        ui.separator();
        ui.add_space(10.0);
        if self.number_font_panel.show(ui, self.skin.as_deref(), ini) {
            self.ini_dirty = true;
        }
        ui.separator();
        ui.label(egui::RichText::new("Cursor").strong().color(PINK));
        let mut cursor_expand = ini.cursor_expand();
        if ui
            .checkbox(&mut cursor_expand, "Expand cursor on click")
            .on_hover_text(
                "CursorExpand in [General]. Test with mouse buttons or Z/X over the preview.",
            )
            .changed()
        {
            ini.set_cursor_expand(cursor_expand);
            self.ini_dirty = true;
        }
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Combo colors").strong().color(PINK));
        ui.add_space(6.0);
        for color in ini.colors.clone() {
            let mut rgb = color.rgb;
            let mut delete = false;
            ui.push_id(("combo-color", color.number), |ui| {
                egui::Frame::new()
                    .fill(Color32::from_gray(34))
                    .inner_margin(10.0)
                    .corner_radius(5.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Combo {}", color.number)).strong(),
                            );
                            ui.color_edit_button_srgb(&mut rgb);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    delete = ui
                                        .small_button("\u{00d7}")
                                        .on_hover_text(format!("Delete Combo {}", color.number))
                                        .clicked();
                                },
                            );
                        });
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for (channel, value) in
                                ["R ", "G ", "B "].into_iter().zip(rgb.iter_mut())
                            {
                                ui.add(egui::DragValue::new(value).range(0..=255).prefix(channel));
                            }
                        });
                    });
            });
            if delete {
                ini.remove_color(color.number);
                self.ini_dirty = true;
            } else if rgb != color.rgb {
                ini.set_color(color.number, rgb);
                self.ini_dirty = true;
            }
            ui.add_space(6.0);
        }
        if ini.colors.is_empty() {
            ui.label("No Combo colors found. Add one below.");
        }
        if ui.button("Add combo color").clicked() {
            let number = (1..)
                .find(|n| !ini.colors.iter().any(|c| c.number == *n))
                .unwrap();
            ini.set_color(number, [255, 192, 0]);
            self.ini_dirty = true;
        }
        if ui
            .add_enabled(self.ini_dirty, egui::Button::new("Save skin.ini"))
            .clicked()
        {
            if let Some(skin) = &self.skin {
                match triplestack::save_skin_ini_with_numbers(skin, ini) {
                    Ok(()) => {
                        self.ini_dirty = false;
                        self.ini_error = None;
                    }
                    Err(error) => self.ini_error = Some(format!("{error:#}")),
                }
            }
        }
        if let Some(error) = &self.ini_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        let number_prefix_changed = ini
            .source()
            .lines()
            .filter(|line| line.to_ascii_lowercase().contains("hitcircleprefix"))
            .collect::<Vec<_>>()
            != source_before
                .lines()
                .filter(|line| line.to_ascii_lowercase().contains("hitcircleprefix"))
                .collect::<Vec<_>>();
        if ini.source() != source_before {
            // The text editor is drawn before the color controls. Refresh it
            // after a control changes the source, even when animation is paused.
            ui.ctx().request_repaint();
        }
        if number_prefix_changed {
            self.update_preview(ui.ctx());
        }
    }

    fn action_button(&self, ui: &Ui, rect: Rect, label: &str, enabled: bool, scale: f32) -> bool {
        patterned_button(ui, self.button_pattern.id(), rect, label, enabled, scale)
    }

    fn draw_skin_cursor(&self, ui: &Ui, viewport: Rect) {
        // Scroll areas and their children can own hover; the cursor should
        // cover the entire preview regardless of which child is under it.
        if !ui.rect_contains_pointer(viewport.shrink(2.0)) {
            return;
        }
        let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else {
            return;
        };
        let Some(cursor) = self.preview.as_ref().and_then(|p| {
            p.assets
                .iter()
                .find(|(name, _)| *name == "Cursor")
                .map(|(_, texture)| texture)
        }) else {
            return;
        };
        let size = cursor.size_vec2();
        let expand_enabled = self.skin_ini.as_ref().is_none_or(SkinIni::cursor_expand);
        let pressed = expand_enabled
            && ui.input(|input| {
                input.pointer.primary_down()
                    || input.pointer.secondary_down()
                    || input.key_down(egui::Key::Z)
                    || input.key_down(egui::Key::X)
            });
        let expansion =
            ui.ctx()
                .animate_bool_with_time(ui.id().with("skin-cursor-expansion"), pressed, 0.1);
        let expansion_scale = if expand_enabled {
            1.0 + 0.3 * expansion
        } else {
            1.0
        };
        let scale = (64.0 / size.x.max(size.y)).min(1.0) / ui.ctx().zoom_factor() * expansion_scale;
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        ui.ctx().request_repaint();
        ui.painter()
            .with_clip_rect(viewport.shrink(2.0).intersect(ui.clip_rect()))
            .image(
                cursor.id(),
                Rect::from_center_size(pointer, size * scale),
                uv(),
                WHITE,
            );
    }

    fn draw_combo_preview_header(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Skin.ini preview").strong().color(PINK));
        ui.label("Approach ring speed");
        ui.add(
            egui::Slider::new(&mut self.approach_duration_ms, 150.0..=1000.0)
                .suffix(" ms")
                .text("Duration"),
        );
        // Invert duration_ms = 1200 - 150 * (AR - 5).
        let approach_rate = 5.0 + (1200.0 - self.approach_duration_ms) / 150.0;
        ui.label(format!(
            "AR {approach_rate:.2} | 150 ms: fastest | 1000 ms: slowest"
        ));
        ui.label(format!(
            "Animation target: {:.0} FPS ({:.1} ms/frame)",
            1.0 / ANIMATION_INTERVAL.as_secs_f64(),
            ANIMATION_INTERVAL.as_secs_f64() * 1000.0
        ));
        ui.collapsing("Visible previews", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, visible) in COMBO_PREVIEW_LABELS
                    .iter()
                    .zip(self.combo_preview_visible.iter_mut())
                {
                    ui.checkbox(visible, *label);
                }
            });
        });
    }

    fn draw_combo_preview(&mut self, ui: &mut Ui, viewport: Rect) {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(viewport.shrink(12.0)),
            |ui| {
                ui.set_clip_rect(viewport.intersect(ui.clip_rect()));
                let ring_scale = self.approach_ring.scale(
                    self.animation.time,
                    Duration::from_secs_f32(self.approach_duration_ms / 1000.0),
                    Duration::from_millis(RING_PAUSE_MS),
                );
                let Some(ini) = &self.skin_ini else {
                    ui.label("Select a skin with a skin.ini file.");
                    return;
                };
                let Some(preview) = &self.preview else {
                    ui.label("Skin assets are unavailable.");
                    return;
                };
                let overlay = preview
                    .assets
                    .iter()
                    .find(|(name, _)| *name == "Hitcircle overlay")
                    .map(|(_, texture)| texture);
                let circle = preview
                    .assets
                    .iter()
                    .find(|(name, _)| *name == "Hitcircle")
                    .map(|(_, texture)| texture);
                if ini.colors.is_empty() {
                    ui.label("Add a Combo color in the editor to preview it.");
                }
                if !self.combo_preview_visible.iter().any(|visible| *visible) {
                    ui.label("Choose a preview under Visible previews above.");
                    return;
                }
                egui::ScrollArea::both()
                    .id_salt("skin-ini-preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for color in &ini.colors {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Combo {}   {}, {}, {}",
                                    color.number, color.rgb[0], color.rgb[1], color.rgb[2]
                                ))
                                .strong(),
                            );
                            // Hidden previews take no horizontal space. Keep an ample slot
                            // for the animated ring instead of shrinking the images.
                            let entries = [
                                Some(&preview.original_circle),
                                Some(&preview.circle),
                                preview
                                    .assets
                                    .iter()
                                    .find(|(name, _)| *name == "Approach circle")
                                    .map(|(_, texture)| texture),
                                circle,
                                overlay,
                                None,
                                None,
                            ];
                            let units: f32 = self
                                .combo_preview_visible
                                .iter()
                                .enumerate()
                                .filter(|(_, visible)| **visible)
                                .map(|(index, _)| if index >= 5 { 1.7 } else { 1.0 })
                                .sum();
                            let width = ui.available_width().max(units * 120.0);
                            let cell_width = width / units;
                            let diameter = (cell_width / 1.8).min(110.0);
                            let image_height = if self.combo_preview_visible[5..]
                                .iter()
                                .any(|visible| *visible)
                            {
                                diameter * APPROACH_START_SCALE
                            } else {
                                diameter
                            };
                            let (rect, _) = ui.allocate_exact_size(
                                vec2(width, image_height + 56.0),
                                Sense::hover(),
                            );
                            let tint = Color32::from_rgb(color.rgb[0], color.rgb[1], color.rgb[2]);
                            let asset = |label: &str| {
                                preview
                                    .assets
                                    .iter()
                                    .find(|(name, _)| *name == label)
                                    .map(|(_, texture)| texture)
                            };
                            let reference_size = circle
                                .map(|texture| texture.size_vec2())
                                .unwrap_or(vec2(128.0, 128.0));
                            let pixel_scale = diameter / reference_size.x.max(reference_size.y);
                            let mut left = rect.left();
                            for (index, texture) in entries.into_iter().enumerate() {
                                if !self.combo_preview_visible[index] {
                                    continue;
                                }
                                let slot_width = cell_width * if index >= 5 { 1.7 } else { 1.0 };
                                let center = pos2(
                                    left + slot_width / 2.0,
                                    rect.top() + image_height / 2.0 + 16.0,
                                );
                                left += slot_width;
                                ui.painter().text(
                                    pos2(center.x, rect.bottom() - 12.0),
                                    Align2::CENTER_CENTER,
                                    COMBO_PREVIEW_LABELS[index],
                                    font(11.0),
                                    WHITE,
                                );
                                if index < 5 {
                                    if let Some(texture) = texture {
                                        let size = texture.size_vec2();
                                        let scale = diameter / size.x.max(size.y);
                                        ui.painter().image(
                                            texture.id(),
                                            Rect::from_center_size(center, size * scale),
                                            uv(),
                                            tint,
                                        );
                                    } else {
                                        ui.painter().text(
                                            center,
                                            Align2::CENTER_CENTER,
                                            "Missing asset",
                                            font(11.0),
                                            WHITE,
                                        );
                                    }
                                    continue;
                                }
                                // Both animations share the ring clock and pixel scale.
                                // The after view uses the exact overlay/number assets export creates.
                                let combined_overlay = if index == 6 {
                                    Some(&preview.circle)
                                } else {
                                    overlay
                                };
                                let combined_number = if index == 6 {
                                    asset("After combo number")
                                } else {
                                    asset("Combo number")
                                };
                                let layer_bounds = |texture: &TextureHandle| {
                                    Rect::from_center_size(
                                        center,
                                        texture.size_vec2() * pixel_scale,
                                    )
                                };
                                if let Some(circle) = circle {
                                    ui.painter().image(
                                        circle.id(),
                                        layer_bounds(circle),
                                        uv(),
                                        tint,
                                    );
                                } else {
                                    ui.painter().circle_filled(center, diameter / 2.0, tint);
                                }
                                if let Some(number) = combined_number {
                                    ui.painter().image(
                                        number.id(),
                                        layer_bounds(number),
                                        uv(),
                                        WHITE,
                                    );
                                } else {
                                    ui.painter().text(
                                        center,
                                        Align2::CENTER_CENTER,
                                        "1",
                                        bold(diameter * 0.4),
                                        WHITE,
                                    );
                                }
                                if let Some(overlay) = combined_overlay {
                                    ui.painter().image(
                                        overlay.id(),
                                        layer_bounds(overlay),
                                        uv(),
                                        WHITE,
                                    );
                                }
                                if let Some(scale) = ring_scale {
                                    let ring_bounds = Rect::from_center_size(
                                        center,
                                        vec2(diameter, diameter) * scale,
                                    );
                                    if let Some(approach) = asset("Approach circle") {
                                        ui.painter().image(approach.id(), ring_bounds, uv(), tint);
                                    } else {
                                        ui.painter().circle_stroke(
                                            center,
                                            diameter * scale / 2.0,
                                            Stroke::new(2.0_f32, tint),
                                        );
                                    }
                                }
                            }
                            ui.separator();
                            ui.add_space(8.0);
                        }
                    });
            },
        );
    }

    fn draw_preview(&mut self, ui: &mut Ui, viewport: Rect) {
        let painter = ui.painter();
        painter.rect(
            viewport,
            0,
            BACKGROUND,
            Stroke::new(2.0_f32, PINK),
            StrokeKind::Inside,
        );
        let inner = viewport.shrink(2.0);
        let painter = painter.with_clip_rect(inner.intersect(painter.clip_rect()));
        let s = ((viewport.width() - 20.0) / 500.0).clamp(0.1, 1.6);
        self.animation.paint(&painter, viewport, 1.0);
        if matches!(self.active_menu, SidebarMenu::SkinIni) {
            self.draw_combo_preview(ui, inner);
            return;
        }
        let Some(preview) = &self.preview else {
            painter.text(
                viewport.center(),
                Align2::CENTER_CENTER,
                "Select a skin to preview",
                font(14.0),
                WHITE,
            );
            return;
        };
        let columns = if preview.numbered.is_some() { 3.0 } else { 2.0 };
        let content_width = (viewport.width() - 20.0).max(1.0);
        // Use the actual column count and viewport width, including wide windows.
        let asset_height = content_width / columns * 0.9 * 0.75;
        let followpoint_height = if preview.followpoints.is_empty() {
            0.0
        } else {
            96.0 * s
        };
        let content_height = asset_height + 52.0 * s + followpoint_height;
        let max_scroll = (content_height + 4.0 - viewport.height()).max(0.0);
        let max_scroll_x = (content_width + 20.0 - viewport.width()).max(0.0);
        if ui.input(|input| {
            !input.modifiers.ctrl
                && input
                    .pointer
                    .hover_pos()
                    .is_some_and(|point| viewport.contains(point))
        }) {
            let (delta, shift) =
                ui.input(|input| (input.smooth_scroll_delta, input.modifiers.shift));
            if shift {
                self.preview_scroll_x = (self.preview_scroll_x - delta.y).clamp(0.0, max_scroll_x);
            } else {
                self.preview_scroll = (self.preview_scroll - delta.y).clamp(0.0, max_scroll);
                self.preview_scroll_x = (self.preview_scroll_x - delta.x).clamp(0.0, max_scroll_x);
            }
        } else {
            self.preview_scroll = self.preview_scroll.min(max_scroll);
            self.preview_scroll_x = self.preview_scroll_x.min(max_scroll_x);
        }
        let content_left = if content_width <= viewport.width() {
            viewport.left() + (viewport.width() - content_width) / 2.0
        } else {
            viewport.left() + 10.0
        };
        let rect = Rect::from_min_size(
            pos2(
                content_left - self.preview_scroll_x,
                viewport.top() + 2.0 - self.preview_scroll,
            ),
            vec2(content_width, content_height),
        );
        for (index, (label, texture)) in [
            ("Original skin", Some(&preview.original_circle)),
            ("Edited stack", Some(&preview.circle)),
            ("Numbered", preview.numbered.as_ref()),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(texture) = texture {
                let center = pos2(
                    rect.left() + rect.width() * (index as f32 + 0.5) / columns,
                    rect.top() + 36.0 * s + asset_height / 2.0,
                );
                let natural = texture.size_vec2();
                let scale = asset_height / natural.x.max(natural.y);
                let bounds = Rect::from_center_size(center, natural * scale);
                painter.image(texture.id(), bounds, uv(), WHITE);
                painter.text(
                    pos2(center.x, bounds.top() - 8.0 * s),
                    Align2::CENTER_BOTTOM,
                    label,
                    bold(16.0 * s),
                    WHITE,
                );
            }
        }
        if !preview.followpoints.is_empty() {
            let row_top = rect.top() + asset_height + 60.0 * s;
            painter.text(
                pos2(rect.left() + 12.0 * s, row_top),
                Align2::LEFT_TOP,
                "Follow points",
                bold(14.0 * s),
                WHITE,
            );
            let count = preview.followpoints.len() as f32;
            let slot = (rect.width() - 24.0 * s) / count;
            for (position, (index, texture)) in preview.followpoints.iter().enumerate() {
                let center = pos2(
                    rect.left() + 12.0 * s + slot * (position as f32 + 0.5),
                    row_top + 43.0 * s,
                );
                let background = Rect::from_center_size(center, vec2(slot - 4.0 * s, 38.0 * s));
                painter.rect_filled(background, 2.0, Color32::from_rgb(38, 38, 38));
                let natural = texture.size_vec2();
                let scale = ((slot - 6.0 * s) / natural.x).min(36.0 * s / natural.y);
                painter.image(
                    texture.id(),
                    Rect::from_center_size(center, natural * scale),
                    uv(),
                    WHITE,
                );
                painter.text(
                    pos2(center.x, row_top + 66.0 * s),
                    Align2::CENTER_CENTER,
                    index.to_string(),
                    font(10.0 * s),
                    WHITE,
                );
            }
        }
        if max_scroll > 0.0 {
            let track = Rect::from_min_max(
                pos2(viewport.right() - 6.0, viewport.top() + 4.0),
                pos2(viewport.right() - 3.0, viewport.bottom() - 4.0),
            );
            let thumb_height = (track.height() * viewport.height() / content_height).max(18.0);
            let response = ui.interact(
                track.expand(6.0),
                ui.id().with("preview-scroll"),
                Sense::click_and_drag(),
            );
            if (response.clicked() || response.dragged())
                && let Some(pointer) = ui.input(|input| input.pointer.interact_pos())
            {
                self.preview_scroll = ((pointer.y - track.top() - thumb_height / 2.0)
                    / (track.height() - thumb_height)
                    * max_scroll)
                    .clamp(0.0, max_scroll);
            }
            let thumb_top =
                track.top() + (track.height() - thumb_height) * self.preview_scroll / max_scroll;
            painter.rect_filled(track, 2.0, Color32::from_gray(80));
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(track.left(), thumb_top),
                    vec2(track.width(), thumb_height),
                ),
                2.0,
                PINK,
            );
        }
        if max_scroll_x > 0.0 {
            let track = Rect::from_min_max(
                pos2(viewport.left() + 4.0, viewport.bottom() - 6.0),
                pos2(viewport.right() - 8.0, viewport.bottom() - 3.0),
            );
            let thumb_width = (track.width() * viewport.width() / (content_width + 20.0))
                .clamp(18.0, track.width());
            let response = ui.interact(
                track.expand(6.0),
                ui.id().with("preview-scroll-x"),
                Sense::click_and_drag(),
            );
            if (response.clicked() || response.dragged())
                && let Some(pointer) = ui.input(|input| input.pointer.interact_pos())
            {
                self.preview_scroll_x = ((pointer.x - track.left() - thumb_width / 2.0)
                    / (track.width() - thumb_width)
                    * max_scroll_x)
                    .clamp(0.0, max_scroll_x);
            }
            let thumb_left =
                track.left() + (track.width() - thumb_width) * self.preview_scroll_x / max_scroll_x;
            painter.rect_filled(track, 2.0, Color32::from_gray(80));
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(thumb_left, track.top()),
                    vec2(thumb_width, track.height()),
                ),
                2.0,
                PINK,
            );
        }
    }

    fn select_skin(&mut self, ctx: &Context, frame: &eframe::Frame) {
        let mut dialog = FileDialog::new()
            .set_parent(frame)
            .set_title("Select a skin folder or osu!lazer data folder");
        if let Some(skin) = &self.skin {
            dialog = dialog.set_directory(skin);
        }
        if let Some(folder) = dialog.pick_folder() {
            if crate::lazer::is_lazer_directory(&folder) {
                self.lazer_browser.open(Some(folder), ctx);
            } else {
                self.load_skin_path(folder, ctx);
            }
        }
    }

    fn load_skin_path(&mut self, folder: PathBuf, ctx: &Context) {
        if folder.join("client.realm").is_file() {
            self.skin = None;
            self.skin_ini = None;
            self.preview = None;
            self.ini_dirty = false;
            self.audio.set_skin(None);
            self.lazer_browser.open(Some(folder), ctx);
            return;
        }
        self.skin = Some(folder);
        self.number_font_panel = Default::default();
        self.skin_ini = None;
        self.editor_search = crate::editor_search::EditorSearch::default();
        self.ini_dirty = false;
        self.ini_error = None;
        if let Some(skin) = &self.skin {
            if !skin.join("skin.ini").is_file() {
                self.skin_ini = Some(SkinIni::from_source(String::new()));
            } else {
                match SkinIni::load(&skin.join("skin.ini")) {
                    Ok(ini) => self.skin_ini = Some(ini),
                    Err(error) => self.ini_error = Some(format!("{error:#}")),
                }
            }
        }
        self.preview_scroll = 0.0;
        self.preview_scroll_x = 0.0;
        self.update_preview(ctx);
        self.audio.set_skin(self.skin.as_deref());
        ctx.request_repaint();
    }

	fn restore_zoom(&mut self, ctx: &Context, zoom: f32) {
		self.requested_zoom = zoom.clamp(MIN_UI_ZOOM, MAX_UI_ZOOM);
		ctx.set_zoom_factor(self.requested_zoom);
	}

    fn save_project(&self, frame: &eframe::Frame, zoom: f32) {
        let Some(path) = FileDialog::new()
            .set_parent(frame)
            .set_title("Save Triple Stack project")
            .set_file_name("skin.tsp")
            .add_filter("Triple Stack project", &["tsp"])
            .save_file()
        else {
            return;
        };
        let project = crate::audio_tab::ProjectFile {
            skin: self.skin.clone(),
            copy: self.options.copy,
            resolution: self.options.force_resolution,
            use_rafis: self.options.use_rafis,
            overlay_on_top: self.options.overlay_on_top,
            source: self.skin_ini.as_ref().map(|ini| ini.source().to_owned()),
            zoom,
            audio: self.audio.project.clone(),
        };
        if let Err(error) = project.save(&path) {
            message(
                frame,
                "Save failed",
                &format!("{error:#}"),
                MessageLevel::Error,
            );
        }
    }

    fn load_project(&mut self, ctx: &Context, frame: &eframe::Frame) {
        let Some(path) = FileDialog::new()
            .set_parent(frame)
            .set_title("Load Triple Stack project")
            .add_filter("Triple Stack project", &["tsp"])
            .pick_file()
        else {
            return;
        };
        let result = (|| -> Result<()> {
            let data = std::fs::read_to_string(&path)?;
            if data.starts_with("TripleStackProject2\n") {
                let project = crate::audio_tab::ProjectFile::parse(&data, path.parent().unwrap())?;
                self.options = Options {
                    copy: project.copy,
                    force_resolution: project.resolution,
                    use_rafis: project.use_rafis,
                    overlay_on_top: project.overlay_on_top,
                };
				self.restore_zoom(ctx, project.zoom);
                if let Some(skin) = project.skin {
                    self.load_skin_path(skin, ctx);
                } else {
                    self.skin = None;
                    self.skin_ini = None;
                    self.preview = None;
                    self.ini_dirty = false;
                }
                if let Some(source) = project.source
                    && let Some(ini) = &mut self.skin_ini
                {
                    if ini.source() != source {
                        ini.set_source(source);
                        self.ini_dirty = true;
                    }
                }
                self.audio.restore(project.audio, self.skin.as_deref());
                if self.audio.has_content() {
                    self.active_menu = SidebarMenu::Audio;
                }
                ctx.request_repaint();
                return Ok(());
            }
            let mut parts = data.splitn(9, '\n');
            if parts.next() != Some("TripleStackProject1") {
                return Err(anyhow!("Unknown project format"));
            }
            let skin = PathBuf::from(parts.next().ok_or_else(|| anyhow!("Missing skin path"))?);
            if !skin.is_dir() {
                return Err(anyhow!("Skin folder does not exist: {}", skin.display()));
            }
            let copy = parts
                .next()
                .ok_or_else(|| anyhow!("Missing copy setting"))?
                .parse()?;
            let resolution: u32 = parts
                .next()
                .ok_or_else(|| anyhow!("Missing resolution"))?
                .parse()?;
            let use_rafis = parts
                .next()
                .ok_or_else(|| anyhow!("Missing circle setting"))?
                .parse()?;
            let overlay_on_top = parts
                .next()
                .ok_or_else(|| anyhow!("Missing overlay setting"))?
                .parse()?;
            let source_len: usize = parts
                .next()
                .ok_or_else(|| anyhow!("Missing source length"))?
                .parse()?;
            let zoom: f32 = parts
                .next()
                .ok_or_else(|| anyhow!("Missing zoom"))?
                .parse()?;
            anyhow::ensure!(zoom.is_finite(), "Invalid project zoom");
            let source = parts
                .next()
                .ok_or_else(|| anyhow!("Missing skin.ini text"))?;
            if source.len() != source_len {
                return Err(anyhow!("Project skin.ini text is incomplete"));
            }
            self.options = Options {
                copy,
                force_resolution: (resolution != 0).then_some(resolution),
                use_rafis,
                overlay_on_top,
            };
			self.restore_zoom(ctx, zoom);
            self.load_skin_path(skin, ctx);
            if let Some(ini) = &mut self.skin_ini {
                if ini.source() != source {
                    ini.set_source(source.to_owned());
                    self.ini_dirty = true;
                }
            }
            self.audio.restore(
                crate::audio_tab::AudioProject::default(),
                self.skin.as_deref(),
            );
            Ok(())
        })();
        if let Err(error) = result {
            message(
                frame,
                "Load failed",
                &format!("{error:#}"),
                MessageLevel::Error,
            );
        }
    }

    fn update_preview(&mut self, ctx: &Context) {
        self.preview = None;
        let Some(path) = &self.skin else { return };
        if let Ok(preview) = triplestack::preview_with_ini(
            path,
            self.options,
            self.skin_ini.as_ref().map(SkinIni::source),
        ) {
            self.preview = Some(PreviewTextures {
                original_circle: upload(ctx, "preview-original", &preview.original_circle),
                circle: upload(ctx, "preview-circle", &preview.circle),
                numbered: preview
                    .numbered
                    .as_ref()
                    .map(|image| upload(ctx, "preview-number", image)),
                followpoints: preview
                    .followpoints
                    .iter()
                    .map(|(index, image)| {
                        (*index, upload(ctx, &format!("followpoint-{index}"), image))
                    })
                    .collect(),
                assets: preview
                    .assets
                    .iter()
                    .map(|(label, image)| (*label, upload(ctx, &format!("asset-{label}"), image)))
                    .collect(),
            });
        }
    }

    fn open_export(&mut self) {
        if !matches!(self.active_menu, SidebarMenu::Export) {
            self.export_panel.suggest(ExportSelection {
                triplestack: matches!(self.active_menu, SidebarMenu::TripleStack),
                skin_ini: self.skin_ini.is_some()
                    && (self.ini_dirty || matches!(self.active_menu, SidebarMenu::SkinIni)),
                audio: self.audio.has_export_changes(),
            });
        }
        self.active_menu = SidebarMenu::Export;
    }
    fn export(&mut self, ctx: &Context, frame: &eframe::Frame) {
        let Some(skin) = self.skin.clone() else {
            return;
        };
        let audio = if self.export_panel.selection.audio {
            match self.audio.export_audio() {
                Ok(audio) => Some(audio),
                Err(error) => {
                    self.export_panel.error(format!("{error:#}"));
                    return;
                }
            }
        } else {
            None
        };
        let base = skin.file_name().unwrap_or_default().to_string_lossy();
        let suffix = if self.export_panel.selection.triplestack {
            "@3xStack"
        } else {
            "-edited"
        };
        let mut dialog = FileDialog::new()
            .set_parent(frame)
            .set_title("Export selected skin changes")
            .set_file_name(format!("{base}{suffix}.osk"))
            .add_filter("osu! skin", &["osk"]);
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            dialog = dialog.set_directory(PathBuf::from(profile).join("Downloads"));
        }
        let Some(mut output) = dialog.save_file() else {
            return;
        };
        if output.extension().is_none() {
            output.set_extension("osk");
        }
        self.export_panel.start(
            skin,
            output,
            self.options,
            self.skin_ini.as_ref().map(|ini| ini.source().to_owned()),
            audio,
            ctx,
        );
    }
}

impl eframe::App for App {
	fn raw_input_hook(&mut self, _ctx: &Context, input: &mut egui::RawInput) {
		// The backend reports the hardware limit at initialization and on device changes.
		if let Some(limit) = input.max_texture_side {
			self.device_texture_limit = limit;
		}

		let dpi = input.viewport().native_pixels_per_point.unwrap_or(1.0);
		let side = (FONT_ATLAS_POINTS * dpi * zoom_level(self.requested_zoom)).round() as usize;
		input.max_texture_side = Some(side.max(MIN_FONT_ATLAS_SIDE).min(self.device_texture_limit));
	}

    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        self.audio.poll_background(ctx);
		update_zoom(ctx, &mut self.requested_zoom);

        let sidebar_max = (ctx.available_rect().width() * 0.55).max(100.0);
        let sidebar_action = egui::SidePanel::left("editor-sidebar")
            .resizable(true)
            .min_width(250.0_f32.min(sidebar_max))
            .max_width(sidebar_max)
            .default_width(374.0_f32.min(sidebar_max))
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin {
                left: 18,
                right: 24,
                top: 18,
                bottom: 18,
            }))
            .show(ctx, |ui| {
                // Reserve a gutter so the scrollbar never overlaps the editor.
                // Leave room on both sides of the scrollbar and at the panel edge.
                let scroll = &mut ui.spacing_mut().scroll;
                scroll.floating = false;
                scroll.bar_width = 10.0;
                scroll.bar_inner_margin = 24.0;
                scroll.bar_outer_margin = 8.0;
                egui::ScrollArea::vertical()
                    .id_salt("sidebar-content")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.draw_sidebar(ui))
                    .inner
            })
            .inner;
        // Check after navigation so switching to Audio stops animation work
        // during this frame, including its mesh cache and 7 ms repaint timer.
        let minimized = ctx.input(|input| input.viewport().minimized == Some(true));
        let animation_visible =
            !minimized && !matches!(self.active_menu, SidebarMenu::Audio | SidebarMenu::Export);
        self.animation.update(ctx, !animation_visible);
        if self.motion.active() && animation_visible {
            ctx.request_repaint_after(ANIMATION_INTERVAL);
        }
        let preview_action = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(8.0))
            .show(ctx, |ui| if minimized { None } else { self.draw(ui) })
            .inner;
        match preview_action.or(sidebar_action) {
            Some(Action::Select) => self.select_skin(ctx, frame),
            Some(Action::SelectLazer) => self.lazer_browser.open(None, ctx),
            Some(Action::SelectLazerSounds(source)) => self.lazer_browser.open_sounds(source, ctx),
            Some(Action::LoadProject) => self.load_project(ctx, frame),
			Some(Action::SaveProject) => self.save_project(frame, ctx.zoom_factor()),
            Some(Action::Confirm) => self.open_export(),
            Some(Action::Export) => self.export(ctx, frame),
            None => {}
        }
        if let Some(selection) = self.lazer_browser.show(ctx) {
            if selection.sounds_only {
                self.audio.begin_folder_import(selection.folder);
            } else {
                self.load_skin_path(selection.folder, ctx);
            }
        }
    }
}

fn zoom_level(requested: f32) -> f32 {
	(requested * ZOOM_STEPS_PER_UNIT).round() / ZOOM_STEPS_PER_UNIT
}

fn update_zoom(ctx: &Context, requested: &mut f32) {
	let previous = *requested;
	ctx.input(|input| {
		if !input.modifiers.ctrl {
			return;
		}
		if input.key_pressed(egui::Key::Num0) {
			*requested = 1.0;
			return;
		}

		let zoom_in = input.key_pressed(egui::Key::Plus) || input.key_pressed(egui::Key::Equals);
		let zoom_out = input.key_pressed(egui::Key::Minus);
		let steps = i32::from(zoom_in) - i32::from(zoom_out);
		let keyboard_zoom = *requested + steps as f32 / ZOOM_STEPS_PER_UNIT;
		*requested = (keyboard_zoom * input.zoom_delta()).clamp(MIN_UI_ZOOM, MAX_UI_ZOOM);
	});

	// Accumulate fine wheel motion, but rasterize text only at reusable zoom levels.
	if *requested != previous {
		ctx.set_zoom_factor(zoom_level(*requested));
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

pub(crate) fn patterned_button(
    ui: &Ui,
    pattern_texture: egui::TextureId,
    rect: Rect,
    label: &str,
    enabled: bool,
    scale: f32,
) -> bool {
    let center = rect.center();
    let response = button(ui, rect, label, label, enabled);
    let painter = ui.painter();
    let pattern = rect.shrink(2.0 * scale);
    painter.image(pattern_texture, pattern, uv(), WHITE);
    if response.is_pointer_button_down_on() || response.hovered() {
        painter.rect_filled(pattern, 0, Color32::from_white_alpha(20));
    }
    let text_color = if enabled {
        WHITE
    } else {
        Color32::from_gray(210)
    };
    let font_size = 13.33 * scale;
    let text_width = painter
        .layout_no_wrap(label.into(), bold(font_size), text_color)
        .size()
        .x;
    let fit = ((rect.width() - 12.0 * scale) / text_width.max(1.0)).min(1.0);
    painter.text(
        center,
        Align2::CENTER_CENTER,
        label,
        bold(font_size * fit),
        text_color,
    );
    focus_ring(painter, &response, rect);
    response.clicked()
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

fn background_mesh(rect: Rect, time: f32, scale: f32) -> Mesh {
    let mut mesh = Mesh::default();
    let (width, height) = (rect.width().ceil() as u32, rect.height().ceil() as u32);
    let step = (20.0 * scale).round().max(8.0) as u32;
    let unit = step as f32 / 20.0;
    for x in (0..width).step_by(step as usize) {
        let wave = 2.0 * unit * (x as f32 / unit * 0.02 + time * (0.04 / 0.03)).sin();
        for y in (0..height).step_by(step as usize) {
            let intensity = ((x + y) as f32 / unit * 0.01 + time * (0.05 / 0.03)).sin();
            let color = if intensity > 0.2 {
                PURPLE
            } else {
                Color32::from_gray(68)
            };
            let (x, y) = (rect.left() + x as f32, rect.top() + y as f32 + wave);
            let first = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(x + 0.5 * unit, y + 19.5 * unit), color);
            mesh.colored_vertex(pos2(x + 10.0 * unit, y + 0.5 * unit), color);
            mesh.colored_vertex(pos2(x + 19.5 * unit, y + 19.5 * unit), color);
            mesh.add_triangle(first, first + 1, first + 2);
        }
    }
    mesh
}

fn upload(ctx: &Context, name: &str, rgba: &RgbaImage) -> TextureHandle {
	let size = [rgba.width() as usize, rgba.height() as usize];
	let image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());

	// Skin textures use the renderer's full resolution independently of the font atlas.
	let textures = ctx.tex_manager();
	let id = textures.write().alloc(name.to_owned(), image.into(), TextureOptions::LINEAR);
	TextureHandle::new(textures, id)
}

fn uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0))
}

fn button_pattern() -> RgbaImage {
    RgbaImage::from_fn(120, 40, |x, y| {
        if (10..110).contains(&x) && (9..31).contains(&y) {
            return image::Rgba(DEEP_PINK.to_array());
        }
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

pub(crate) fn install_fonts(ctx: &Context) {
    let mut definitions = FontDefinitions::default();
    let mut bold_family = definitions.families[&FontFamily::Proportional].clone();
    definitions.font_data.insert(
        "source-code-pro".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../Source_Code_Pro/static/SourceCodePro-Regular.ttf"
        ))),
    );
    definitions.font_data.insert(
        "source-code-pro-bold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../Source_Code_Pro/static/SourceCodePro-Bold.ttf"
        ))),
    );
    definitions
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "source-code-pro".into());
    bold_family.insert(0, "source-code-pro-bold".into());
    definitions
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "source-code-pro".into());
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

#[cfg(test)]
mod approach_tests {
    use super::*;

	#[test]
	fn fine_wheel_input_accumulates_between_zoom_levels() {
		let ctx = Context::default();
		let mut requested = 1.0;
		let input = egui::RawInput {
			modifiers: egui::Modifiers::CTRL,
			events: vec![egui::Event::Zoom(1.01)],
			..Default::default()
		};

		for _ in 0..10 {
			let _ = ctx.run(input.clone(), |ctx| update_zoom(ctx, &mut requested));
		}

		assert_eq!(ctx.zoom_factor(), 1.1);
		assert!((1.10..1.11).contains(&requested));
	}

	#[test]
	fn skin_textures_keep_their_dimensions_above_the_font_atlas_limit() {
		let ctx = Context::default();
		let width = MIN_FONT_ATLAS_SIDE + 1;
		let pixels = RgbaImage::new(width as u32, 1);
		let input = egui::RawInput { max_texture_side: Some(MIN_FONT_ATLAS_SIDE), ..Default::default() };

		let _ = ctx.run(input, |ctx| {
			let skin_texture = upload(ctx, "wide skin image", &pixels);
			assert_eq!(skin_texture.size(), [width, 1]);
		});
	}

    #[test]
    fn hidden_preview_releases_mesh_and_stops_scheduling_frames() {
        let ctx = Context::default();
        let mesh = Arc::new(Mesh::default());
        let weak = Arc::downgrade(&mesh);
        let time = Duration::from_secs(42);
        let mut animation = Animation {
            time,
            last_tick: Some(Instant::now()),
            triangles: Some((Rect::EVERYTHING, time, 1.0, mesh)),
        };
        let mut delay = Duration::ZERO;
        for _ in 0..4 {
            let output = ctx.run(egui::RawInput::default(), |ctx| animation.update(ctx, true));
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        assert_eq!(animation.time, time);
        assert!(animation.last_tick.is_none());
        assert!(animation.triangles.is_none());
        assert!(weak.upgrade().is_none());
        assert_eq!(delay, Duration::MAX);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            animation.update(ctx, false)
        });
        assert_eq!(animation.time, time); // No jump after returning to the preview.
        assert!(animation.last_tick.is_some());
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay <= ANIMATION_INTERVAL
        );
    }

    #[test]
    fn ring_renders_contact_before_pausing_even_after_a_missed_frame() {
        for milliseconds in [150, 600, 1000] {
            let mut ring = ApproachRing::default();
            let duration = Duration::from_millis(milliseconds);
            let pause = Duration::from_secs(1);
            let start = Duration::from_secs(10);
            assert_eq!(
                ring.scale(start, duration, pause),
                Some(APPROACH_START_SCALE)
            );
            let before = ring
                .scale(start + duration - Duration::from_millis(1), duration, pause)
                .unwrap();
            assert!(before > 1.0);
            let contact = start + duration + Duration::from_millis(12);
            assert_eq!(ring.scale(contact, duration, pause), Some(1.0));
            assert_eq!(ring.scale(contact, duration, pause), Some(1.0));
            assert_eq!(
                ring.scale(contact + ANIMATION_INTERVAL, duration, pause),
                None
            );
            assert_eq!(
                ring.scale(contact + pause, duration, pause),
                Some(APPROACH_START_SCALE)
            );
        }
    }

    #[test]
    fn zero_pause_still_renders_exact_contact() {
        let mut ring = ApproachRing::default();
        let duration = Duration::from_millis(150);
        let pause = Duration::ZERO;
        assert_eq!(
            ring.scale(Duration::ZERO, duration, pause),
            Some(APPROACH_START_SCALE)
        );
        assert_eq!(ring.scale(duration, duration, pause), Some(1.0));
        assert_eq!(
            ring.scale(duration + ANIMATION_INTERVAL, duration, pause),
            Some(APPROACH_START_SCALE)
        );
    }
}
