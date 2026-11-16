use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use triplestack::beatmap_audio::audio_file;

const MANIFEST: &str = "hitsound-preset.json";
const MAX_FILES: usize = 4096;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const STABLE_SOURCE: &str = ".stable-source.json";
#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct StableSource {
    directory: PathBuf,
    digest: String,
}
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct CompileReport {
    pub created: Vec<CompiledPreset>,
    pub unchanged: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct CompiledPreset {
    pub name: String,
    pub folder: PathBuf,
    pub sounds: usize,
}
pub struct CompileProgress {
    pub completed: usize,
    pub total: usize,
    pub current: String,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    name: String,
    files: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub folder: PathBuf,
    pub files: Vec<PathBuf>,
}
#[derive(Clone)]
pub struct PresetLibrary {
    pub root: PathBuf,
}
impl Default for PresetLibrary {
    fn default() -> Self {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            root: base.join("TripleStack").join("AudioPresets"),
        }
    }
}
fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
impl Preset {
    pub fn load(folder: &Path) -> Result<Self> {
        let manifest = folder.join(MANIFEST);
        if manifest.is_file() {
            let metadata: Manifest = serde_json::from_slice(&fs::read(&manifest)?)?;
            ensure!(metadata.version == 1, "Unsupported hitsound preset version");
            ensure!(!metadata.name.trim().is_empty(), "Preset name is empty");
            ensure!(
                metadata.files.len() <= MAX_FILES,
                "Preset contains too many audio files"
            );
            let mut files = Vec::new();
            let canonical_root = folder.canonicalize()?;
            for relative in metadata.files {
                let relative = PathBuf::from(relative.replace('\\', "/"));
                ensure!(
                    safe_relative(&relative) && audio_file(&relative),
                    "Invalid preset sample path"
                );
                let path = folder.join(relative);
                ensure!(path.is_file(), "Preset file is missing: {}", path.display());
                ensure!(
                    path.canonicalize()?.starts_with(&canonical_root),
                    "Preset sample leaves its folder"
                );
                files.push(path);
            }
            ensure!(!files.is_empty(), "Preset has no audio files");
            return Ok(Self {
                name: metadata.name,
                folder: folder.to_owned(),
                files,
            });
        }
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(folder).follow_links(false) {
            let entry = entry?;
            if entry.file_type().is_file() && audio_file(entry.path()) {
                files.push(entry.into_path());
            }
            ensure!(
                files.len() <= MAX_FILES,
                "Folder contains too many audio files"
            );
        }
        files.sort();
        ensure!(
            !files.is_empty(),
            "This folder contains no WAV, OGG, MP3 or FLAC files"
        );
        Ok(Self {
            name: folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            folder: folder.to_owned(),
            files,
        })
    }
}
impl PresetLibrary {
    pub fn compile_stable(
        &self,
        directory: &Path,
        mut progress: impl FnMut(CompileProgress),
    ) -> Result<CompileReport> {
        ensure!(directory.is_dir(), "Choose the stable Skins directory");
        ensure!(
            !directory.join("client.realm").is_file(),
            "Use the lazer skin browser for a lazer data directory"
        );
        let directory = directory.canonicalize()?;
        fs::create_dir_all(&self.root)?;
        let workspace = self.root.canonicalize()?;
        ensure!(
            !directory.starts_with(&workspace) && !workspace.starts_with(&directory),
            "Choose a Skins directory outside the preset workspace"
        );
        let mut folders = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && !entry.file_type()?.is_symlink() {
                folders.push(entry.path());
            }
        }
        folders.sort();
        let existing = self.list()?;
        let mut stamps = Vec::new();
        for preset in existing {
            if let Ok(bytes) = fs::read(preset.folder.join(STABLE_SOURCE))
                && let Ok(stamp) = serde_json::from_slice::<StableSource>(&bytes)
            {
                stamps.push(stamp);
            }
        }
        let mut report = CompileReport::default();
        for (index, folder) in folders.iter().enumerate() {
            let name = folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            progress(CompileProgress {
                completed: index,
                total: folders.len(),
                current: name.clone(),
            });
            let result = (|| -> Result<()> {
                // No audio is a normal skip; unreadable files are reported per skin.
                let has_audio = walkdir::WalkDir::new(folder)
                    .follow_links(false)
                    .into_iter()
                    .try_fold(false, |found, entry| -> Result<bool> {
                        let entry = entry?;
                        Ok(found || (entry.file_type().is_file() && audio_file(entry.path())))
                    })?;
                if !has_audio {
                    report.skipped.push(name.clone());
                    return Ok(());
                }
                let source = Preset::load(folder)?;
                let mut digest = Sha256::new();
                let mut total = 0u64;
                for file in &source.files {
                    total = total
                        .checked_add(file.metadata()?.len())
                        .context("Preset size overflow")?;
                    ensure!(total <= MAX_BYTES, "Preset audio exceeds 512 MB");
                    let relative = file
                        .strip_prefix(folder)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    digest.update((relative.len() as u64).to_le_bytes());
                    digest.update(relative.as_bytes());
                    digest.update(file.metadata()?.len().to_le_bytes());
                    let mut input = fs::File::open(file)?;
                    let mut buffer = [0u8; 65536];
                    loop {
                        let count = input.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        digest.update(&buffer[..count]);
                    }
                }
                let stamp = StableSource {
                    directory: folder.canonicalize()?,
                    digest: format!("{:x}", digest.finalize()),
                };
                if stamps.contains(&stamp) {
                    report.unchanged.push(name.clone());
                    return Ok(());
                }
                let preset = self.save_folder(folder, &name)?;
                fs::write(
                    preset.folder.join(STABLE_SOURCE),
                    serde_json::to_vec_pretty(&stamp)?,
                )?;
                stamps.push(stamp);
                report.created.push(CompiledPreset {
                    name: preset.name,
                    folder: preset.folder,
                    sounds: preset.files.len(),
                });
                Ok(())
            })();
            if let Err(error) = result {
                report.errors.push(format!("{name}: {error:#}"));
            }
        }
        progress(CompileProgress {
            completed: folders.len(),
            total: folders.len(),
            current: String::new(),
        });
        Ok(report)
    }
    pub fn list(&self) -> Result<Vec<Preset>> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut presets = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if crate::audio_examples::legacy_example_id(&path).is_some() {
                continue;
            }
            // Background imports publish by renaming their completed staging folder.
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if path.join(MANIFEST).is_file() {
                presets.push(Preset::load(&path)?);
            }
        }
        presets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(presets)
    }
    fn managed_folder(&self, folder: &Path) -> Result<PathBuf> {
        ensure!(
            crate::audio_examples::legacy_example_id(folder).is_none(),
            "Built-in examples cannot be deleted; copy one to personal presets first"
        );
        let root = self
            .root
            .canonicalize()
            .context("Preset workspace is missing")?;
        ensure!(
            !folder.symlink_metadata()?.file_type().is_symlink(),
            "Cannot remove a linked preset folder"
        );
        let target = folder.canonicalize()?;
        ensure!(
            target.parent() == Some(root.as_path()),
            "Only copied presets inside the app workspace can be removed"
        );
        ensure!(
            target.join(MANIFEST).is_file(),
            "This folder is not a saved preset"
        );
        Ok(target)
    }
    pub fn remove(&self, folder: &Path) -> Result<()> {
        let target = self.managed_folder(folder)?;
        // Check every resolved entry before recursive removal, including Windows
        // reparse points, so external skin/source folders can never be followed.
        for entry in walkdir::WalkDir::new(&target).follow_links(false) {
            let entry = entry?;
            ensure!(
                !entry.file_type().is_symlink()
                    && entry.path().canonicalize()?.starts_with(&target),
                "Preset contains a link outside its working folder"
            );
        }
        fs::remove_dir_all(&target)
            .with_context(|| format!("Removing saved preset {}", target.display()))
    }
    pub fn sharing_directory(&self) -> PathBuf {
        self.root.join("SharedZIPs")
    }
    pub fn create_share_zip(&self, folder: &Path) -> Result<PathBuf> {
        let preset = Preset::load(folder)?;
        let root = self.sharing_directory();
        fs::create_dir_all(&root)?;
        let name: String = preset
            .name
            .chars()
            .map(|c| {
                if c.is_control() || "<>:\"/\\|?*".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .take(90)
            .collect();
        let name = name.trim_matches([' ', '.']);
        let stem = format!("Preset-{}", if name.is_empty() { "Sounds" } else { name });
        let mut target = root.join(format!("{stem}.zip"));
        let mut number = 2;
        while target.exists() {
            target = root.join(format!("{stem}-{number}.zip"));
            number += 1;
        }
        self.export_zip(folder, &target)?;
        Ok(target)
    }
    pub fn shared_zips(&self) -> Result<Vec<PathBuf>> {
        let root = self.sharing_directory();
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
            {
                files.push(entry.path());
            }
        }
        files.sort();
        Ok(files)
    }
    pub fn remove_shared_zip(&self, path: &Path) -> Result<()> {
        let root = self.sharing_directory().canonicalize()?;
        ensure!(
            !path.symlink_metadata()?.file_type().is_symlink(),
            "Cannot remove a linked archive"
        );
        let target = path.canonicalize()?;
        ensure!(
            target.parent() == Some(root.as_path())
                && target.is_file()
                && target
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip")),
            "Only ZIPs in the app sharing directory can be removed"
        );
        fs::remove_file(&target)
            .with_context(|| format!("Removing shared ZIP {}", target.display()))
    }
    pub fn save_folder(&self, source: &Path, name: &str) -> Result<Preset> {
        let source = Preset::load(source)?;
        ensure!(!name.trim().is_empty(), "Enter a name for this preset");
        fs::create_dir_all(&self.root)?;
        let stage = tempfile::Builder::new()
            .prefix(".import-")
            .tempdir_in(&self.root)?;
        let mut files = Vec::new();
        let mut total = 0u64;
        for path in source.files {
            total = total
                .checked_add(path.metadata()?.len())
                .context("Preset size overflow")?;
            ensure!(total <= MAX_BYTES, "Preset audio exceeds 512 MB");
            let relative = path.strip_prefix(&source.folder)?;
            ensure!(safe_relative(relative), "Invalid sample path");
            let destination = Path::new("audio").join(relative);
            let target = stage.path().join(&destination);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(path, target)?;
            files.push(destination.to_string_lossy().replace('\\', "/"));
        }
        let metadata = Manifest {
            version: 1,
            name: name.trim().to_owned(),
            files,
        };
        fs::write(
            stage.path().join(MANIFEST),
            serde_json::to_vec_pretty(&metadata)?,
        )?;
        let target = self.unique_folder(name);
        fs::rename(stage.path(), &target).context("Saving the preset folder")?;
        Preset::load(&target)
    }
    fn unique_folder(&self, name: &str) -> PathBuf {
        let cleaned: String = name
            .chars()
            .map(|c| {
                if c.is_control() || "<>:\"/\\|?*".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .take(100)
            .collect();
        let cleaned = cleaned.trim_matches([' ', '.']);
        // Prefix also avoids Windows reserved device names such as CON.
        let base = format!(
            "Preset-{}",
            if cleaned.is_empty() { "Audio" } else { cleaned }
        );
        let mut target = self.root.join(&base);
        let mut count = 2;
        while target.exists() {
            target = self.root.join(format!("{base}-{count}"));
            count += 1;
        }
        target
    }
    pub fn export_zip(&self, folder: &Path, destination: &Path) -> Result<()> {
        ensure!(
            destination
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("zip")),
            "Save exported presets with a .zip extension"
        );
        let preset = Preset::load(folder)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(destination.parent().unwrap_or(Path::new(".")))?;
        {
            let mut archive = zip::ZipWriter::new(temporary.as_file_mut());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let mut files = Vec::new();
            let mut total = 0;
            for path in &preset.files {
                ensure!(
                    path != destination,
                    "The export file cannot replace a sample"
                );
                let relative = path.strip_prefix(&preset.folder)?;
                ensure!(safe_relative(relative), "Invalid sample path");
                total += path.metadata()?.len();
                ensure!(total <= MAX_BYTES, "Preset audio exceeds 512 MB");
                archive.start_file(relative.to_string_lossy().replace('\\', "/"), options)?;
                std::io::copy(&mut fs::File::open(path)?, &mut archive)?;
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
            archive.start_file(MANIFEST, options)?;
            archive.write_all(&serde_json::to_vec_pretty(&Manifest {
                version: 1,
                name: preset.name,
                files,
            })?)?;
            archive.finish()?;
        }
        temporary.persist(destination)?;
        Ok(())
    }
    pub fn import_zip(&self, path: &Path) -> Result<Preset> {
        fs::create_dir_all(&self.root)?;
        let stage = tempfile::Builder::new()
            .prefix(".zip-")
            .tempdir_in(&self.root)?;
        let mut archive = zip::ZipArchive::new(fs::File::open(path)?)?;
        ensure!(
            archive.len() <= MAX_FILES + 16,
            "Preset archive contains too many entries"
        );
        let mut total = 0;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let relative = entry.enclosed_name().context("Invalid archive path")?;
            ensure!(safe_relative(&relative), "Invalid archive path");
            ensure!(
                entry
                    .unix_mode()
                    .is_none_or(|mode| mode & 0o170000 != 0o120000),
                "Preset cannot contain symbolic links"
            );
            if entry.is_dir() {
                continue;
            }
            ensure!(
                relative == Path::new(MANIFEST) || audio_file(&relative),
                "Preset archive contains a non-audio file"
            );
            total += entry.size();
            ensure!(
                total <= MAX_BYTES + 1024 * 1024,
                "Preset archive is too large"
            );
            let target = stage.path().join(relative);
            fs::create_dir_all(target.parent().unwrap())?;
            ensure!(!target.exists(), "Duplicate preset archive entry");
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?;
            let copied = std::io::copy(&mut (&mut entry).take(MAX_BYTES + 1), &mut output)?;
            ensure!(copied <= MAX_BYTES, "Preset sample is too large");
        }
        let preset = Preset::load(stage.path())?;
        ensure!(
            stage.path().join(MANIFEST).is_file(),
            "Archive has no hitsound preset manifest"
        );
        let target = self.unique_folder(&preset.name);
        fs::rename(stage.path(), &target)?;
        Preset::load(&target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_bulk_import_is_repeatable_and_continues_past_empty_or_bad_skins() {
        let directory = tempfile::tempdir().unwrap();
        let skins = directory.path().join("Skins");
        for name in ["First", "Second", "Empty", "Broken"] {
            fs::create_dir_all(skins.join(name)).unwrap();
        }
        fs::write(skins.join("First/normal-hitnormal.wav"), b"first").unwrap();
        fs::write(skins.join("First/hitcircle.png"), b"excluded").unwrap();
        fs::create_dir(skins.join("Second/nested")).unwrap();
        fs::write(skins.join("Second/nested/soft-hitclap.ogg"), b"second").unwrap();
        fs::write(skins.join("Broken/normal-hitnormal.wav"), b"sound").unwrap();
        fs::write(
            skins.join("Broken/hitsound-preset.json"),
            b"invalid manifest",
        )
        .unwrap();
        let library = PresetLibrary {
            root: directory.path().join("workspace"),
        };
        let mut progress = Vec::new();
        let report = library
            .compile_stable(&skins, |p| progress.push((p.completed, p.total)))
            .unwrap();
        assert_eq!(report.created.len(), 2);
        assert_eq!(report.skipped, vec!["Empty"]);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(progress.last(), Some(&(4, 4)));
        assert!(report.created.iter().all(|preset| preset.sounds == 1));
        let repeat = library.compile_stable(&skins, |_| {}).unwrap();
        assert!(repeat.created.is_empty());
        assert_eq!(repeat.unchanged.len(), 2);
        assert_eq!(library.list().unwrap().len(), 2);
        fs::write(skins.join("First/normal-hitnormal.wav"), b"changed").unwrap();
        let changed = library.compile_stable(&skins, |_| {}).unwrap();
        assert_eq!(changed.created.len(), 1);
        assert_eq!(changed.unchanged.len(), 1);
        assert_eq!(library.list().unwrap().len(), 3);
        assert_eq!(
            fs::read(skins.join("First/normal-hitnormal.wav")).unwrap(),
            b"changed"
        );
        let zip = library
            .create_share_zip(&changed.created[0].folder)
            .unwrap();
        let imported = library.import_zip(&zip).unwrap();
        assert_eq!(fs::read(&imported.files[0]).unwrap(), b"changed");
        assert!(library.compile_stable(&library.root, |_| {}).is_err());
    }
    #[test]
    fn removing_copied_presets_or_shared_zips_keeps_external_sources_safe() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("normal-hitnormal.wav"), b"original sound").unwrap();
        let library = PresetLibrary {
            root: directory.path().join("workspace"),
        };
        let copied = library.save_folder(&source, "Share me").unwrap();
        let share = library.create_share_zip(&copied.folder).unwrap();
        let another_share = library.create_share_zip(&copied.folder).unwrap();
        assert_ne!(share, another_share);
        assert_eq!(library.shared_zips().unwrap().len(), 2);
        let imported = library.import_zip(&share).unwrap();
        library.remove_shared_zip(&share).unwrap();
        assert!(imported.files[0].is_file());
        assert!(!share.exists());
        library.remove(&copied.folder).unwrap();
        assert!(!copied.folder.exists());
        assert_eq!(
            fs::read(source.join("normal-hitnormal.wav")).unwrap(),
            b"original sound"
        );
        assert!(another_share.exists());
        assert!(library.remove(&source).is_err());
        assert!(library.remove(&library.root).is_err());
        let outside = directory.path().join("original.zip");
        fs::write(&outside, b"keep").unwrap();
        assert!(library.remove_shared_zip(&outside).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
        assert_eq!(library.list().unwrap().len(), 1);
    }
    #[test]
    fn saved_presets_are_independent_and_zip_roundtrips_are_portable() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir_all(source.join("subfolder")).unwrap();
        fs::write(source.join("normal-hitnormal.wav"), b"sound one").unwrap();
        fs::write(source.join("subfolder/soft-hitclap.ogg"), b"sound two").unwrap();
        let library = PresetLibrary {
            root: directory.path().join("library"),
        };
        let preset = library.save_folder(&source, "My / Sounds").unwrap();
        fs::remove_file(source.join("normal-hitnormal.wav")).unwrap();
        assert_eq!(fs::read(&preset.files[0]).unwrap(), b"sound one");
        assert_eq!(library.list().unwrap().len(), 1);
        let archive = directory.path().join("share.zip");
        library.export_zip(&preset.folder, &archive).unwrap();
        let receiving = PresetLibrary {
            root: directory.path().join("someone-else"),
        };
        let imported = receiving.import_zip(&archive).unwrap();
        assert_eq!(imported.name, "My / Sounds");
        assert_eq!(imported.files.len(), 2);
        assert_eq!(fs::read(&imported.files[1]).unwrap(), b"sound two");
        assert!(
            imported
                .files
                .iter()
                .all(|p| p.starts_with(&receiving.root))
        );
        let second = receiving.import_zip(&archive).unwrap();
        assert_ne!(second.folder, imported.folder);
    }
    #[test]
    fn unsafe_archives_and_external_manifest_paths_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("bad.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("../outside.wav", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        let library = PresetLibrary {
            root: directory.path().join("library"),
        };
        assert!(library.import_zip(&archive).is_err());
        assert!(!directory.path().join("outside.wav").exists());
        fs::write(
            directory.path().join(MANIFEST),
            r#"{"version":1,"name":"Bad","files":["../outside.wav"]}"#,
        )
        .unwrap();
        assert!(Preset::load(directory.path()).is_err());
    }
}
