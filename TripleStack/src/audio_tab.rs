use crate::audio_examples::{ExampleLibrary, legacy_example_id};
use crate::audio_playback::{AudioBackend, PlaybackPlan, PlaybackStatus, ScheduledSound};
use crate::audio_presets::{CompileProgress, CompileReport, Preset, PresetLibrary};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, Ui};
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
use triplestack::beatmap_audio::{BeatmapAudio, SoundEvent, audio_file};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditionSide {
    Before,
    #[default]
    After,
}
impl AuditionSide {
    fn index(self) -> usize {
        if self == Self::Before { 0 } else { 1 }
    }
}
struct PendingImport {
    folder: PathBuf,
    name: String,
}
pub struct LazerSoundRequest {
    pub source: Option<PathBuf>,
}
enum CompileUpdate {
    Progress(CompileProgress),
    Complete(Result<CompileReport, String>),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioProject {
    pub beatmap: Option<PathBuf>,
    pub folders: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub preset: Option<PathBuf>,
    pub song_enabled: bool,
    pub music_volume: f32,
    pub hit_volume: f32,
    pub audition: AuditionSide,
    pub compare: bool,
}
impl Default for AudioProject {
    fn default() -> Self {
        Self {
            beatmap: None,
            folders: Vec::new(),
            files: Vec::new(),
            preset: None,
            song_enabled: true,
            music_volume: 0.65,
            hit_volume: 0.8,
            audition: AuditionSide::After,
            compare: true,
        }
    }
}
#[derive(Clone)]
struct ResolvedEvent {
    event: SoundEvent,
    path: Option<PathBuf>,
}
pub struct AudioTab {
    pub project: AudioProject,
    backend: AudioBackend,
    map: Option<BeatmapAudio>,
    groups: BTreeMap<PathBuf, Vec<PathBuf>>,
    skin_files: Vec<PathBuf>,
    skin: Option<PathBuf>,
    resolved: Vec<ResolvedEvent>,
    before_resolved: Vec<ResolvedEvent>,
    preset_names: BTreeMap<PathBuf, String>,
    library: PresetLibrary,
    catalog_loaded: bool,
    saved_presets: Vec<(PathBuf, String)>,
    examples_library: Option<ExampleLibrary>,
    example_presets: Vec<Preset>,
    examples_loaded: bool,
    example_job: Option<mpsc::Receiver<Result<Vec<Preset>, String>>>,
    shared_zip_cache: Option<Vec<PathBuf>>,
    pending_import: Option<PendingImport>,
    lazer_request: Option<LazerSoundRequest>,
    compile_job: Option<mpsc::Receiver<CompileUpdate>>,
    compile_progress: Option<CompileProgress>,
    compile_report: Option<CompileReport>,
    error: Option<String>,
    notice: Option<String>,
    filter: String,
    start_ms: f64,
    selected: Option<PathBuf>,
    follow_playback: bool,
    timeline_offsets: [f32; 2],
    last_follow_positions: [Option<f64>; 2],
}
impl Default for AudioTab {
    fn default() -> Self {
        let library = PresetLibrary::default();
        let examples_library = (!cfg!(test)).then(|| ExampleLibrary::beside(&library.root));
        Self {
            project: AudioProject::default(),
            backend: AudioBackend::default(),
            map: None,
            groups: BTreeMap::new(),
            skin_files: Vec::new(),
            skin: None,
            resolved: Vec::new(),
            before_resolved: Vec::new(),
            preset_names: BTreeMap::new(),
            library,
            examples_library,
            example_presets: Vec::new(),
            examples_loaded: false,
            example_job: None,
            catalog_loaded: false,
            saved_presets: Vec::new(),
            shared_zip_cache: None,
            pending_import: None,
            lazer_request: None,
            compile_job: None,
            compile_progress: None,
            compile_report: None,
            error: None,
            notice: None,
            filter: String::new(),
            start_ms: 0.0,
            selected: None,
            follow_playback: true,
            timeline_offsets: [0.0; 2],
            last_follow_positions: [None; 2],
        }
    }
}
impl AudioTab {
    pub fn has_export_changes(&self) -> bool {
        self.project.preset.is_some() || !self.project.files.is_empty()
    }
    pub fn export_summary(&self) -> String {
        let preset = self
            .project
            .preset
            .as_ref()
            .and_then(|folder| self.preset_names.get(folder))
            .map(String::as_str)
            .unwrap_or("Selected skin sounds");
        format!(
            "After: {} | {} added audio files",
            short_name(preset),
            self.project.files.len()
        )
    }
    pub fn export_audio(&self) -> Result<crate::skin_export::AudioEdits> {
        let preset = if let Some(folder) = &self.project.preset {
            self.groups.get(folder).context("The selected sound preset is not ready; wait for it to load or choose another preset")?.as_slice()
        } else {
            &[]
        };
        let manual_stems: std::collections::BTreeSet<_> = self
            .project
            .files
            .iter()
            .filter(|file| file.is_file() && audio_file(file))
            .filter_map(|file| {
                file.file_stem()
                    .map(|stem| stem.to_string_lossy().to_lowercase())
            })
            .collect();
        let mut files_by_name = BTreeMap::new();
        for file in preset
            .iter()
            .filter(|file| {
                !file.file_stem().is_some_and(|stem| {
                    manual_stems.contains(&stem.to_string_lossy().to_lowercase())
                })
            })
            .chain(&self.project.files)
        {
            if file.is_file() && audio_file(file) {
                files_by_name.insert(
                    file.file_name().unwrap().to_string_lossy().to_lowercase(),
                    file.clone(),
                );
            }
        }
        let files: Vec<_> = files_by_name.into_values().collect();
        anyhow::ensure!(
            !files.is_empty(),
            "No replacement audio is available to export"
        );
        Ok(crate::skin_export::AudioEdits {
            replace_bank: self.project.preset.is_some(),
            files,
        })
    }
    fn compile_stable(&mut self, folder: PathBuf, ctx: &egui::Context) {
        let library = self.library.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.compile_job = Some(rx);
        self.compile_report = None;
        self.compile_progress = None;
        self.error = None;
        std::thread::spawn(move || {
            let result = library
                .compile_stable(&folder, |progress| {
                    let _ = tx.send(CompileUpdate::Progress(progress));
                    ctx.request_repaint();
                })
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(CompileUpdate::Complete(result));
            ctx.request_repaint();
        });
    }
    fn compile_controls(&mut self, ui: &mut Ui) {
        let updates: Vec<_> = self
            .compile_job
            .as_ref()
            .map(|job| job.try_iter().collect())
            .unwrap_or_default();
        for update in updates {
            match update {
                CompileUpdate::Progress(progress) => self.compile_progress = Some(progress),
                CompileUpdate::Complete(result) => {
                    self.compile_job = None;
                    self.compile_progress = None;
                    match result {
                        Ok(report) => {
                            // Add catalog entries without interrupting the current audition.
                            self.reload_saved_presets();
                            self.compile_report = Some(report);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            }
        }
        if ui.add_enabled(self.compile_job.is_none(), egui::Button::new("Compile all presets from stable directory").wrap())
            .on_hover_text("Choose your osu! stable Skins folder. Creates one saved sound preset per skin; unchanged imports are skipped.")
            .clicked() {
            let mut dialog = FileDialog::new().set_title("Choose osu! stable Skins directory");
            if let Some(parent) = self.skin.as_ref().and_then(|skin| skin.parent()) { dialog = dialog.set_directory(parent); }
            if let Some(folder) = dialog.pick_folder() { self.compile_stable(folder, ui.ctx()); }
        }
        if self.compile_job.is_some() {
            ui.spinner();
            if let Some(progress) = &self.compile_progress {
                ui.add(
                    egui::ProgressBar::new(
                        progress.completed as f32 / progress.total.max(1) as f32,
                    )
                    .text(format!("{} / {}", progress.completed, progress.total)),
                );
                ui.add(egui::Label::new(&progress.current).truncate())
                    .on_hover_text(&progress.current);
            }
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        if let Some(report) = &self.compile_report {
            ui.label(format!(
                "{} added, {} unchanged, {} without audio, {} failed",
                report.created.len(),
                report.unchanged.len(),
                report.skipped.len(),
                report.errors.len()
            ));
            if !report.errors.is_empty() {
                ui.collapsing("Import errors", |ui| {
                    for error in &report.errors {
                        ui.add(egui::Label::new(error).wrap());
                    }
                });
            }
        }
    }
    pub fn begin_folder_import(&mut self, folder: PathBuf) {
        self.error = None;
        self.lazer_request = None;
        if crate::lazer::is_lazer_directory(&folder) {
            self.pending_import = None;
            self.lazer_request = Some(LazerSoundRequest {
                source: Some(folder),
            });
            return;
        }
        self.pending_import = Some(PendingImport {
            name: folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            folder,
        });
    }
    pub fn take_lazer_request(&mut self) -> Option<LazerSoundRequest> {
        self.lazer_request.take()
    }
    fn finish_folder_import(&mut self, save_copy: bool) -> Result<()> {
        let pending = self
            .pending_import
            .as_ref()
            .context("No sound folder selected")?;
        let preset = if save_copy {
            self.library.save_folder(&pending.folder, &pending.name)?
        } else {
            Preset::load(&pending.folder)?
        };
        self.pending_import = None;
        self.error = None;
        self.adopt_preset(preset);
        Ok(())
    }
    pub fn attach_context(&self, ctx: &egui::Context) {
        self.backend.attach_context(ctx);
    }
    pub fn has_content(&self) -> bool {
        self.project.beatmap.is_some()
            || !self.project.folders.is_empty()
            || !self.project.files.is_empty()
            || self.project.preset.is_some()
    }
    pub fn restore(&mut self, project: AudioProject, skin: Option<&Path>) {
        self.backend.stop();
        self.project = project;
        self.project.music_volume = self.project.music_volume.clamp(0.0, 1.0);
        self.project.hit_volume = self.project.hit_volume.clamp(0.0, 1.0);
        self.map = None;
        self.start_ms = 0.0;
        self.error = None;
        if let Some(path) = &self.project.beatmap {
            match BeatmapAudio::load(path) {
                Ok(map) => self.map = Some(map),
                Err(error) => self.error = Some(format!("{error:#}")),
            }
        }
        let preset = self.project.preset.clone();
        self.set_skin(skin);
        self.project.preset = preset;
        self.refresh();
    }
    pub fn set_skin(&mut self, skin: Option<&Path>) {
        let selected = skin.map(Path::to_owned);
        if self.skin != selected {
            self.backend.stop();
            if self.project.preset == self.skin {
                self.project.preset = None;
            }
        }
        self.skin = selected;
        self.skin_files = skin.map(files_in).unwrap_or_default();
        self.resolve();
    }
    fn refresh(&mut self) {
        self.backend.stop();
        self.catalog_loaded = true;
        self.groups.clear();
        self.preset_names.clear();
        for root in &self.project.folders {
            if legacy_example_id(root).is_some() {
                continue;
            }
            if root.join("hitsound-preset.json").is_file() {
                match Preset::load(root) {
                    Ok(preset) => {
                        self.preset_names.insert(preset.folder.clone(), preset.name);
                        self.groups.insert(preset.folder, preset.files);
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
                continue;
            }
            for entry in walkdir::WalkDir::new(root).max_depth(5).follow_links(false) {
                match entry {
                    Ok(entry) if entry.file_type().is_file() && audio_file(entry.path()) => {
                        self.groups
                            .entry(entry.path().parent().unwrap().to_owned())
                            .or_default()
                            .push(entry.into_path());
                    }
                    Ok(_) => {}
                    Err(error) => self.error = Some(format!("Cannot read sound folder: {error}")),
                }
            }
        }
        self.reload_saved_presets();
        self.shared_zip_cache = None;
        if self.project.preset.as_ref().is_some_and(|path| {
            !self.groups.contains_key(path)
                && !(!self.examples_loaded
                    && (legacy_example_id(path).is_some()
                        || self
                            .examples_library
                            .as_ref()
                            .is_some_and(|library| path.starts_with(&library.root))))
        }) {
            self.project.preset = None;
        }
        for files in self.groups.values_mut() {
            files.sort();
            files.dedup();
        }
        self.resolve();
    }
    fn resolve(&mut self) {
        let preset = self
            .project
            .preset
            .as_ref()
            .and_then(|path| self.groups.get(path))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        self.before_resolved = self
            .map
            .as_ref()
            .map(|map| resolve_events(&map.events, &[], &[], &self.skin_files))
            .unwrap_or_default();
        // A selected replacement preset is a separate sound bank, not a mix
        // with the original skin. Missing replacement sounds remain silent.
        let after_skin = if self.project.preset.is_some() {
            &[][..]
        } else {
            &self.skin_files[..]
        };
        self.resolved = self
            .map
            .as_ref()
            .map(|map| resolve_events(&map.events, &self.project.files, preset, after_skin))
            .unwrap_or_default();
    }
    fn scheduled(&self, side: AuditionSide) -> Vec<ScheduledSound> {
        let rows = if side == AuditionSide::Before {
            &self.before_resolved
        } else {
            &self.resolved
        };
        rows.iter()
            .filter_map(|row| {
                row.path.clone().map(|path| ScheduledSound {
                    time_ms: row.event.time_ms,
                    path,
                    gain: row.event.gain,
                })
            })
            .collect()
    }
    fn audition(&mut self, side: AuditionSide, start_if_stopped: bool) {
        self.project.audition = side;
        if self.backend.status().playing {
            self.backend.replace_events(self.scheduled(side));
        } else if start_if_stopped {
            self.start(self.start_ms);
        }
    }
    fn adopt_preset(&mut self, preset: Preset) {
        self.notice = Some(if preset.folder.starts_with(&self.library.root) {
            format!("Saved preset: {}", preset.name)
        } else {
            format!("Using folder for this project: {}", preset.name)
        });
        if !preset.folder.starts_with(&self.library.root)
            && !self.project.folders.contains(&preset.folder)
        {
            self.project.folders.push(preset.folder.clone());
        }
        self.refresh();
        self.groups.insert(preset.folder.clone(), preset.files);
        self.preset_names.insert(preset.folder.clone(), preset.name);
        self.project.preset = Some(preset.folder);
        self.project.audition = AuditionSide::After;
        self.project.compare = true;
        self.resolve();
    }
    fn remove_saved_preset(&mut self, folder: &Path) -> Result<()> {
        // The library validates ownership and resolved paths before deleting.
        self.library.remove(folder)?;
        self.backend.stop();
        if self.project.preset.as_deref() == Some(folder) {
            self.project.preset = None;
        }
        self.project
            .folders
            .retain(|path| !path.starts_with(folder));
        self.project.files.retain(|path| !path.starts_with(folder));
        if self
            .selected
            .as_ref()
            .is_some_and(|path| path.starts_with(folder))
        {
            self.selected = None;
        }
        self.refresh();
        self.notice = Some("Removed the saved preset copy. Shared ZIPs are kept.".into());
        self.error = None;
        Ok(())
    }
    fn load_examples(&mut self) {
        if !self.examples_loaded
            && self.example_job.is_none()
            && let Some(library) = &self.examples_library
        {
            let library = library.clone();
            let (tx, rx) = mpsc::channel();
            self.example_job = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(
                    library
                        .load()
                        .map_err(|error| format!("Loading examples: {error:#}")),
                );
            });
        }
        for preset in &self.example_presets {
            self.preset_names
                .insert(preset.folder.clone(), preset.name.clone());
            self.groups
                .insert(preset.folder.clone(), preset.files.clone());
        }
        if let Some(selected) = &self.project.preset {
            let id = legacy_example_id(selected).or_else(|| {
                self.examples_library
                    .as_ref()
                    .filter(|library| selected.starts_with(&library.root))
                    .and_then(|_| selected.file_name())
                    .map(|id| id.to_string_lossy().into_owned())
            });
            if let Some(id) = id
                && let Some(example) = self.example_presets.iter().find(|preset| {
                    preset
                        .folder
                        .file_name()
                        .is_some_and(|folder| folder == id.as_str())
                })
            {
                self.project.preset = Some(example.folder.clone());
            }
        }
    }
    pub fn poll_background(&mut self, ctx: &egui::Context) {
        let result = self.example_job.as_ref().map(|job| job.try_recv());
        match result {
            Some(Ok(result)) => {
                self.example_job = None;
                self.examples_loaded = true;
                match result {
                    Ok(presets) => {
                        self.example_presets = presets;
                        self.load_examples();
                        self.resolve();
                        self.audition(self.project.audition, false);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.example_job = None;
                self.examples_loaded = true;
                self.error = Some("Example installer stopped before returning a result".into());
            }
            _ => {}
        }
        if self.example_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
    fn poll_examples(&mut self, ui: &mut Ui) {
        self.poll_background(ui.ctx());
        if self.example_job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparing built-in examples...");
            });
        }
    }
    fn example_picker(&mut self, ui: &mut Ui, id: &str) {
        let current = self
            .example_presets
            .iter()
            .find(|preset| self.project.preset.as_ref() == Some(&preset.folder));
        let label = current
            .map(|p| short_name(&p.name))
            .unwrap_or_else(|| "Choose an example".into());
        let mut selection = None;
        egui::ComboBox::from_id_salt(id)
            .width(ui.available_width())
            .height(180.0)
            .truncate()
            .selected_text(label)
            .show_ui(ui, |ui| {
                let row_height = ui.spacing().interact_size.y.max(24.0);
                egui::ScrollArea::vertical()
                    .id_salt(id)
                    .max_height(180.0)
                    .auto_shrink([false, true])
                    .show_rows(ui, row_height, self.example_presets.len(), |ui, range| {
                        for index in range {
                            let preset = &self.example_presets[index];
                            if ui
                                .add_sized(
                                    [ui.available_width(), row_height],
                                    egui::Button::selectable(
                                        self.project.preset.as_ref() == Some(&preset.folder),
                                        short_name(&preset.name),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&preset.name)
                                .clicked()
                            {
                                selection = Some(preset.folder.clone());
                                ui.close();
                            }
                        }
                    });
            });
        if let Some(folder) = selection {
            self.project.preset = Some(folder);
            self.resolve();
            self.audition(AuditionSide::After, false);
        }
    }
    fn reload_saved_presets(&mut self) {
        self.load_examples();
        match self.library.list() {
            Ok(presets) => {
                self.saved_presets.clear();
                for preset in presets {
                    self.saved_presets
                        .push((preset.folder.clone(), short_name(&preset.name)));
                    self.preset_names.insert(preset.folder.clone(), preset.name);
                    self.groups.insert(preset.folder, preset.files);
                }
            }
            Err(error) => self.error = Some(format!("Loading saved presets: {error:#}")),
        }
    }
    fn preset_workspace(&mut self, ui: &mut Ui) {
        ui.collapsing("Presets / shared ZIPs", |ui| self.workspace_contents(ui));
    }
    fn workspace_contents(&mut self, ui: &mut Ui) {
        ui.add(egui::Label::new(short_name(&self.library.root.display().to_string())).truncate())
            .on_hover_text(self.library.root.display().to_string());
        ui.horizontal_wrapped(|ui| {
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(self.library.root.display().to_string());
            }
            if ui.button("Refresh list").clicked() {
                if self.example_presets.is_empty() {
                    self.examples_loaded = false;
                }
                self.reload_saved_presets();
                self.shared_zip_cache = None;
            }
            if ui
                .add_enabled(
                    self.project.preset.is_some(),
                    egui::Button::new("Create share ZIP"),
                )
                .clicked()
                && let Some(folder) = &self.project.preset
            {
                match self.library.create_share_zip(folder) {
                    Ok(path) => {
                        self.shared_zip_cache = None;
                        ui.ctx().copy_text(path.display().to_string());
                        self.notice = Some(format!("Created ZIP; path copied: {}", path.display()));
                        self.error = None;
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
        });
        ui.small("The x removes a saved copy or ZIP.");
        let mut select = None;
        let mut remove = None;
        let row_height = ui.spacing().interact_size.y.max(24.0);
        let list_height = (ui.ctx().content_rect().height() * 0.23).clamp(72.0, 180.0);
        ui.label(format!("Personal presets ({})", self.saved_presets.len()));
        egui::ScrollArea::vertical()
            .id_salt("workspace-presets")
            .max_height(list_height)
            .auto_shrink([false, true])
            .show_rows(ui, row_height, self.saved_presets.len(), |ui, range| {
                for index in range {
                    let (folder, label) = &self.saved_presets[index];
                    ui.push_id(folder, |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), row_height),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .small_button("x")
                                    .on_hover_text("Delete saved preset copy")
                                    .clicked()
                                {
                                    remove = Some(folder.clone());
                                }
                                if ui
                                    .add_sized(
                                        [ui.available_width(), row_height],
                                        egui::Button::selectable(
                                            self.project.preset.as_ref() == Some(folder),
                                            label,
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(
                                        self.preset_names
                                            .get(folder)
                                            .map(String::as_str)
                                            .unwrap_or(label),
                                    )
                                    .clicked()
                                {
                                    select = Some(folder.clone());
                                }
                            },
                        );
                    });
                }
            });
        if let Some(folder) = select {
            self.project.preset = Some(folder);
            self.resolve();
            self.audition(AuditionSide::After, false);
        }
        if let Some(folder) = remove
            && let Err(error) = self.remove_saved_preset(&folder)
        {
            self.error = Some(format!("{error:#}"));
        }
        if !self.example_presets.is_empty() {
            ui.separator();
            ui.label(format!("Examples ({})", self.example_presets.len()));
            self.example_picker(ui, "workspace-example-picker");
            let example = self
                .example_presets
                .iter()
                .find(|preset| self.project.preset.as_ref() == Some(&preset.folder))
                .cloned();
            if ui
                .add_enabled(
                    example.is_some(),
                    egui::Button::new("Copy example to personal").truncate(),
                )
                .clicked()
                && let Some(example) = example
            {
                let name = example
                    .name
                    .strip_prefix("Preset-")
                    .unwrap_or(&example.name);
                match self.library.save_folder(&example.folder, name) {
                    Ok(preset) => self.adopt_preset(preset),
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
            ui.small("Built-in examples are kept with the app.");
        }
        if self.shared_zip_cache.is_none() {
            match self.library.shared_zips() {
                Ok(paths) => self.shared_zip_cache = Some(paths),
                Err(error) => {
                    self.error = Some(format!("{error:#}"));
                    self.shared_zip_cache = Some(Vec::new());
                }
            }
        }
        let paths = self.shared_zip_cache.as_deref().unwrap_or_default();
        ui.label(format!("Shared ZIPs ({})", paths.len()));
        let mut remove_zip = None;
        egui::ScrollArea::vertical()
            .id_salt("workspace-zips")
            .max_height(list_height.min(112.0))
            .auto_shrink([false, true])
            .show_rows(ui, row_height, paths.len(), |ui, range| {
                for index in range {
                    let path = &paths[index];
                    ui.push_id(path, |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), row_height),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .small_button("x")
                                    .on_hover_text("Delete shared ZIP only")
                                    .clicked()
                                {
                                    remove_zip = Some(path.clone());
                                }
                                if ui
                                    .small_button("Copy")
                                    .on_hover_text("Copy ZIP path")
                                    .clicked()
                                {
                                    ui.ctx().copy_text(path.display().to_string());
                                }
                                ui.add_sized(
                                    [ui.available_width(), row_height],
                                    egui::Label::new(short_name(
                                        &path.file_name().unwrap_or_default().to_string_lossy(),
                                    ))
                                    .truncate(),
                                )
                                .on_hover_text(path.display().to_string());
                            },
                        );
                    });
                }
            });
        if let Some(path) = remove_zip {
            match self.library.remove_shared_zip(&path) {
                Ok(()) => {
                    self.shared_zip_cache = None;
                    self.notice = Some("Removed shared ZIP. Saved presets are kept.".into());
                    self.error = None;
                }
                Err(error) => self.error = Some(format!("{error:#}")),
            }
        }
    }
    fn import_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &mut self.pending_import else {
            return;
        };
        let mut action = 0;
        egui::Window::new("Save this folder as a preset?").id(egui::Id::new("save-audio-preset"))
            .collapsible(false).resizable(false).default_width(380.0).show(ctx, |ui| {
                ui.add(egui::Label::new(pending.folder.display().to_string()).truncate()).on_hover_text(pending.folder.display().to_string());
                ui.label("Save a reusable copy of these audio files, or use the folder for this project only.");
                ui.label("Preset name"); ui.text_edit_singleline(&mut pending.name);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!pending.name.trim().is_empty(), egui::Button::new("Save reusable preset")).clicked() { action = 1; }
                    if ui.button("Use folder this time").clicked() { action = 2; }
                    if ui.button("Cancel").clicked() { action = 3; }
                });
                if let Some(error) = &self.error { ui.colored_label(Color32::LIGHT_RED, error); }
            });
        if action == 0 {
            return;
        }
        if action == 3 {
            self.pending_import = None;
            return;
        }
        if let Err(error) = self.finish_folder_import(action == 1) {
            self.error = Some(format!("{error:#}"));
        }
    }
    fn load_map(&mut self, path: PathBuf) {
        match BeatmapAudio::load(&path) {
            Ok(map) => {
                self.backend.stop();
                self.start_ms = 0.0;
                self.map = Some(map);
                self.project.beatmap = Some(path);
                self.error = None;
                self.resolve();
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }
    fn start(&mut self, from_ms: f64) {
        let Some(map) = &self.map else {
            return;
        };
        let song = if self.project.song_enabled {
            let Some(song) = map.song.as_ref().filter(|path| path.is_file()) else {
                self.error = Some("The beatmap's song file is missing. Place it beside the .osu file or turn off Play song.".into());
                return;
            };
            Some(song.clone())
        } else {
            None
        };
        self.error = None;
        self.follow_playback = true;
        self.last_follow_positions = [None; 2];
        self.backend.play(PlaybackPlan {
            song,
            events: self.scheduled(self.project.audition),
            from_ms: from_ms.clamp(0.0, map.duration_ms),
            end_ms: map.duration_ms,
            music_volume: self.project.music_volume,
            hit_volume: self.project.hit_volume,
        });
    }
    pub fn sidebar(&mut self, ui: &mut Ui) {
        if !self.catalog_loaded {
            self.refresh();
        }
        self.poll_examples(ui);
        ui.heading("Hitsound library");
        ui.small("Apply these sounds to a skin with Confirm > Audio / hitsound changes.");
        self.compile_controls(ui);
        if ui.button("Load .osu beatmap").clicked() {
            let mut dialog = FileDialog::new().add_filter("osu! beatmap", &["osu"]);
            if let Some(map) = &self.project.beatmap {
                dialog = dialog.set_directory(map.parent().unwrap());
            }
            if let Some(path) = dialog.pick_file() {
                self.load_map(path);
            }
        }
        if ui
            .add(egui::Button::new("Add sounds / preset").truncate())
            .on_hover_text("Choose a skin folder or a folder of audio files")
            .clicked()
            && let Some(folder) = FileDialog::new()
                .set_title("Choose a folder of replacement hitsounds")
                .pick_folder()
        {
            self.begin_folder_import(folder);
        }
        if ui
            .add(egui::Button::new("Add sounds from lazer skin").truncate())
            .clicked()
        {
            self.pending_import = None;
            self.lazer_request = Some(LazerSoundRequest { source: None });
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Import preset ZIP").clicked()
                && let Some(path) = FileDialog::new()
                    .add_filter("Hitsound preset", &["zip"])
                    .pick_file()
            {
                match self.library.import_zip(&path) {
                    Ok(preset) => {
                        self.error = None;
                        self.adopt_preset(preset);
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
            if ui
                .add_enabled(
                    self.project.preset.is_some(),
                    egui::Button::new("Share preset ZIP"),
                )
                .clicked()
                && let Some(folder) = &self.project.preset
                && let Some(path) = FileDialog::new()
                    .set_file_name("hitsounds.zip")
                    .add_filter("Hitsound preset", &["zip"])
                    .save_file()
            {
                let path = if path.extension().is_none() {
                    path.with_extension("zip")
                } else {
                    path
                };
                match self.library.export_zip(folder, &path) {
                    Ok(()) => {
                        self.error = None;
                        self.notice = Some(format!("Exported preset to {}", path.display()));
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
        });
        if ui.button("Add audio files").clicked()
            && let Some(paths) = FileDialog::new()
                .add_filter("Audio", &["wav", "ogg", "mp3", "flac"])
                .pick_files()
        {
            for path in paths {
                if !self.project.files.contains(&path) {
                    self.project.files.push(path);
                }
            }
            self.resolve();
        }
        ui.add_space(8.0);
        if let Some(skin) = &self.skin {
            ui.add(
                egui::Label::new(format!(
                    "Selected skin: {}",
                    skin.file_name().unwrap_or_default().to_string_lossy()
                ))
                .truncate(),
            )
            .on_hover_text(skin.display().to_string());
        }
        ui.label("Active sound preset");
        self.preset_picker(ui, "audio-preset");
        self.preset_workspace(ui);
        ui.label(
            egui::RichText::new(
                "Saved presets stay available for future beatmaps. Export a ZIP to share the copied audio files.",
            )
            .small(),
        );
        ui.collapsing("Manage sources", |ui| {
            let mut remove_folder = None;
            for (index, path) in self.project.folders.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("x").clicked() {
                        remove_folder = Some(index);
                    }
                    ui.add(egui::Label::new(path.display().to_string()).truncate())
                        .on_hover_text(path.display().to_string());
                });
            }
            let mut remove_file = None;
            for (index, path) in self.project.files.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("x").clicked() {
                        remove_file = Some(index);
                    }
                    ui.add(
                        egui::Label::new(path.file_name().unwrap_or_default().to_string_lossy())
                            .truncate(),
                    )
                    .on_hover_text(path.display().to_string());
                });
            }
            if let Some(index) = remove_folder {
                self.project.folders.remove(index);
                self.refresh();
            }
            if let Some(index) = remove_file {
                self.project.files.remove(index);
                self.resolve();
            }
        });
        ui.separator();
        if ui
            .checkbox(&mut self.project.song_enabled, "Play song with hitsounds")
            .changed()
        {
            let status = self.backend.status();
            if status.playing {
                self.start(status.position_ms);
                if status.paused {
                    self.backend.pause(true);
                }
            }
        }
        let changed = ui
            .add(egui::Slider::new(&mut self.project.music_volume, 0.0..=1.0).text("Song volume"))
            .changed()
            | ui.add(
                egui::Slider::new(&mut self.project.hit_volume, 0.0..=1.0).text("Hitsound volume"),
            )
            .changed();
        if changed {
            self.backend
                .volumes(self.project.music_volume, self.project.hit_volume);
        }
        if ui.button("Refresh files").clicked() {
            self.refresh();
            self.skin_files = self.skin.as_deref().map(files_in).unwrap_or_default();
            self.resolve();
        }
        if let Some(map) = &self.map {
            ui.separator();
            ui.add(egui::Label::new(&map.title).truncate())
                .on_hover_text(&map.title);
            ui.label(format!(
                "{} objects / {} sound events",
                map.objects,
                map.events.len()
            ));
            let missing = self
                .resolved
                .iter()
                .filter(|row| row.path.is_none())
                .count();
            let before_missing = self
                .before_resolved
                .iter()
                .filter(|row| row.path.is_none())
                .count();
            ui.label(format!(
                "Before: {before_missing} missing | After: {missing} missing"
            ));
            ui.label(egui::RichText::new("Missing or unreadable samples are ignored. Playback continues without substituting other sounds.").small());
        }
        if let Some(notice) = &self.notice {
            ui.add(
                egui::Label::new(egui::RichText::new(notice).color(Color32::LIGHT_GREEN))
                    .truncate(),
            )
            .on_hover_text(notice);
        }
        self.import_dialog(ui.ctx());
    }
    fn preset_picker(&mut self, ui: &mut Ui, id_salt: &str) {
        let previous = self.project.preset.clone();
        egui::ComboBox::from_id_salt(id_salt)
            .width(ui.available_width())
            .truncate()
            .selected_text(
                self.project
                    .preset
                    .as_ref()
                    .map(|path| {
                        self.preset_names.get(path).cloned().unwrap_or_else(|| {
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        })
                    })
                    .map(|name| short_name(&name))
                    .unwrap_or_else(|| "Selected skin sounds".into()),
            )
            .show_ui(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                ui.selectable_value(&mut self.project.preset, None, "Selected skin sounds");
                ui.label("Personal folders / presets");
                for path in self.groups.keys().filter(|path| {
                    !self
                        .example_presets
                        .iter()
                        .any(|preset| &preset.folder == *path)
                }) {
                    ui.selectable_value(
                        &mut self.project.preset,
                        Some(path.clone()),
                        short_name(&self.preset_names.get(path).cloned().unwrap_or_else(|| {
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        })),
                    )
                    .on_hover_text(path.display().to_string());
                }
            });
        if previous != self.project.preset {
            self.resolve();
            self.audition(AuditionSide::After, false);
        }
        if !self.example_presets.is_empty() {
            ui.label("Examples");
            self.example_picker(ui, &format!("{id_salt}-examples"));
        }
    }
    fn library_files(&self) -> Vec<PathBuf> {
        let mut files = self.project.files.clone();
        if let Some(preset) = &self.project.preset
            && let Some(preset_files) = self.groups.get(preset)
        {
            files.extend(preset_files.iter().cloned());
        }
        files.extend(self.skin_files.iter().cloned());
        files.sort();
        files.dedup();
        files
    }
    pub fn draw(&mut self, ui: &mut Ui) {
        let status = self.backend.status();
        if status.playing
            && !status.paused
            && !ui
                .ctx()
                .input(|input| input.viewport().minimized == Some(true))
        {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(if self.follow_playback {
                    16
                } else {
                    30
                }));
        }
        // Keep controls bounded at small sizes so comparison timelines always
        // have their own viewport instead of extending the application page.
        let controls_height = (ui.available_height() * 0.45).clamp(1.0, 300.0);
        egui::ScrollArea::vertical()
            .id_salt("audio-controls")
            .max_height(controls_height)
            .auto_shrink([false, true])
            .show(ui, |ui| self.draw_audio_controls(ui, &status));
        ui.separator();
        let filter = self.filter.to_ascii_lowercase();
        self.draw_timeline(ui, &status, &filter);
    }
    fn draw_audio_controls(&mut self, ui: &mut Ui, status: &PlaybackStatus) {
        ui.heading("Audio preview");
        ui.add(
            egui::Label::new("Play Before / Play After switches sounds on the same song clock.")
                .truncate(),
        );
        ui.horizontal_wrapped(|ui| {
            for (side, label) in [
                (AuditionSide::Before, "Play Before"),
                (AuditionSide::After, "Play After"),
            ] {
                if ui
                    .add_enabled(
                        self.map.is_some(),
                        egui::Button::new(label)
                            .selected(status.playing && self.project.audition == side),
                    )
                    .clicked()
                {
                    self.audition(side, true);
                }
            }
            if ui
                .add_enabled(
                    status.playing,
                    egui::Button::new(if status.paused { "Resume" } else { "Pause" }),
                )
                .clicked()
            {
                self.backend.pause(!status.paused);
            }
            if ui.button("Stop").clicked() {
                self.backend.stop();
            }
            if ui
                .add_enabled(self.map.is_some(), egui::Button::new("Map preview time"))
                .clicked()
            {
                if let Some(map) = &self.map {
                    self.start_ms = map.preview_ms;
                }
                self.start(self.start_ms);
            }
        });
        if let Some(map) = &self.map {
            ui.add(
                egui::Slider::new(&mut self.start_ms, 0.0..=map.duration_ms).text("Start at (ms)"),
            );
            ui.label(format!(
                "Position: {}{}",
                timestamp(status.position_ms),
                if status.paused { " (paused)" } else { "" }
            ));
            ui.add(
                egui::ProgressBar::new((status.position_ms / map.duration_ms.max(1.0)) as f32)
                    .show_percentage(),
            );
        }
        if let Some(error) = self.error.as_ref().or(status.error.as_ref()) {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if !status.warnings.is_empty() {
            ui.collapsing(
                format!(
                    "{} sample warning(s) - preview continues",
                    status.warnings.len()
                ),
                |ui| {
                    for warning in &status.warnings {
                        ui.colored_label(Color32::YELLOW, warning);
                    }
                },
            );
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.filter);
        });
        let files = self.library_files();
        let filter = self.filter.to_ascii_lowercase();
        let files: Vec<_> = files
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&filter)
            })
            .collect();
        egui::CollapsingHeader::new(format!("Audio files ({})", files.len()))
            .id_salt("audio-library")
            .show(ui, |ui| {
                let height = (ui.available_height() * 0.25).clamp(48.0, 160.0);
                egui::ScrollArea::vertical()
                    .id_salt("audio-files")
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show_rows(ui, 24.0, files.len(), |ui, range| {
                        for index in range {
                            let path = &files[index];
                            let bank = if self.skin_files.contains(path) {
                                "Before"
                            } else {
                                "After"
                            };
                            let label = format!(
                                "{bank}: {}",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            );
                            if ui
                                .add_sized(
                                    [ui.available_width(), 24.0],
                                    egui::Button::selectable(
                                        self.selected.as_ref() == Some(path),
                                        label,
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                self.selected = Some(path.clone());
                                self.backend.preview(path.clone(), self.project.hit_volume);
                            }
                        }
                    });
            });
    }

    fn draw_timeline(&mut self, ui: &mut Ui, status: &PlaybackStatus, filter: &str) {
        let has_after = self.project.preset.is_some() || !self.project.files.is_empty();
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Hitsound timelines").strong());
            ui.checkbox(&mut self.follow_playback, "Follow playback");
            if has_after {
                ui.checkbox(&mut self.project.compare, "Compare");
            }
        });
        let size = ui.available_size();
        if has_after && self.project.compare && size.x >= 680.0 {
            ui.columns(2, |columns| {
                self.draw_timeline_pane(&mut columns[0], status, filter, AuditionSide::Before);
                self.draw_timeline_pane(&mut columns[1], status, filter, AuditionSide::After);
            });
        } else if has_after && self.project.compare && size.y >= 240.0 {
            let pane_height = (size.y - ui.spacing().item_spacing.y) / 2.0;
            for side in [AuditionSide::Before, AuditionSide::After] {
                ui.allocate_ui_with_layout(
                    egui::vec2(size.x, pane_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.draw_timeline_pane(ui, status, filter, side),
                );
            }
        } else {
            if has_after && self.project.compare {
                ui.horizontal(|ui| {
                    for side in [AuditionSide::Before, AuditionSide::After] {
                        if ui
                            .selectable_label(self.project.audition == side, format!("{side:?}"))
                            .clicked()
                        {
                            self.audition(side, false);
                        }
                    }
                });
            }
            self.draw_timeline_pane(ui, status, filter, self.project.audition);
        }
    }
    fn draw_timeline_pane(
        &mut self,
        ui: &mut Ui,
        status: &PlaybackStatus,
        filter: &str,
        side: AuditionSide,
    ) {
        if side == AuditionSide::After {
            self.preset_picker(ui, "timeline-after-preset");
        } else {
            ui.add_sized(
                [ui.available_width(), ui.spacing().interact_size.y],
                egui::Label::new("Original skin sounds").truncate(),
            );
        }
        let source = if side == AuditionSide::Before {
            self.skin
                .as_ref()
                .map(|p| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_else(|| "Select a skin".into())
        } else {
            self.project
                .preset
                .as_ref()
                .map(|p| {
                    self.preset_names.get(p).cloned().unwrap_or_else(|| {
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    })
                })
                .unwrap_or_else(|| {
                    if self.project.files.is_empty() {
                        "Selected skin".into()
                    } else {
                        "Imported audio files".into()
                    }
                })
        };
        let title = format!("{side:?} - {source}");
        ui.add(egui::Label::new(egui::RichText::new(&title).strong()).truncate())
            .on_hover_text(&title);
        let side_index = side.index();
        let source_rows = if side == AuditionSide::Before {
            &self.before_resolved
        } else {
            &self.resolved
        };
        let rows: Vec<_> = source_rows
            .iter()
            .filter(|row| row.event.sample.to_ascii_lowercase().contains(filter))
            .collect();
        let current = if status.playing {
            rows.partition_point(|row| row.event.time_ms <= status.position_ms)
                .checked_sub(1)
        } else {
            None
        };
        let active_time = current.map(|index| rows[index].event.time_ms);
        let row_height = 24.0;
        let pitch = row_height + ui.spacing().item_spacing.y;
        let available = ui.available_rect_before_wrap();
        let height = available.height().max(1.0);
        let manual_scroll = ui.input(|input| {
            input.pointer.hover_pos().is_some_and(|pointer| {
                available.contains(pointer)
                    && (input.smooth_scroll_delta.y != 0.0
                        || (input.pointer.primary_down() && pointer.x >= available.right() - 16.0))
            })
        });
        if manual_scroll {
            self.follow_playback = false;
        }
        let mut area = egui::ScrollArea::vertical()
            .id_salt(("audio-events", side_index))
            .max_height(height)
            .auto_shrink([false, false]);
        if self.follow_playback && status.playing && !status.paused {
            let target = current
                .map(|current| {
                    ((current as f32 + 0.5) * pitch - height * 0.35)
                        .clamp(0.0, (rows.len() as f32 * pitch - height).max(0.0))
                })
                .unwrap_or(0.0);
            let jump = self.last_follow_positions[side_index]
                .is_none_or(|last| (status.position_ms - last).abs() > 1000.0);
            let dt = ui.input(|input| input.stable_dt);
            self.timeline_offsets[side_index] =
                follow_offset(self.timeline_offsets[side_index], target, dt, jump);
            area = area.vertical_scroll_offset(self.timeline_offsets[side_index]);
        }
        self.last_follow_positions[side_index] = Some(status.position_ms);
        let output = area.show_rows(ui, row_height, rows.len(), |ui, range| {
            for index in range {
                let row = rows[index];
                let highlighted =
                    active_time.is_some_and(|time| (row.event.time_ms - time).abs() < 0.001);
                let label = format!(
                    "{}  {}  {}  {}%{}",
                    timestamp(row.event.time_ms),
                    row.event.kind,
                    row.event.sample,
                    (row.event.gain * 100.0).round(),
                    if row.path.is_none() {
                        "  [missing]"
                    } else {
                        ""
                    }
                );
                let button = egui::Button::new(&label).selected(highlighted).truncate();
                let response = ui
                    .add_enabled_ui(row.path.is_some(), |ui| {
                        ui.add_sized([ui.available_width(), row_height], button)
                    })
                    .inner;
                if response
                    .on_hover_text(format!(
                        "{label}\n{}",
                        row.path
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "Missing from selected audio sources".into())
                    ))
                    .clicked()
                    && let Some(path) = &row.path
                {
                    self.backend
                        .preview(path.clone(), row.event.gain * self.project.hit_volume);
                }
            }
        });
        self.timeline_offsets[side_index] = output.state.offset.y;
    }
}
fn follow_offset(current: f32, target: f32, dt: f32, jump: bool) -> f32 {
    if jump {
        return target;
    }
    let blend = 1.0 - (-dt.clamp(0.0, 0.1) / 0.12).exp();
    let next = current + (target - current) * blend;
    if (next - target).abs() < 0.5 {
        target
    } else {
        next
    }
}

fn timestamp(ms: f64) -> String {
    let value = ms.max(0.0).round() as u64;
    format!(
        "{:02}:{:02}.{:03}",
        value / 60000,
        value / 1000 % 60,
        value % 1000
    )
}
fn short_name(name: &str) -> String {
    let mut chars = name.chars();
    let mut label: String = chars.by_ref().take(48).collect();
    if chars.next().is_some() {
        label.push('\u{2026}');
    }
    label
}

fn files_in(root: &Path) -> Vec<PathBuf> {
    let mut files = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.is_file() && audio_file(path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    files.sort();
    files
}
fn find(files: &[PathBuf], name: &str) -> Option<PathBuf> {
    for extension in ["wav", "ogg", "mp3", "flac"] {
        let filename = if Path::new(name).extension().is_some() {
            name.to_owned()
        } else {
            format!("{name}.{extension}")
        };
        if let Some(path) = files.iter().rev().find(|path| {
            path.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&filename))
                && path.is_file()
        }) {
            return Some(path.clone());
        }
    }
    None
}
fn resolve_events(
    events: &[SoundEvent],
    manual: &[PathBuf],
    preset: &[PathBuf],
    skin: &[PathBuf],
) -> Vec<ResolvedEvent> {
    let mut cache = std::collections::HashMap::new();
    events
        .iter()
        .map(|event| {
            let key = (event.sample.clone(), event.fallback.clone(), event.custom);
            let path = cache
                .entry(key)
                .or_insert_with(|| resolve_sample(event, manual, preset, skin))
                .clone();
            ResolvedEvent {
                event: event.clone(),
                path,
            }
        })
        .collect()
}

fn resolve_sample(
    event: &SoundEvent,
    manual: &[PathBuf],
    preset: &[PathBuf],
    skin: &[PathBuf],
) -> Option<PathBuf> {
    // Beatmaps define when to play a sound, never the hitsound source here.
    find(manual, &event.sample)
        .or_else(|| find(preset, &event.sample))
        .or_else(|| find(skin, &event.sample))
        .or_else(|| {
            if event.custom {
                None
            } else {
                find(manual, &event.fallback)
                    .or_else(|| find(preset, &event.fallback))
                    .or_else(|| find(skin, &event.fallback))
            }
        })
}

pub fn validate_project_paths(project: &mut AudioProject, root: &Path) {
    let resolve = |path: &mut PathBuf| {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    };
    if let Some(path) = &mut project.beatmap {
        resolve(path);
    }
    if let Some(path) = &mut project.preset {
        resolve(path);
    }
    for path in project.folders.iter_mut().chain(project.files.iter_mut()) {
        resolve(path);
    }
}

#[derive(Serialize, Deserialize)]
pub struct ProjectFile {
    pub skin: Option<PathBuf>,
    pub copy: bool,
    pub resolution: Option<u32>,
    pub use_rafis: bool,
    pub overlay_on_top: bool,
    pub source: Option<String>,
    pub zoom: f32,
    pub audio: AudioProject,
}
impl ProjectFile {
    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(
            path,
            format!(
                "TripleStackProject2\n{}",
                serde_json::to_string_pretty(self)?
            ),
        )
        .with_context(|| format!("Saving {}", path.display()))
    }
    pub fn parse(text: &str, root: &Path) -> Result<Self> {
        let json = text
            .strip_prefix("TripleStackProject2\n")
            .context("Unknown project format")?;
        let mut project: Self = serde_json::from_str(json)?;
        anyhow::ensure!(
            project.zoom.is_finite()
                && project.audio.music_volume.is_finite()
                && project.audio.hit_volume.is_finite(),
            "Invalid project settings"
        );
        anyhow::ensure!(
            project
                .resolution
                .is_none_or(|value| (100..=160).contains(&value)),
            "Invalid circle resolution"
        );
        if let Some(skin) = &mut project.skin {
            if skin.is_relative() {
                *skin = root.join(&*skin);
            }
            anyhow::ensure!(skin.is_dir(), "Skin folder is missing: {}", skin.display());
        }
        validate_project_paths(&mut project.audio, root);
        Ok(project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exported_audio_uses_after_sources_and_manual_overrides_without_map_audio() {
        let directory = tempfile::tempdir().unwrap();
        let preset = directory.path().join("preset");
        let manual = directory.path().join("manual");
        let original = directory.path().join("before");
        for folder in [&preset, &manual, &original] {
            std::fs::create_dir(folder).unwrap();
        }
        std::fs::write(preset.join("normal-hitnormal.wav"), b"preset normal").unwrap();
        std::fs::write(preset.join("normal-hitclap.wav"), b"preset clap").unwrap();
        std::fs::write(preset.join("normal-hitclap.ogg"), b"alternate clap").unwrap();
        let override_file = manual.join("normal-hitnormal.ogg");
        std::fs::write(&override_file, b"manual normal").unwrap();
        std::fs::write(original.join("normal-hitwhistle.wav"), b"original whistle").unwrap();
        let mut tab = AudioTab {
            library: PresetLibrary {
                root: directory.path().join("library"),
            },
            ..Default::default()
        };
        tab.set_skin(Some(&original));
        tab.adopt_preset(Preset::load(&preset).unwrap());
        tab.project.files.push(override_file.clone());
        tab.project.audition = AuditionSide::Before;
        let export = tab.export_audio().unwrap();
        assert!(export.replace_bank);
        assert_eq!(export.files.len(), 3);
        assert!(export.files.contains(&override_file));
        assert!(!export.files.contains(&preset.join("normal-hitnormal.wav")));
        assert!(
            export
                .files
                .iter()
                .all(|file| file.starts_with(&preset) || file.starts_with(&manual))
        );
        tab.project.preset = None;
        let export = tab.export_audio().unwrap();
        assert!(!export.replace_bank);
        assert_eq!(export.files, vec![override_file]);
    }
    #[test]
    fn example_installation_preserves_old_projects_and_personal_presets_stay_separate() {
        let directory = tempfile::tempdir().unwrap();
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../assets/audio-examples.json")).unwrap();
        let id = catalog["presets"][0]["id"].as_str().unwrap();
        let library = PresetLibrary {
            root: directory.path().join("AudioPresets"),
        };
        let legacy = library.root.join(id);
        let example_root = directory.path().join("ExamplePresets");
        let example = example_root.join("bundle").join(id);
        for folder in [&legacy, &example] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("normal-hitnormal.wav"), b"sample").unwrap();
            std::fs::write(
                folder.join("hitsound-preset.json"),
                serde_json::json!({"version":1,"name":id,"files":["normal-hitnormal.wav"]})
                    .to_string(),
            )
            .unwrap();
        }
        std::fs::write(
            legacy.join(crate::audio_examples::LEGACY_MARKER),
            serde_json::json!({"id":id}).to_string(),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let mut tab = AudioTab {
            library,
            examples_library: Some(ExampleLibrary { root: example_root }),
            example_job: Some(rx),
            ..Default::default()
        };
        tab.restore(
            AudioProject {
                preset: Some(legacy.clone()),
                folders: vec![legacy.clone()],
                ..Default::default()
            },
            None,
        );
        assert_eq!(tab.project.preset.as_ref(), Some(&legacy));
        assert!(tab.saved_presets.is_empty());
        tx.send(Ok(vec![Preset::load(&example).unwrap()])).unwrap();
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    tab.poll_examples(ui);
                    tab.workspace_contents(ui);
                });
            },
        );
        assert_eq!(tab.project.preset.as_ref(), Some(&example));
        assert!(tab.groups.contains_key(&example));
        assert!(tab.remove_saved_preset(&example).is_err());
        assert!(tab.remove_saved_preset(&legacy).is_err());
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some((&text.galley.job.text, text.pos.y))
                } else {
                    None
                }
            })
            .collect();
        let personal_y = texts
            .iter()
            .find(|(text, _)| text.starts_with("Personal presets"))
            .unwrap()
            .1;
        let examples_y = texts
            .iter()
            .find(|(text, _)| text.starts_with("Examples ("))
            .unwrap()
            .1;
        assert!(personal_y < examples_y);
        assert!(
            !texts.iter().any(|(text, _)| text.as_str() == "x"),
            "Examples must not have a delete button"
        );
        let copy = tab
            .library
            .save_folder(&example, "My personal sounds")
            .unwrap();
        let copy_folder = copy.folder.clone();
        tab.adopt_preset(copy);
        assert_eq!(tab.saved_presets.len(), 1);
        assert_eq!(tab.saved_presets[0].0, copy_folder);
        tab.remove_saved_preset(&copy_folder).unwrap();
        assert!(example.join("normal-hitnormal.wav").is_file());
        assert!(legacy.join("normal-hitnormal.wav").is_file());
    }
    #[test]
    fn workspace_rows_remain_compact_and_visible_during_scrolling() {
        fn visible_rows(output: &egui::FullOutput, prefix: &str) -> Vec<(String, egui::Pos2)> {
            output
                .shapes
                .iter()
                .filter_map(|clipped| {
                    if let egui::Shape::Text(text) = &clipped.shape {
                        let label = &text.galley.job.text;
                        let center = text.pos + text.galley.size() * 0.5;
                        if label.starts_with(prefix) && clipped.clip_rect.contains(center) {
                            return Some((label.clone(), text.pos));
                        }
                    }
                    None
                })
                .collect()
        }
        let ctx = egui::Context::default();
        let mut tab = AudioTab::default();
        tab.saved_presets = (0..30)
            .map(|i| {
                (
                    PathBuf::from(format!("preset-{i}")),
                    format!("Preset scroll row {i}"),
                )
            })
            .collect();
        tab.shared_zip_cache = Some(
            (0..30)
                .map(|i| PathBuf::from(format!("ZIP scroll row {i}.zip")))
                .collect(),
        );
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(740.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| tab.workspace_contents(ui));
                },
            )
        };
        for _ in 0..3 {
            frame(Vec::new());
        }
        for prefix in ["Preset scroll row", "ZIP scroll row"] {
            let initial = visible_rows(&frame(Vec::new()), prefix);
            assert!(
                initial.len() >= 3,
                "Only {} {prefix} entries visible",
                initial.len()
            );
            let pointer = initial[1].1 + egui::vec2(8.0, 5.0);
            frame(vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -95.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            for _ in 0..8 {
                frame(Vec::new());
            }
            let scrolled = visible_rows(&frame(Vec::new()), prefix);
            assert!(
                scrolled.len() >= 3,
                "Only {} {prefix} entries visible after scrolling",
                scrolled.len()
            );
            assert_ne!(initial[0].0, scrolled[0].0, "List did not scroll");
            for rows in [&initial, &scrolled] {
                for pair in rows.windows(2) {
                    let spacing = pair[1].1.y - pair[0].1.y;
                    assert!(
                        (24.0..=30.0).contains(&spacing),
                        "Unexpected row spacing: {spacing}"
                    );
                }
            }
        }
    }
    #[test]
    fn expanded_workspace_keeps_large_catalogs_bounded_and_uses_cached_entries() {
        let directory = tempfile::tempdir().unwrap();
        for count in [60, 10000] {
            for size in [egui::vec2(240.0, 540.0), egui::vec2(400.0, 600.0)] {
                let ctx = egui::Context::default();
                let mut tab = AudioTab {
                    library: PresetLibrary {
                        root: directory.path().join("not-on-disk"),
                    },
                    ..Default::default()
                };
                for index in 0..count {
                    let name = format!("{index} {}", "Long skin name ".repeat(100));
                    tab.saved_presets
                        .push((PathBuf::from(format!("preset-{index}")), short_name(&name)));
                }
                tab.shared_zip_cache = Some(
                    (0..count)
                        .map(|index| {
                            PathBuf::from(format!("{index}-{}.zip", "long-name".repeat(100)))
                        })
                        .collect(),
                );
                for _ in 0..3 {
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let bounds = ui.max_rect();
                                tab.workspace_contents(ui);
                                assert!(
                                    ui.min_rect().right() <= bounds.right() + 1.0,
                                    "workspace width overflow"
                                );
                                assert!(
                                    ui.min_rect().bottom() <= bounds.bottom() + 1.0,
                                    "workspace height overflow"
                                );
                            });
                        },
                    );
                    assert!(
                        output.shapes.len() < 400,
                        "offscreen entries were rendered: {}",
                        output.shapes.len()
                    );
                    assert!(tab.error.is_none());
                    assert_eq!(tab.saved_presets.len(), count);
                    assert_eq!(tab.shared_zip_cache.as_ref().unwrap().len(), count);
                }
            }
        }
        assert_eq!(short_name(&"\u{65e5}".repeat(100)).chars().count(), 49);
    }

    #[test]
    fn skin_folder_import_and_removal_update_the_catalog_and_preserve_before() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path().join("before");
        let replacement = directory.path().join("replacement");
        for folder in [&skin, &replacement] {
            std::fs::create_dir(folder).unwrap();
        }
        let original = skin.join("normal-hitnormal.wav");
        std::fs::write(&original, b"before").unwrap();
        let replacement_sound = replacement.join("normal-hitnormal.wav");
        std::fs::write(&replacement_sound, b"after").unwrap();
        std::fs::write(replacement.join("hitcircle.png"), b"image excluded").unwrap();
        let map = directory.path().join("map.osu");
        std::fs::write(
            &map,
            "osu file format v14\n[HitObjects]\n0,0,1000,1,0,0:0:0:0:\n",
        )
        .unwrap();
        let mut tab = AudioTab {
            library: PresetLibrary {
                root: directory.path().join("workspace"),
            },
            ..Default::default()
        };
        tab.set_skin(Some(&skin));
        tab.load_map(map);
        tab.begin_folder_import(replacement.clone());
        assert!(tab.take_lazer_request().is_none());
        tab.finish_folder_import(true).unwrap();
        let saved = tab.project.preset.clone().unwrap();
        assert!(tab.saved_presets.iter().any(|(folder, _)| folder == &saved));
        assert_eq!(tab.groups[&saved].len(), 1);
        assert_eq!(tab.skin.as_ref(), Some(&skin));
        assert_eq!(tab.before_resolved[0].path.as_ref(), Some(&original));
        let sample = tab.resolved[0].path.clone().unwrap();
        assert_eq!(std::fs::read(&sample).unwrap(), b"after");
        let zip = tab.library.create_share_zip(&saved).unwrap();
        tab.selected = Some(sample.clone());
        tab.project.files.push(sample);
        tab.project.folders.push(saved.clone());
        tab.remove_saved_preset(&saved).unwrap();
        assert!(!saved.exists());
        assert!(!tab.saved_presets.iter().any(|(folder, _)| folder == &saved));
        assert!(zip.exists());
        assert!(tab.project.preset.is_none());
        assert!(tab.selected.is_none());
        assert!(tab.project.files.is_empty() && tab.project.folders.is_empty());
        assert!(!tab.groups.contains_key(&saved) && !tab.preset_names.contains_key(&saved));
        assert_eq!(tab.resolved[0].path.as_ref(), Some(&original));
        assert_eq!(std::fs::read(&replacement_sound).unwrap(), b"after");
        tab.begin_folder_import(replacement.clone());
        tab.finish_folder_import(false).unwrap();
        assert_eq!(tab.project.preset.as_ref(), Some(&replacement));
        assert!(tab.remove_saved_preset(&replacement).is_err());
        assert!(replacement_sound.exists());
    }

    #[test]
    fn lazer_data_folders_open_the_audio_skin_browser_instead_of_scanning_hashes() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("client.realm"), []).unwrap();
        std::fs::create_dir(directory.path().join("files")).unwrap();
        let mut tab = AudioTab::default();
        tab.begin_folder_import(directory.path().to_owned());
        let request = tab.take_lazer_request().unwrap();
        assert_eq!(request.source.as_deref(), Some(directory.path()));
        assert!(tab.take_lazer_request().is_none());
        assert!(tab.pending_import.is_none());
        assert!(tab.project.preset.is_none() && tab.project.folders.is_empty());
    }

    #[test]
    fn portable_preset_resolves_separate_before_after_and_survives_new_maps() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path().join("skin");
        let source = directory.path().join("replacement");
        std::fs::create_dir(&skin).unwrap();
        std::fs::create_dir(&source).unwrap();
        let original = skin.join("normal-hitnormal.wav");
        std::fs::write(&original, b"original sound").unwrap();
        std::fs::write(skin.join("normal-hitclap.wav"), b"original clap").unwrap();
        std::fs::write(source.join("normal-hitnormal.wav"), b"new sound").unwrap();
        let library = PresetLibrary {
            root: directory.path().join("library"),
        };
        let preset = library.save_folder(&source, "New sound pack").unwrap();
        let folder = preset.folder.clone();
        let mut tab = AudioTab {
            library: library.clone(),
            ..Default::default()
        };
        tab.set_skin(Some(&skin));
        tab.adopt_preset(preset);
        for (filename, time) in [("first.osu", 1000.0), ("second.osu", 2000.0)] {
            let map = directory.path().join(filename);
            std::fs::write(
                &map,
                format!("osu file format v14\n[HitObjects]\n0,0,{time},1,8,0:0:0:0:\n"),
            )
            .unwrap();
            tab.load_map(map);
            assert_eq!(tab.project.preset.as_ref(), Some(&folder));
            assert_eq!(tab.before_resolved[0].path.as_ref(), Some(&original));
            assert_eq!(
                tab.resolved[0].event.time_ms,
                tab.before_resolved[0].event.time_ms
            );
            assert!(tab.resolved[0].path.as_ref().unwrap().starts_with(&folder));
            assert_eq!(
                std::fs::read(tab.resolved[0].path.as_ref().unwrap()).unwrap(),
                b"new sound"
            );
            // Missing replacement claps must never be filled by the original skin.
            assert!(tab.before_resolved[1].path.is_some());
            assert!(tab.resolved[1].path.is_none());
            assert_eq!(tab.scheduled(AuditionSide::Before).len(), 2);
            assert_eq!(tab.scheduled(AuditionSide::After).len(), 1);
        }
        let mut next_session = AudioTab {
            library,
            ..Default::default()
        };
        next_session.refresh();
        assert!(next_session.groups.contains_key(&folder));
        assert_eq!(next_session.preset_names[&folder], "New sound pack");
    }

    #[test]
    fn comparison_panes_share_a_playhead_and_fit_narrow_and_wide_windows() {
        for size in [egui::vec2(420.0, 500.0), egui::vec2(1000.0, 600.0)] {
            let ctx = egui::Context::default();
            let mut tab = AudioTab::default();
            tab.project.preset = Some(PathBuf::from("New preset"));
            tab.resolved = (0..1000)
                .map(|index| ResolvedEvent {
                    event: SoundEvent {
                        time_ms: index as f64 * 100.0,
                        sample: "normal-hitnormal".into(),
                        fallback: "normal-hitnormal".into(),
                        custom: false,
                        map_sample: true,
                        gain: 1.0,
                        kind: "Circle",
                    },
                    path: None,
                })
                .collect();
            tab.before_resolved = tab.resolved.clone();
            let status = PlaybackStatus {
                playing: true,
                position_ms: 50000.0,
                ..Default::default()
            };
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let bounds = ui.max_rect();
                            tab.draw_timeline(ui, &status, "");
                            assert!(ui.min_rect().right() <= bounds.right() + 1.0);
                            assert!(ui.min_rect().bottom() <= bounds.bottom() + 1.0);
                        });
                    },
                );
            }
            assert!(tab.timeline_offsets.iter().all(|offset| *offset > 10000.0));
            assert!((tab.timeline_offsets[0] - tab.timeline_offsets[1]).abs() < 1.0);
        }
    }

    #[test]
    fn follow_motion_is_smooth_and_seeks_land_immediately() {
        let mut offset = 0.0;
        for _ in 0..60 {
            let next = follow_offset(offset, 1000.0, 1.0 / 60.0, false);
            assert!(next >= offset && next <= 1000.0);
            offset = next;
        }
        assert!((offset - 1000.0).abs() < 0.5);
        assert_eq!(follow_offset(offset, 25000.0, 1.0 / 60.0, true), 25000.0);
    }

    #[test]
    fn long_skin_names_and_large_timeline_stay_inside_window() {
        for size in [
            egui::vec2(620.0, 360.0),
            egui::vec2(840.0, 540.0),
            egui::vec2(1400.0, 900.0),
        ] {
            let ctx = egui::Context::default();
            let mut tab = AudioTab::default();
            let long = "A very long skin name ".repeat(30);
            tab.catalog_loaded = true;
            tab.skin = Some(PathBuf::from(&long));
            tab.project.preset = Some(PathBuf::from(&long));
            tab.map = Some(BeatmapAudio {
                path: PathBuf::from("test.osu"),
                title: long,
                song: None,
                preview_ms: 0.0,
                duration_ms: 100000.0,
                events: Vec::new(),
                objects: 1000,
            });
            tab.resolved = (0..1000)
                .map(|index| ResolvedEvent {
                    event: SoundEvent {
                        time_ms: index as f64 * 100.0,
                        sample: "normal-hitnormal".into(),
                        fallback: "normal-hitnormal".into(),
                        custom: false,
                        map_sample: true,
                        gain: 1.0,
                        kind: "Circle",
                    },
                    path: None,
                })
                .collect();
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        let sidebar = egui::SidePanel::left("test-sidebar")
                            .default_width(330.0)
                            .max_width(size.x * 0.55)
                            .show(ctx, |ui| {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| tab.sidebar(ui));
                            });
                        assert!(
                            sidebar.response.rect.width() <= size.x * 0.55 + 1.0,
                            "long name expanded sidebar"
                        );
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let bounds = ui.max_rect();
                            tab.draw(ui);
                            assert!(
                                ui.min_rect().right() <= bounds.right() + 1.0,
                                "audio width overflow at {size:?}"
                            );
                            assert!(
                                ui.min_rect().bottom() <= bounds.bottom() + 1.0,
                                "audio height overflow at {size:?}: {:?} vs {bounds:?}",
                                ui.min_rect()
                            );
                        });
                    },
                );
            }
        }
    }

    #[test]
    fn timeline_follows_playback_and_holds_position_when_paused() {
        let ctx = egui::Context::default();
        let mut tab = AudioTab::default();
        tab.resolved = (0..1000)
            .map(|index| ResolvedEvent {
                event: SoundEvent {
                    time_ms: index as f64 * 100.0,
                    sample: "normal-hitnormal".into(),
                    fallback: "normal-hitnormal".into(),
                    custom: false,
                    map_sample: true,
                    gain: 1.0,
                    kind: "Circle",
                },
                path: None,
            })
            .collect();
        let mut status = PlaybackStatus {
            playing: true,
            position_ms: 50000.0,
            ..Default::default()
        };
        let frame = |tab: &mut AudioTab, status: &PlaybackStatus| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| tab.draw_timeline(ui, status, ""));
                },
            );
        };
        for _ in 0..3 {
            frame(&mut tab, &status);
        }
        let initial = tab.timeline_offsets[1];
        assert!(initial > 10000.0);
        status.position_ms += 100.0;
        frame(&mut tab, &status);
        assert!(tab.timeline_offsets[1] > initial);
        let paused = tab.timeline_offsets[1];
        status.paused = true;
        frame(&mut tab, &status);
        assert_eq!(tab.timeline_offsets[1], paused);
        status.paused = false;
        status.position_ms = -1.0;
        frame(&mut tab, &status);
        assert_eq!(tab.timeline_offsets[1], 0.0);
    }
    #[test]
    fn skin_change_preserves_explicit_preset_but_clears_old_skin_preset() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        let preset = dir.path().join("preset");
        for path in [&first, &second, &preset] {
            std::fs::create_dir(path).unwrap();
        }
        std::fs::write(preset.join("normal-hitnormal.wav"), []).unwrap();
        let mut tab = AudioTab::default();
        tab.restore(
            AudioProject {
                folders: vec![preset.clone()],
                preset: Some(preset.clone()),
                ..Default::default()
            },
            Some(&first),
        );
        assert_eq!(tab.project.preset, Some(preset.clone()));
        tab.set_skin(Some(&second));
        assert_eq!(tab.project.preset, Some(preset));
        assert_eq!(tab.skin.as_deref(), Some(second.as_path()));
        tab.project.preset = Some(second);
        tab.set_skin(Some(&first));
        assert!(tab.project.preset.is_none());
    }
    #[test]
    #[ignore = "requires TRIPLESTACK_TEST_SONGS and TRIPLESTACK_TEST_SKIN"]
    fn songs_directory_and_skin_fallbacks_are_playable() {
        let root = PathBuf::from(std::env::var_os("TRIPLESTACK_TEST_SONGS").unwrap());
        let skin = PathBuf::from(std::env::var_os("TRIPLESTACK_TEST_SKIN").unwrap());
        let skin_files = files_in(&skin);
        let mut loaded = 0;
        let mut other_modes = 0;
        let mut errors = Vec::new();
        let mut samples = std::collections::BTreeSet::new();
        let mut missing_custom = 0;
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.path().extension().is_none_or(|ext| ext != "osu") {
                continue;
            }
            match BeatmapAudio::load(entry.path()) {
                Ok(map) => {
                    loaded += 1;
                    if let Some(song) = &map.song {
                        match std::fs::File::open(song)
                            .map_err(anyhow::Error::from)
                            .and_then(|file| {
                                rodio::Decoder::try_from(file).map_err(anyhow::Error::from)
                            }) {
                            Ok(mut decoder) => {
                                assert!(decoder.next().is_some());
                            }
                            Err(error) => errors.push(format!("{}: {error}", song.display())),
                        }
                    }
                    for event in &map.events {
                        if let Some(path) = resolve_sample(event, &[], &[], &skin_files) {
                            samples.insert(path);
                        } else {
                            missing_custom += 1;
                        }
                    }
                }
                Err(error) if error.to_string().contains("Mode 0") => other_modes += 1,
                Err(error) => errors.push(format!("{}: {error:#}", entry.path().display())),
            }
        }
        let mut ignored = 0;
        for path in &samples {
            let (sample, warning) = crate::audio_playback::load_sample(path);
            if sample.is_none() {
                ignored += 1;
                assert!(warning.is_some());
            }
        }
        println!(
            "Loaded {loaded} standard difficulties; {other_modes} other modes; {} unique samples; {ignored} unreadable samples ignored; {missing_custom} unresolved custom events",
            samples.len()
        );
        assert!(loaded > 0);
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }
    #[test]
    fn skin_only_resolution_ignores_conflicting_beatmap_files() {
        let dir = tempfile::tempdir().unwrap();
        let map_root = dir.path().join("map");
        let skin_root = dir.path().join("skin");
        for root in [&map_root, &skin_root] {
            std::fs::create_dir(root).unwrap();
        }
        let map_file = map_root.join("soft-hitnormal12.wav");
        let custom_file = map_root.join("custom.wav");
        let skin_file = skin_root.join("soft-hitnormal.wav");
        for path in [&map_file, &custom_file, &skin_file] {
            std::fs::write(path, []).unwrap();
        }
        let map = map_root.join("test.osu");
        std::fs::write(&map, "osu file format v14\n[General]\nAudioFilename: song.mp3\n[HitObjects]\n0,0,1000,1,0,2:0:12:0:\n0,0,2000,1,0,0:0:0:0:custom.wav\n").unwrap();
        let mut tab = AudioTab::default();
        tab.restore(
            AudioProject {
                beatmap: Some(map),
                ..Default::default()
            },
            Some(&skin_root),
        );
        assert_eq!(tab.resolved[0].path.as_ref(), Some(&skin_file));
        assert!(tab.resolved[1].path.is_none());
        assert_eq!(tab.library_files(), vec![skin_file.clone()]);
        // Numbered variants supplied by the skin are honored too.
        let numbered = skin_root.join("soft-hitnormal12.wav");
        std::fs::write(&numbered, []).unwrap();
        tab.set_skin(Some(&skin_root));
        assert_eq!(tab.resolved[0].path.as_ref(), Some(&numbered));
        std::fs::remove_file(&skin_file).unwrap();
        std::fs::remove_file(&numbered).unwrap();
        tab.set_skin(Some(&skin_root));
        assert!(tab.resolved.iter().all(|row| row.path.is_none()));
        assert!(map_file.is_file());
    }

    #[test]
    fn explicitly_imported_audio_can_override_skin_samples() {
        let dir = tempfile::tempdir().unwrap();
        let skin = dir.path().join("soft-hitnormal.wav");
        let manual = dir.path().join("soft-hitnormal12.wav");
        for path in [&skin, &manual] {
            std::fs::write(path, []).unwrap();
        }
        let event = SoundEvent {
            time_ms: 0.0,
            sample: "soft-hitnormal12".into(),
            fallback: "soft-hitnormal".into(),
            custom: false,
            map_sample: true,
            gain: 1.0,
            kind: "Circle",
        };
        assert_eq!(
            resolve_sample(&event, &[manual.clone()], &[], &[skin.clone()]),
            Some(manual)
        );
        assert_eq!(
            resolve_sample(&event, &[], &[], &[skin.clone()]),
            Some(skin)
        );
        assert!(resolve_sample(&event, &[], &[], &[]).is_none());
    }
    #[test]
    fn project_roundtrip_preserves_audio_only_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.tsp");
        let project = ProjectFile {
            skin: None,
            copy: true,
            resolution: Some(160),
            use_rafis: true,
            overlay_on_top: false,
            source: None,
            zoom: 1.0,
            audio: AudioProject {
                beatmap: Some(PathBuf::from("map/test.osu")),
                folders: vec![PathBuf::from("sounds")],
                files: vec![PathBuf::from("clap.wav")],
                preset: Some(PathBuf::from("sounds")),
                music_volume: 0.3,
                ..Default::default()
            },
        };
        project.save(&path).unwrap();
        let loaded =
            ProjectFile::parse(&std::fs::read_to_string(&path).unwrap(), dir.path()).unwrap();
        assert_eq!(
            loaded.audio.beatmap.unwrap(),
            dir.path().join("map/test.osu")
        );
        assert_eq!(loaded.audio.files[0], dir.path().join("clap.wav"));
        assert_eq!(loaded.audio.preset.unwrap(), dir.path().join("sounds"));
        assert_eq!(loaded.audio.music_volume, 0.3);
        assert!(loaded.skin.is_none());
    }
    #[test]
    #[ignore = "requires TRIPLESTACK_TEST_BEATMAP"]
    fn provided_beatmap_audio_files_decode_and_timeline_resolves() {
        let path = PathBuf::from(
            std::env::var_os("TRIPLESTACK_TEST_BEATMAP").expect("set TRIPLESTACK_TEST_BEATMAP"),
        );
        let map = BeatmapAudio::load(&path).unwrap();
        let root = path.parent().unwrap();
        let files = files_in(root);
        use rodio::Source;
        for path in &files {
            let mut decoder = rodio::Decoder::try_from(std::fs::File::open(path).unwrap()).unwrap();
            assert!(decoder.channels() > 0 && decoder.sample_rate() > 0);
            assert!(decoder.next().is_some(), "{} is empty", path.display());
        }
        let skin_files = std::env::var_os("TRIPLESTACK_TEST_SKIN")
            .map(|path| files_in(Path::new(&path)))
            .unwrap_or_default();
        let resolved_files: Vec<_> = map
            .events
            .iter()
            .filter_map(|event| resolve_sample(event, &[], &[], &skin_files))
            .collect();
        if !skin_files.is_empty() {
            assert_eq!(resolved_files.len(), map.events.len());
        }
        let used: std::collections::BTreeSet<_> = resolved_files.iter().collect();
        for path in &used {
            let (sample, warning) = crate::audio_playback::load_sample(path);
            assert!(sample.is_some() || warning.is_some());
        }
        let resolved = resolved_files.len();
        let mut tab = AudioTab::default();
        tab.restore(
            AudioProject {
                beatmap: Some(path.clone()),
                ..Default::default()
            },
            std::env::var_os("TRIPLESTACK_TEST_SKIN")
                .as_deref()
                .map(Path::new),
        );
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::SidePanel::left("audio-test-sidebar").show(ctx, |ui| tab.sidebar(ui));
                egui::CentralPanel::default().show(ctx, |ui| tab.draw(ui));
            },
        );
        println!("Resolved sample files: {used:?}");
        assert!(map.objects > 1000);
        assert!(
            map.events
                .windows(2)
                .all(|pair| pair[0].time_ms <= pair[1].time_ms)
        );
        assert!(map.song.as_ref().unwrap().is_file());
        println!(
            "{} objects, {} events, {} resolved with beatmap and selected skin, {} decodable audio files",
            map.objects,
            map.events.len(),
            resolved,
            files.len()
        );
    }
}
