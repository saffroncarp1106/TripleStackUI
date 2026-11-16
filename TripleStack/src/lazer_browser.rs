use crate::lazer::{self, LazerSettings, LazerSkin};
use eframe::egui::{self, Color32, Context};
use rfd::FileDialog;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

enum JobResult {
    Catalog(Result<Vec<LazerSkin>, String>),
    Copy(Result<PathBuf, String>),
}
pub struct LazerSelection {
    pub folder: PathBuf,
    pub sounds_only: bool,
}
pub struct LazerBrowser {
    open: bool,
    sounds_only: bool,
    settings: LazerSettings,
    skins: Vec<LazerSkin>,
    query: String,
    selected: Option<usize>,
    error: Option<String>,
    note: String,
    job: Option<Receiver<JobResult>>,
    reuse_edits: bool,
}
impl Default for LazerBrowser {
    fn default() -> Self {
        Self {
            open: false,
            sounds_only: false,
            settings: LazerSettings::discover(),
            skins: Vec::new(),
            query: String::new(),
            selected: None,
            error: None,
            note: String::new(),
            job: None,
            reuse_edits: true,
        }
    }
}
impl LazerBrowser {
    pub fn open(&mut self, source: Option<PathBuf>, ctx: &Context) {
        self.open_for(source, ctx, false);
    }
    pub fn open_sounds(&mut self, source: Option<PathBuf>, ctx: &Context) {
        self.open_for(source, ctx, true);
    }
    fn open_for(&mut self, source: Option<PathBuf>, ctx: &Context, sounds_only: bool) {
        // An old background result must never load into a different import request.
        self.job = None;
        self.note.clear();
        self.error = None;
        self.sounds_only = sounds_only;
        self.open = true;
        if let Some(source) = source {
            if self.settings.source != source {
                self.skins.clear();
                self.selected = None;
                self.job = None;
            }
            self.settings.source = source;
        }
        if self.skins.is_empty()
            && self.job.is_none()
            && lazer::is_lazer_directory(&self.settings.source)
        {
            self.read(ctx);
        }
    }
    fn read(&mut self, ctx: &Context) {
        self.error = None;
        if let Err(error) = self.settings.save() {
            self.error = Some(format!("Saving lazer source settings: {error:#}"));
        }
        let settings = self.settings.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        self.note = "Reading installed lazer skins...".into();
        std::thread::spawn(move || {
            let result = lazer::read_skins(&settings).map_err(|error| format!("{error:#}"));
            let _ = tx.send(JobResult::Catalog(result));
            ctx.request_repaint();
        });
    }
    fn copy(&mut self, ctx: &Context) {
        let Some(skin) = self
            .selected
            .and_then(|index| self.skins.get(index))
            .cloned()
        else {
            return;
        };
        let source = self.settings.source.clone();
        let reuse = self.reuse_edits;
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        self.error = None;
        self.note = format!("Preparing {}...", skin.name);
        std::thread::spawn(move || {
            let result = lazer::extract_skin(&source, &skin, &lazer::cache_root(), reuse)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(JobResult::Copy(result));
            ctx.request_repaint();
        });
    }
    pub fn show(&mut self, ctx: &Context) -> Option<LazerSelection> {
        if let Some(receiver) = &self.job {
            match receiver.try_recv() {
                Ok(result) => {
                    self.job = None;
                    match result {
                        JobResult::Catalog(Ok(skins)) => {
                            self.note = format!("{} installed skins", skins.len());
                            self.skins = skins;
                            self.selected = None;
                        }
                        JobResult::Copy(Ok(folder)) if self.open => {
                            self.open = false;
                            return Some(LazerSelection {
                                folder,
                                sounds_only: self.sounds_only,
                            });
                        }
                        JobResult::Copy(Ok(_)) => {}
                        JobResult::Catalog(Err(error)) | JobResult::Copy(Err(error)) => {
                            self.error = Some(error);
                            self.note.clear();
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.error = Some("The lazer reader stopped before returning a result".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !self.open {
            return None;
        }
        let busy = self.job.is_some();
        if busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut open = self.open;
        let mut read = false;
        let mut load = false;
        egui::Window::new("osu!lazer skins").id(egui::Id::new("lazer-skin-browser"))
            .open(&mut open).default_size([650.0, 520.0]).min_height(280.0)
            .max_width((ctx.content_rect().width() - 24.0).max(100.0))
            .max_height((ctx.content_rect().height() - 24.0).max(100.0))
            .show(ctx, |ui| {
                ui.add(egui::Label::new("Select your lazer data folder, then choose an installed skin.").wrap());
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Choose lazer folder")).clicked() {
                        let mut dialog = FileDialog::new().set_title("Choose lazer data folder containing client.realm and files");
                        if self.settings.source.is_dir() { dialog = dialog.set_directory(&self.settings.source); }
                        if let Some(folder) = dialog.pick_folder() {
                            self.settings.source = folder; self.skins.clear(); self.selected = None; read = true;
                        }
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Refresh skins")).clicked() { read = true; }
                });
                ui.add(egui::Label::new(self.settings.source.display().to_string()).truncate())
                    .on_hover_text(self.settings.source.display().to_string());
                ui.collapsing("Reader dependencies", |ui| {
                    ui.label("Uses PowerShell 7 and the Realm.dll from your installed lazer application.");
                    if ui.add_enabled(!busy, egui::Button::new("Choose Realm.dll")).clicked()
                        && let Some(path) = FileDialog::new().add_filter("Realm assembly", &["dll"]).pick_file() {
                        self.settings.realm_dll = Some(path);
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Choose PowerShell 7 (pwsh.exe)")).clicked()
                        && let Some(path) = FileDialog::new().add_filter("PowerShell executable", &["exe"]).pick_file() {
                        self.settings.powershell = Some(path);
                    }
                });
                ui.horizontal(|ui| { ui.label("Search"); ui.text_edit_singleline(&mut self.query); });
                let query = self.query.to_lowercase();
                let matches: Vec<_> = self.skins.iter().enumerate().filter(|(_, skin)|
                    skin.name.to_lowercase().contains(&query) || skin.creator.to_lowercase().contains(&query)).collect();
                let height = (ui.available_height() - 105.0).max(48.0);
                egui::ScrollArea::vertical().id_salt("lazer-skin-list").max_height(height)
                    .auto_shrink([false, false]).show_rows(ui, 30.0, matches.len(), |ui, range| {
                        for position in range {
                            let (index, skin) = matches[position];
                            let label = format!("{} | {} | {} files", skin.name, skin.creator, skin.files.len());
                            let response = ui.add_enabled_ui(!busy, |ui| ui.add_sized([ui.available_width(), 30.0],
                                egui::Button::selectable(self.selected == Some(index), &label).truncate())).inner;
                            if response.on_hover_text(label).clicked() { self.selected = Some(index); }
                        }
                    });
                ui.checkbox(&mut self.reuse_edits, "Reuse the existing working copy, keeping its edits");
                let label = if self.sounds_only { "Use hitsounds from selected skin" } else { "Load selected lazer skin" };
                if ui.add_enabled(!busy && self.selected.is_some(), egui::Button::new(label)).clicked() { load = true; }
                if busy { ui.spinner(); }
                ui.add(egui::Label::new(&self.note).truncate()).on_hover_text(&self.note);
                if let Some(error) = &self.error { ui.colored_label(Color32::LIGHT_RED, error); }
                ui.label(egui::RichText::new("Skins are copied to a separate working folder before editing.").small());
            });
        self.open = open;
        if read {
            self.read(ctx);
        }
        if load {
            self.copy(ctx);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_selections_are_tagged_and_old_jobs_cannot_load_into_a_new_request() {
        let ctx = Context::default();
        let mut browser = LazerBrowser {
            settings: LazerSettings::default(),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        browser.job = Some(rx);
        tx.send(JobResult::Copy(Ok(PathBuf::from("old skin"))))
            .unwrap();
        browser.open_sounds(None, &ctx);
        assert!(browser.job.is_none());
        assert!(browser.sounds_only);
        let (tx, rx) = mpsc::channel();
        browser.job = Some(rx);
        tx.send(JobResult::Copy(Ok(PathBuf::from("new sounds"))))
            .unwrap();
        let selection = browser.show(&ctx).unwrap();
        assert!(selection.sounds_only);
        assert_eq!(selection.folder, PathBuf::from("new sounds"));
        browser.open(None, &ctx);
        assert!(!browser.sounds_only);
    }
    #[test]
    fn long_lazer_skin_names_stay_inside_the_browser_window() {
        for size in [egui::vec2(620.0, 360.0), egui::vec2(840.0, 540.0)] {
            let ctx = Context::default();
            let mut browser = LazerBrowser {
                open: true,
                settings: LazerSettings::default(),
                skins: (0..100)
                    .map(|index| LazerSkin {
                        key: format!("{index:x}"),
                        name: "Very long lazer skin name ".repeat(20),
                        creator: "Very long creator ".repeat(10),
                        instantiation: None,
                        files: Vec::new(),
                    })
                    .collect(),
                ..Default::default()
            };
            for _ in 0..3 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        browser.show(ctx);
                    },
                );
                let used = ctx.used_rect();
                assert!(
                    used.width() <= size.x + 1.0,
                    "browser width overflow: {used:?}"
                );
                assert!(
                    used.height() <= size.y + 1.0,
                    "browser height overflow: {used:?}"
                );
            }
        }
    }
}
