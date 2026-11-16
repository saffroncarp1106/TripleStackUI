//! Built-in sound banks are shipped as a compressed, content-deduplicated archive.
use crate::audio_presets::Preset;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

static ARCHIVE: &[u8] = include_bytes!("../assets/audio-examples.zip");
const INDEX: &str = include_str!("../assets/audio-examples.json");
pub const LEGACY_MARKER: &str = ".builtin-example.json";
#[derive(Deserialize)]
struct Catalog {
    version: u32,
    presets: Vec<Example>,
}
#[derive(Deserialize)]
struct Example {
    id: String,
    name: String,
    files: Vec<ExampleFile>,
}
#[derive(Deserialize)]
struct ExampleFile {
    path: String,
    hash: String,
}
#[derive(Serialize, Deserialize)]
struct LegacyMarker {
    id: String,
}
fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(INDEX).expect("valid bundled example catalog"))
}
pub fn legacy_example_id(folder: &Path) -> Option<String> {
    let marker: LegacyMarker =
        serde_json::from_slice(&fs::read(folder.join(LEGACY_MARKER)).ok()?).ok()?;
    catalog()
        .presets
        .iter()
        .any(|example| {
            example.id == marker.id
                && folder
                    .file_name()
                    .is_some_and(|name| name == marker.id.as_str())
        })
        .then_some(marker.id)
}
fn relative(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path.replace('\\', "/"));
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|c| matches!(c, Component::Normal(_))),
        "Invalid example path"
    );
    Ok(path)
}
#[derive(Clone)]
pub struct ExampleLibrary {
    pub root: PathBuf,
}
impl ExampleLibrary {
    pub fn beside(personal: &Path) -> Self {
        Self {
            root: personal.parent().unwrap_or(personal).join("ExamplePresets"),
        }
    }
    fn version_root(&self) -> PathBuf {
        static VERSION: OnceLock<String> = OnceLock::new();
        self.root
            .join(VERSION.get_or_init(|| format!("bundle-{:x}", Sha256::digest(ARCHIVE))))
    }
    pub fn load(&self) -> Result<Vec<Preset>> {
        let catalog = catalog();
        ensure!(catalog.version == 1, "Unsupported example catalog");
        let root = self.version_root();
        let load = || {
            catalog
                .presets
                .iter()
                .map(|example| Preset::load(&root.join(&example.id)))
                .collect::<Result<Vec<_>>>()
        };
        if root.join(".complete").is_file()
            && let Ok(presets) = load()
        {
            return Ok(presets);
        }
        fs::create_dir_all(&root)?;
        let resolved_root = root.canonicalize()?;
        let mut archive = zip::ZipArchive::new(Cursor::new(ARCHIVE))?;
        for example in &catalog.presets {
            let id = relative(&example.id)?;
            ensure!(id.components().count() == 1, "Invalid example ID");
            let folder = root.join(id);
            let mut files = Vec::new();
            for file in &example.files {
                ensure!(
                    file.hash.len() == 64 && file.hash.bytes().all(|c| c.is_ascii_hexdigit()),
                    "Invalid example hash"
                );
                let relative = relative(&file.path)?;
                ensure!(
                    triplestack::beatmap_audio::audio_file(&relative),
                    "Example contains a non-audio sample"
                );
                let target = folder.join(&relative);
                fs::create_dir_all(target.parent().unwrap())?;
                ensure!(
                    target
                        .parent()
                        .unwrap()
                        .canonicalize()?
                        .starts_with(&resolved_root),
                    "Example path leaves its cache"
                );
                let mut entry = archive.by_name(&format!("blobs/{}", file.hash))?;
                let mut temporary = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
                let mut digest = Sha256::new();
                let mut buffer = [0u8; 65536];
                loop {
                    let size = entry.read(&mut buffer)?;
                    if size == 0 {
                        break;
                    }
                    digest.update(&buffer[..size]);
                    temporary.write_all(&buffer[..size])?;
                }
                ensure!(
                    format!("{:x}", digest.finalize()) == file.hash,
                    "Bundled sample checksum mismatch"
                );
                temporary.persist(&target)?;
                files.push(file.path.clone());
            }
            let manifest = serde_json::json!({"version":1,"name":example.name,"files":files});
            let mut temporary = tempfile::NamedTempFile::new_in(&folder)?;
            temporary.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
            temporary.persist(folder.join("hitsound-preset.json"))?;
        }
        fs::write(root.join(".complete"), b"1")?;
        load().context("Loading built-in examples")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_examples_are_portable_protected_and_can_be_copied_to_personal() {
        let directory = tempfile::tempdir().unwrap();
        let personal = crate::audio_presets::PresetLibrary {
            root: directory.path().join("AudioPresets"),
        };
        let library = ExampleLibrary::beside(&personal.root);
        let examples = library.load().unwrap();
        assert_eq!(examples.len(), 22);
        assert!(examples.iter().all(|p| !p.files.is_empty()));
        assert!(personal.list().unwrap().is_empty());
        fs::create_dir_all(&personal.root).unwrap();
        assert!(personal.remove(&examples[0].folder).is_err());
        let copy = personal
            .save_folder(&examples[0].folder, "My own example")
            .unwrap();
        assert_eq!(copy.files.len(), examples[0].files.len());
        personal.remove(&copy.folder).unwrap();
        assert!(examples[0].files[0].is_file());
        let legacy = personal.root.join(&catalog().presets[0].id);
        fs::create_dir(&legacy).unwrap();
        fs::write(
            legacy.join("hitsound-preset.json"),
            fs::read(examples[0].folder.join("hitsound-preset.json")).unwrap(),
        )
        .unwrap();
        fs::write(
            legacy.join(LEGACY_MARKER),
            serde_json::to_vec(&LegacyMarker {
                id: catalog().presets[0].id.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(personal.list().unwrap().is_empty());
        assert!(personal.remove(&legacy).is_err());
        let reloaded = library.load().unwrap();
        assert_eq!(reloaded[0].folder, examples[0].folder);
    }
}
