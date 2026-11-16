use anyhow::{Context, Result, ensure};
use eframe::egui::{self, Color32, Ui};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
use triplestack::{Options, beatmap_audio::audio_file, skin_ini::SkinIni};

#[derive(Clone, Copy, Default)]
pub struct ExportSelection {
    pub triplestack: bool,
    pub skin_ini: bool,
    pub audio: bool,
}
impl ExportSelection {
    pub fn any(self) -> bool {
        self.triplestack || self.skin_ini || self.audio
    }
}
pub struct AudioEdits {
    pub replace_bank: bool,
    pub files: Vec<PathBuf>,
}

/// Every export is staged separately. Cancelling or failing never processes the source skin.
pub fn export_skin(
    source: &Path,
    output: &Path,
    options: Options,
    selection: ExportSelection,
    draft: Option<&str>,
    audio: Option<&AudioEdits>,
) -> Result<()> {
    ensure!(selection.any(), "Select at least one change to export");
    ensure!(
        source.is_dir() && !source.join("client.realm").exists(),
        "Load a skin folder or a lazer skin working copy first"
    );
    if selection.skin_ini {
        ensure!(draft.is_some(), "No skin.ini draft is available");
    }
    if selection.audio {
        ensure!(
            audio.is_some_and(|audio| !audio.files.is_empty()),
            "Choose an After preset or add audio files first"
        );
    }
    let resolved_source = source.canonicalize()?;
    let output_parent = output
        .parent()
        .context("Choose an export folder")?
        .canonicalize()?;
    ensure!(
        !output_parent.starts_with(&resolved_source),
        "Save the exported skin outside its source folder"
    );
    let temporary = tempfile::Builder::new()
        .prefix("triplestack-export-")
        .tempdir()?;
    let stage = temporary.path().join("skin");
    fs::create_dir(&stage)?;
    for entry in walkdir::WalkDir::new(source)
        .min_depth(1)
        .follow_links(false)
    {
        let entry = entry?;
        ensure!(
            !entry.file_type().is_symlink(),
            "Skin contains a linked file: {}",
            entry.path().display()
        );
        let relative = entry.path().strip_prefix(source)?;
        if relative.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some(
                    ".triplestack-lazer-source.json"
                        | ".stable-source.json"
                        | ".builtin-example.json"
                        | "hitsound-preset.json"
                )
            )
        }) {
            continue;
        }
        let target = stage.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(entry.path(), target)?;
        }
    }
    if selection.skin_ini {
        let path = stage.join("skin.ini");
        if path.is_file() {
            let mut ini = SkinIni::load(&path)?;
            ini.set_source(draft.unwrap().to_owned());
            ini.save(&path)?;
        } else {
            fs::write(path, draft.unwrap())?;
        }
    }
    if selection.triplestack {
        triplestack::process(
            &stage,
            Options {
                copy: false,
                ..options
            },
        )?;
    }
    if !selection.triplestack {
        triplestack::number_fonts::ensure_complete(&stage, None)?;
    }
    if selection.audio {
        let audio = audio.unwrap();
        let mut stems = BTreeSet::new();
        for file in &audio.files {
            ensure!(
                file.is_file() && audio_file(file),
                "Audio file is missing: {}",
                file.display()
            );
            stems.insert(file.file_stem().unwrap().to_string_lossy().to_lowercase());
        }
        // Replacement banks stay independent of the old bank, just like After playback.
        // Manual overrides remove competing encodings (e.g. old WAV vs replacement OGG).
        for entry in walkdir::WalkDir::new(&stage).follow_links(false) {
            let entry = entry?;
            if entry.file_type().is_file()
                && audio_file(entry.path())
                && (audio.replace_bank
                    || stems.contains(
                        &entry
                            .path()
                            .file_stem()
                            .unwrap()
                            .to_string_lossy()
                            .to_lowercase(),
                    ))
            {
                fs::remove_file(entry.path())?;
            }
        }
        for file in &audio.files {
            fs::copy(file, stage.join(file.file_name().unwrap()))
                .with_context(|| format!("Copying hitsound {}", file.display()))?;
        }
    }
    triplestack::export_osk(&stage, output)
}

#[derive(Default)]
pub struct ExportPanel {
    pub selection: ExportSelection,
    result: Option<Result<PathBuf, String>>,
    job: Option<mpsc::Receiver<Result<PathBuf, String>>>,
}
impl ExportPanel {
    pub fn suggest(&mut self, selection: ExportSelection) {
        if self.job.is_none() {
            self.selection = selection;
            self.result = None;
        }
    }
    pub fn error(&mut self, error: impl ToString) {
        self.result = Some(Err(error.to_string()));
    }
    pub fn start(
        &mut self,
        source: PathBuf,
        output: PathBuf,
        options: Options,
        draft: Option<String>,
        audio: Option<AudioEdits>,
        ctx: &egui::Context,
    ) {
        let selection = self.selection;
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        self.result = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = export_skin(
                &source,
                &output,
                options,
                selection,
                draft.as_deref(),
                audio.as_ref(),
            )
            .map(|_| output)
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        skin: Option<&Path>,
        ini_available: bool,
        audio_available: bool,
        audio_summary: &str,
        button_pattern: egui::TextureId,
    ) -> bool {
        if !ini_available {
            self.selection.skin_ini = false;
        }
        if !audio_available {
            self.selection.audio = false;
        }
        if let Some(job) = &self.job {
            match job.try_recv() {
                Ok(result) => {
                    self.job = None;
                    self.result = Some(result);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.error("Export stopped before returning a result");
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let busy = self.job.is_some();
        let mut export = false;
        egui::ScrollArea::vertical().id_salt("export-options").auto_shrink([false, false]).show(ui, |ui| {
            ui.heading("Export skin");
            ui.label("Choose the changes to include, then export a new .osk skin file.");
            if let Some(skin) = skin {
                ui.add(egui::Label::new(format!("Target skin: {}", skin.file_name().unwrap_or_default().to_string_lossy())).truncate())
                    .on_hover_text(skin.display().to_string());
            } else { ui.label("Load a target skin from stable or lazer before exporting."); }
            ui.separator();
            ui.add_enabled_ui(!busy, |ui| {
                ui.checkbox(&mut self.selection.triplestack, "TripleStack assets");
                ui.small("Build the stacked circle overlays and combo numbers, with their required settings.");
                ui.add_space(10.0);
                ui.add_enabled_ui(ini_available, |ui| ui.checkbox(&mut self.selection.skin_ini, "skin.ini editor changes"));
                ui.small("Include your text edits, combo colours and cursor settings.");
                ui.add_space(10.0);
                ui.add_enabled_ui(audio_available, |ui| ui.checkbox(&mut self.selection.audio, "Audio / hitsound changes"));
                ui.small("Apply the active After sound preset and manually added audio to this skin.");
                ui.add(egui::Label::new(audio_summary).truncate()).on_hover_text(audio_summary);
                if !audio_available { ui.small("Choose a preset or add sounds in Audio / Hitsounds first."); }
            });
            ui.separator();
            ui.label("Your source skin and preset folders stay unchanged. Only checked changes go into the export.");
            if !self.selection.any() { ui.label("Check at least one option above."); }
            let ready = skin.is_some() && self.selection.any()
                && (!self.selection.skin_ini || ini_available) && (!self.selection.audio || audio_available);
            let (_, button_rect) = ui.allocate_space(egui::vec2(ui.available_width().min(320.0), 44.0));
            export = crate::ui::patterned_button(ui, button_pattern, button_rect, "Export skin (.osk)", !busy && ready, 1.0);
            if busy { ui.horizontal(|ui| { ui.spinner(); ui.label("Building your export..."); }); ui.ctx().request_repaint_after(Duration::from_millis(200)); }
            if let Some(result) = &self.result {
                match result {
                    Ok(path) => { ui.colored_label(Color32::LIGHT_GREEN, "Skin exported successfully.");
                        ui.add(egui::Label::new(path.display().to_string()).truncate()).on_hover_text(path.display().to_string()); }
                    Err(error) => { ui.add(egui::Label::new(egui::RichText::new(error).color(Color32::LIGHT_RED)).wrap()); }
                }
            }
        });
        export
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    fn entries(path: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        let mut files = std::collections::BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            files.insert(entry.name().to_owned(), bytes);
        }
        files
    }
    #[test]
    fn export_checkboxes_independently_control_assets_ini_and_audio() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original");
        fs::create_dir(&source).unwrap();
        image::RgbaImage::from_pixel(128, 128, image::Rgba([100, 100, 100, 255]))
            .save(source.join("hitcircle.png"))
            .unwrap();
        image::RgbaImage::from_pixel(32, 32, image::Rgba([1, 2, 3, 255]))
            .save(source.join("hitcircleoverlay.png"))
            .unwrap();
        image::RgbaImage::from_pixel(12, 20, image::Rgba([255, 255, 255, 255]))
            .save(source.join("default-1.png"))
            .unwrap();
        let original_ini = b"[General]\nName: Original\nCursorExpand: 1\n";
        fs::write(source.join("skin.ini"), original_ini).unwrap();
        fs::write(source.join("normal-hitnormal.wav"), b"old normal").unwrap();
        fs::write(source.join("normal-hitclap.wav"), b"old clap").unwrap();
        fs::write(
            source.join(".triplestack-lazer-source.json"),
            b"private source metadata",
        )
        .unwrap();
        let new_audio = directory.path().join("normal-hitnormal.ogg");
        fs::write(&new_audio, b"new normal").unwrap();
        let audio = AudioEdits {
            replace_bank: true,
            files: vec![new_audio],
        };
        let draft = "[General]\nName: Edited\nCursorExpand: 0\n";
        let original_files: Vec<_> = fs::read_dir(&source)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        for bits in 1..8 {
            let selection = ExportSelection {
                triplestack: bits & 1 != 0,
                skin_ini: bits & 2 != 0,
                audio: bits & 4 != 0,
            };
            let output = directory.path().join(format!("export-{bits}.osk"));
            export_skin(
                &source,
                &output,
                Options {
                    copy: false,
                    use_rafis: false,
                    force_resolution: None,
                    overlay_on_top: false,
                },
                selection,
                Some(draft),
                Some(&audio),
            )
            .unwrap();
            let files = entries(&output);
            let overlay = image::load_from_memory(&files["hitcircleoverlay.png"]).unwrap();
            assert_eq!(
                overlay.width(),
                if selection.triplestack { 128 } else { 32 }
            );
            let ini_path = directory.path().join("verify.ini");
            fs::write(&ini_path, &files["skin.ini"]).unwrap();
            let ini = SkinIni::load(&ini_path).unwrap();
            assert_eq!(ini.cursor_expand(), !selection.skin_ini);
            if !selection.triplestack && !selection.skin_ini {
                assert_eq!(&files["skin.ini"], original_ini);
            }
            assert_eq!(files.contains_key("normal-hitnormal.ogg"), selection.audio);
            assert_eq!(files.contains_key("normal-hitnormal.wav"), !selection.audio);
            assert_eq!(files.contains_key("normal-hitclap.wav"), !selection.audio);
            assert!(!files.contains_key(".triplestack-lazer-source.json"));
            for (path, bytes) in &original_files {
                assert_eq!(
                    fs::read(path).unwrap(),
                    *bytes,
                    "Source changed: {}",
                    path.display()
                );
            }
        }
        let output = directory.path().join("unchecked.osk");
        assert!(
            export_skin(
                &source,
                &output,
                Options::default(),
                ExportSelection::default(),
                Some(draft),
                Some(&audio)
            )
            .is_err()
        );
        assert!(!output.exists());
    }
    #[test]
    fn audio_only_needs_no_circles_and_manual_overrides_keep_other_sounds() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("skin");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("normal-hitnormal.wav"), b"old").unwrap();
        fs::write(source.join("normal-hitclap.wav"), b"keep").unwrap();
        let sample = directory.path().join("normal-hitnormal.ogg");
        fs::write(&sample, b"replacement").unwrap();
        let output = directory.path().join("audio.osk");
        let selection = ExportSelection {
            audio: true,
            ..Default::default()
        };
        export_skin(
            &source,
            &output,
            Options::default(),
            selection,
            None,
            Some(&AudioEdits {
                replace_bank: false,
                files: vec![sample],
            }),
        )
        .unwrap();
        let files = entries(&output);
        for digit in 0..10 {
            assert!(
                files.contains_key(&format!("default-{digit}.png")),
                "Missing exported fallback digit {digit}"
            );
        }
        assert_eq!(files["normal-hitnormal.ogg"], b"replacement");
        assert_eq!(files["normal-hitclap.wav"], b"keep");
        assert!(!files.contains_key("normal-hitnormal.wav"));
        assert!(!files.contains_key("hitcircleoverlay.png"));
        let before = fs::read(&output).unwrap();
        assert!(
            export_skin(
                &source,
                &output,
                Options::default(),
                selection,
                None,
                Some(&AudioEdits {
                    replace_bank: true,
                    files: vec![directory.path().join("missing.wav")]
                })
            )
            .is_err()
        );
        assert_eq!(fs::read(output).unwrap(), before);
    }
    #[test]
    fn ini_only_font_replacement_exports_all_digits_without_stacking() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path().join("skin");
        fs::create_dir(&skin).unwrap();
        let font = &triplestack::number_fonts::bundled()[0];
        let prefix = font.skin_prefix();
        let draft = format!("[Fonts]\nHitCirclePrefix: {prefix}\nScorePrefix: score\n");
        let output = directory.path().join("font-only.osk");
        export_skin(
            &skin,
            &output,
            Options::default(),
            ExportSelection {
                skin_ini: true,
                ..Default::default()
            },
            Some(&draft),
            None,
        )
        .unwrap();
        let files = entries(&output);
        for digit in 0..10 {
            assert!(image::load_from_memory(&files[&format!("{prefix}-{digit}.png")]).is_ok());
        }
        assert!(!files.contains_key("hitcircleoverlay.png"));
        assert_eq!(fs::read_dir(&skin).unwrap().count(), 0);
    }
    #[test]
    fn export_options_fit_small_and_large_viewports() {
        for size in [egui::vec2(230.0, 300.0), egui::vec2(600.0, 550.0)] {
            let ctx = egui::Context::default();
            crate::ui::install_fonts(&ctx);
            let mut panel = ExportPanel::default();
            for _ in 0..3 {
                let _=ctx.run(egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,size)),..Default::default()},|ctx|{
                    egui::CentralPanel::default().show(ctx,|ui|{
                        let bounds=ui.max_rect();panel.draw(ui,Some(Path::new("Very long skin name that must never stretch the page or hide controls")),true,true,"After: selected example sounds", egui::TextureId::Managed(0));
                        assert!(ui.min_rect().right()<=bounds.right()+1.0);assert!(ui.min_rect().bottom()<=bounds.bottom()+1.0);
                    });
                });
            }
        }
    }
}
