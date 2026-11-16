//! Read-only lazer skin access adapted from the user's LazerUnSlopper pipeline.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const SKIN_READER: &str = include_str!("../scripts/read_lazer_skins.ps1");
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkinFile {
    pub name: String,
    pub hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LazerSkin {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub instantiation: Option<String>,
    pub files: Vec<SkinFile>,
}
#[derive(Deserialize)]
struct Catalog {
    version: u32,
    skins: Vec<LazerSkin>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LazerSettings {
    pub source: PathBuf,
    pub realm_dll: Option<PathBuf>,
    pub powershell: Option<PathBuf>,
}
#[derive(Serialize, Deserialize)]
struct WorkingCopy {
    version: u32,
    source: PathBuf,
    skin: LazerSkin,
}

fn app_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TripleStack")
}
pub fn cache_root() -> PathBuf {
    app_root().join("LazerSkins")
}
impl LazerSettings {
    pub fn discover() -> Self {
        if let Ok(bytes) = fs::read(app_root().join("lazer.json"))
            && let Ok(settings) = serde_json::from_slice(&bytes)
        {
            return settings;
        }
        let mut candidates = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            for app in ["LazerUnSlopper", "Mapfold"] {
                if let Ok(bytes) = fs::read(PathBuf::from(&local).join(app).join("settings.json"))
                    && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                    && let Some(source) = value["source"].as_str()
                {
                    candidates.push(PathBuf::from(source));
                }
            }
        }
        for variable in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(variable) {
                for name in ["osu", "osu-lazer", "osulazer", "osu!lazer"] {
                    candidates.push(PathBuf::from(&root).join(name));
                }
            }
        }
        Self {
            source: candidates
                .into_iter()
                .find(|path| is_lazer_directory(path))
                .unwrap_or_default(),
            ..Default::default()
        }
    }
    pub fn save(&self) -> Result<()> {
        let root = app_root();
        fs::create_dir_all(&root)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&root)?;
        temporary.write_all(&serde_json::to_vec_pretty(self)?)?;
        temporary.persist(root.join("lazer.json"))?;
        Ok(())
    }
    fn realm_assembly(&self) -> Result<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        self.realm_dll.clone().filter(|p| p.is_file())
            .or_else(|| std::env::var_os("LAZER_REALM_DLL").map(PathBuf::from).filter(|p| p.is_file()))
            .or_else(|| local.and_then(|local| ["osulazer", "osu!lazer"].into_iter()
                .map(|name| local.join(name).join("current/Realm.dll")).find(|p| p.is_file())))
            .context("Cannot locate osu!lazer's Realm.dll. Choose it under Reader dependencies (normally in the lazer application/current folder).")
    }
    fn powershell_executable(&self) -> PathBuf {
        self.powershell.clone().filter(|p| p.is_file())
            .or_else(|| std::env::var_os("LAZER_PWSH").map(PathBuf::from).filter(|p| p.is_file()))
            .or_else(|| std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("PowerShell/7/pwsh.exe")).filter(|p| p.is_file()))
            .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".cache/codex-runtimes/codex-primary-runtime/dependencies/native/powershell/pwsh.exe")).filter(|p| p.is_file()))
            .unwrap_or_else(|| PathBuf::from("pwsh"))
    }
}
pub fn is_lazer_directory(path: &Path) -> bool {
    path.join("client.realm").is_file() && path.join("files").is_dir()
}

pub fn read_skins(settings: &LazerSettings) -> Result<Vec<LazerSkin>> {
    ensure!(
        is_lazer_directory(&settings.source),
        "Choose the lazer data folder containing client.realm and files, rather than the application folder"
    );
    let realm = settings.realm_assembly()?;
    let temp = tempfile::Builder::new()
        .prefix("triplestack-lazer-reader-")
        .tempdir()?;
    let script = temp.path().join("read_skins.ps1");
    let output = temp.path().join("skins.json");
    let stderr_path = temp.path().join("reader-error.txt");
    fs::write(&script, SKIN_READER)?;
    let mut command = Command::new(settings.powershell_executable());
    command
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&script)
        .arg("-Database")
        .arg(settings.source.join("client.realm"))
        .arg("-RealmAssembly")
        .arg(realm)
        .arg("-OutputFile")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(&stderr_path)?));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().context("Starting the lazer Realm reader. PowerShell 7 is required; choose pwsh.exe under Reader dependencies if it is not installed on PATH.")?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "The lazer Realm reader timed out. Check that the selected Realm.dll matches your lazer installation."
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    ensure!(
        status.success(),
        "Lazer skin reader failed: {}",
        fs::read_to_string(stderr_path).unwrap_or_default().trim()
    );
    let bytes = fs::read(output).context("Reading the lazer skin catalog")?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let catalog: Catalog = serde_json::from_slice(bytes).context("Parsing lazer skin metadata")?;
    ensure!(
        catalog.version == 1,
        "Unsupported lazer skin catalog version"
    );
    let mut skins = catalog.skins;
    skins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(skins)
}
fn safe_relative(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(name.replace('\\', "/"));
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|c| matches!(c, Component::Normal(_))),
        "Unsafe lazer asset path: {name}"
    );
    for part in path.components() {
        let name = part.as_os_str().to_string_lossy();
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        let device = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        ensure!(
            !device
                && !name.ends_with([' ', '.'])
                && !name
                    .chars()
                    .any(|c| c.is_control() || "<>:\"|?*".contains(c)),
            "Unsupported Windows asset filename: {name}"
        );
    }
    Ok(path)
}
fn safe_label(name: &str) -> String {
    let value: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    let value = value.trim_matches([' ', '.']);
    if value.is_empty() {
        "Lazer skin".into()
    } else {
        value.to_owned()
    }
}
fn future_canonical(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().context("Invalid working-copy directory")?;
    Ok(future_canonical(parent)?.join(path.file_name().context("Invalid folder name")?))
}
fn validate_cache(source: &Path, cache: &Path) -> Result<()> {
    let source = source.canonicalize()?;
    let cache = future_canonical(&std::path::absolute(cache)?)?;
    ensure!(
        !cache.starts_with(&source) && !source.starts_with(&cache),
        "Lazer working copies must be outside the lazer data folder"
    );
    Ok(())
}
fn blob_path(source: &Path, hash: &str) -> Result<PathBuf> {
    ensure!(
        hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
        "Invalid lazer file hash"
    );
    Ok(source
        .join("files")
        .join(&hash[..1])
        .join(&hash[..2])
        .join(hash))
}
fn digest_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn extract_skin(
    source: &Path,
    skin: &LazerSkin,
    cache: &Path,
    reuse_edits: bool,
) -> Result<PathBuf> {
    validate_cache(source, cache)?;
    ensure!(
        !skin.key.is_empty() && skin.key.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'),
        "Invalid lazer skin ID"
    );
    let base = format!("{} [{}]", safe_label(&skin.name), skin.key);
    let mut destination = cache.join(&base);
    if reuse_edits && destination.is_dir() {
        let marker = destination.join(".triplestack-lazer-source.json");
        if let Ok(bytes) = fs::read(marker)
            && let Ok(existing) = serde_json::from_slice::<WorkingCopy>(&bytes)
            && existing.skin.key == skin.key
        {
            ensure!(
                destination
                    .canonicalize()?
                    .starts_with(cache.canonicalize()?),
                "Working copy leaves the cache directory"
            );
            return Ok(destination);
        }
    }
    let mut number = 2;
    while destination.exists() {
        destination = cache.join(format!("{base} ({number})"));
        number += 1;
    }
    let source_root = source.canonicalize()?;
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for file in &skin.files {
        let relative = safe_relative(&file.name)?;
        ensure!(
            seen.insert(relative.to_string_lossy().to_lowercase()),
            "Duplicate lazer asset name: {}",
            file.name
        );
        ensure!(
            !relative
                .to_string_lossy()
                .eq_ignore_ascii_case(".triplestack-lazer-source.json"),
            "Reserved skin asset name"
        );
        let blob = blob_path(source, &file.hash)?;
        ensure!(
            blob.is_file(),
            "Missing lazer asset for {}: {}",
            file.name,
            blob.display()
        );
        ensure!(
            !blob.symlink_metadata()?.file_type().is_symlink()
                && blob.canonicalize()?.starts_with(&source_root),
            "Asset leaves the lazer data store"
        );
        plans.push((blob, relative, &file.hash));
    }
    fs::create_dir_all(cache)?;
    let stage = tempfile::Builder::new()
        .prefix(".extract-")
        .tempdir_in(cache)?;
    for (blob, relative, expected) in plans {
        let target = stage.path().join(relative);
        fs::create_dir_all(target.parent().unwrap())?;
        fs::copy(&blob, &target)
            .with_context(|| format!("Copying lazer asset {}", blob.display()))?;
        #[cfg(windows)]
        {
            let mut permissions = target.metadata()?.permissions();
            if permissions.readonly() {
                permissions.set_readonly(false);
                fs::set_permissions(&target, permissions)?;
            }
        }
        ensure!(
            digest_file(&target)?.eq_ignore_ascii_case(expected),
            "Lazer asset integrity check failed: {}",
            blob.display()
        );
    }
    if !stage.path().join("skin.ini").is_file() {
        let single_line = |text: &str| text.replace(['\r', '\n'], " ");
        fs::write(
            stage.path().join("skin.ini"),
            format!(
                "[General]\nName: {}\nAuthor: {}\nVersion: latest\n",
                single_line(&skin.name),
                single_line(&skin.creator)
            ),
        )?;
    }
    fs::write(
        stage.path().join(".triplestack-lazer-source.json"),
        serde_json::to_vec_pretty(&WorkingCopy {
            version: 1,
            source: source.to_owned(),
            skin: skin.clone(),
        })?,
    )?;
    fs::rename(stage.path(), &destination).context("Saving the lazer skin working copy")?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(source: &Path, name: &str, data: &[u8]) -> SkinFile {
        let hash = format!("{:x}", Sha256::digest(data));
        let blob = blob_path(source, &hash).unwrap();
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::write(blob, data).unwrap();
        SkinFile {
            name: name.into(),
            hash,
        }
    }
    #[test]
    fn extraction_preserves_assets_and_existing_edits_without_writing_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("lazer");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("client.realm"), b"database").unwrap();
        let skin = LazerSkin {
            key: "abc-123".into(),
            name: "My / Lazer skin".into(),
            creator: "Author".into(),
            instantiation: None,
            files: vec![
                fixture(&source, "hitcircle.png", b"image"),
                fixture(&source, "normal-hitnormal.wav", b"audio"),
                fixture(&source, "nested/cursor.png", b"cursor"),
            ],
        };
        let cache = dir.path().join("cache");
        let extracted = extract_skin(&source, &skin, &cache, true).unwrap();
        assert_eq!(
            fs::read(extracted.join("normal-hitnormal.wav")).unwrap(),
            b"audio"
        );
        assert!(extracted.join("skin.ini").is_file());
        fs::write(extracted.join("skin.ini"), b"edited").unwrap();
        assert_eq!(
            extract_skin(&source, &skin, &cache, true).unwrap(),
            extracted
        );
        assert_eq!(fs::read(extracted.join("skin.ini")).unwrap(), b"edited");
        let fresh = extract_skin(&source, &skin, &cache, false).unwrap();
        assert_ne!(fresh, extracted);
        assert_ne!(fs::read(fresh.join("skin.ini")).unwrap(), b"edited");
        assert_eq!(fs::read(source.join("client.realm")).unwrap(), b"database");
        assert!(!source.join("skin.ini").exists());
    }
    #[test]
    fn rejects_unsafe_names_corrupt_assets_and_caches_inside_lazer() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("lazer");
        fs::create_dir(&source).unwrap();
        let file = fixture(&source, "hitcircle.png", b"data");
        let mut skin = LazerSkin {
            key: "abc".into(),
            name: "Skin".into(),
            creator: String::new(),
            instantiation: None,
            files: vec![file.clone()],
        };
        assert!(extract_skin(&source, &skin, &source.join("cache"), true).is_err());
        assert!(!source.join("cache").exists());
        skin.files[0].name = "../outside.wav".into();
        assert!(extract_skin(&source, &skin, &dir.path().join("cache"), true).is_err());
        skin.files[0] = file;
        fs::write(blob_path(&source, &skin.files[0].hash).unwrap(), b"corrupt").unwrap();
        assert!(extract_skin(&source, &skin, &dir.path().join("cache"), true).is_err());
        assert!(
            fs::read_dir(dir.path().join("cache"))
                .unwrap()
                .next()
                .is_none()
        );
        for name in [
            "C:/outside",
            "CON.txt",
            "hitcircle.png:stream",
            "folder/../hit.wav",
            "file. ",
        ] {
            assert!(safe_relative(name).is_err());
        }
    }
    #[test]
    #[ignore = "requires installed osu!lazer, Realm.dll and PowerShell 7"]
    fn installed_lazer_catalog_and_selected_skin_copy_work() {
        let settings = LazerSettings::discover();
        let before = digest_file(&settings.source.join("client.realm")).unwrap();
        let skins = read_skins(&settings).unwrap();
        assert!(!skins.is_empty());
        let selected = skins
            .iter()
            .find(|skin| {
                skin.files
                    .iter()
                    .any(|file| file.name.eq_ignore_ascii_case("hitcircle.png"))
            })
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let extracted = extract_skin(&settings.source, selected, dir.path(), true).unwrap();
        assert!(extracted.join("hitcircle.png").is_file());
        assert!(extracted.join("skin.ini").is_file());
        let preview = triplestack::preview(&extracted, triplestack::Options::default()).unwrap();
        assert!(preview.original_circle.width() > 0 && preview.circle.width() > 0);
        assert!(preview.assets.iter().any(|(label, _)| *label == "Cursor"));
        let mut ini = triplestack::skin_ini::SkinIni::load(&extracted.join("skin.ini")).unwrap();
        ini.set_cursor_expand(false);
        ini.save(&extracted.join("skin.ini")).unwrap();
        assert!(
            !triplestack::skin_ini::SkinIni::load(&extracted.join("skin.ini"))
                .unwrap()
                .cursor_expand()
        );
        let sounds = fs::read_dir(&extracted)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| triplestack::beatmap_audio::audio_file(path))
            .count();
        assert!(sounds > 0);
        // The audio import path owns its files independently of the lazer
        // working copy and the share archive used to deliver it.
        let library = crate::audio_presets::PresetLibrary {
            root: dir.path().join("audio-workspace"),
        };
        let preset = library.save_folder(&extracted, &selected.name).unwrap();
        assert!(
            preset
                .files
                .iter()
                .all(|file| triplestack::beatmap_audio::audio_file(file))
        );
        assert!(preset.files.len() >= sounds);
        let zip = library.create_share_zip(&preset.folder).unwrap();
        let imported = library.import_zip(&zip).unwrap();
        library.remove_shared_zip(&zip).unwrap();
        let mut playable = 0;
        for (source, received) in preset.files.iter().zip(&imported.files) {
            assert_eq!(fs::read(source).unwrap(), fs::read(received).unwrap());
            if crate::audio_playback::load_sample(received).0.is_some() {
                playable += 1;
            }
        }
        assert!(playable > 0);
        assert_eq!(
            digest_file(&settings.source.join("client.realm")).unwrap(),
            before
        );
        println!(
            "Read {} lazer skins; extracted {} ({} files)",
            skins.len(),
            selected.name,
            selected.files.len()
        );
    }
}
