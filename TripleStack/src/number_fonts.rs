//! Portable number-sprite families. Managed fonts are materialized into exported skins.
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::LazyLock,
};

static BUNDLED: &[u8] = include_bytes!("../assets/number-fonts.zip");
#[derive(Clone, Serialize, Deserialize)]
pub struct NumberFont {
    pub id: String,
    pub name: String,
    pub prefix: String,
}
impl NumberFont {
    pub fn skin_prefix(&self) -> String {
        format!("fonts/triplestack/{}/default", self.id)
    }
}
static FONTS: LazyLock<Vec<NumberFont>> = LazyLock::new(|| {
    let mut archive = zip::ZipArchive::new(Cursor::new(BUNDLED)).expect("bundled font archive");
    let mut json = String::new();
    archive
        .by_name("fonts.json")
        .expect("font catalog")
        .read_to_string(&mut json)
        .expect("font catalog text");
    serde_json::from_str(&json).expect("font catalog entries")
});
pub fn bundled() -> &'static [NumberFont] {
    &FONTS
}
pub fn personal_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TripleStack/NumberFonts")
}
pub fn imported() -> Result<Vec<NumberFont>> {
    let root = personal_root();
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut fonts = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path().join("font.json");
        if let Ok(bytes) = fs::read(path)
            && let Ok(font) = serde_json::from_slice::<NumberFont>(&bytes)
        {
            fonts.push(font);
        }
    }
    fonts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(fonts)
}
pub fn discover(skin: &Path) -> Result<Vec<String>> {
    let mut prefixes = BTreeSet::new();
    for entry in walkdir::WalkDir::new(skin).max_depth(7).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let stem = stem.strip_suffix("@2x").unwrap_or(stem);
        let Some((prefix, digit)) = stem.rsplit_once('-') else {
            continue;
        };
        if entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            && digit.len() == 1
            && digit.bytes().all(|b| b.is_ascii_digit())
        {
            let path = entry
                .path()
                .parent()
                .unwrap()
                .join(prefix)
                .strip_prefix(skin)?
                .to_string_lossy()
                .replace('\\', "/");
            prefixes.insert(path);
        }
    }
    Ok(prefixes.into_iter().collect())
}
pub fn import_folder(source: &Path) -> Result<Vec<NumberFont>> {
    import_into(source, &personal_root())
}
fn import_into(source: &Path, root: &Path) -> Result<Vec<NumberFont>> {
    let prefixes = discover(source)?;
    ensure!(
        !prefixes.is_empty(),
        "Choose a folder containing numbered PNGs such as default-0.png through default-9.png"
    );
    ensure!(
        prefixes.len() <= 128,
        "Choose a smaller font folder (up to 128 font sets)"
    );
    fs::create_dir_all(root)?;
    let mut fonts = Vec::new();
    for prefix in prefixes {
        let mut digits = Vec::new();
        let mut hash = Sha256::new();
        for digit in 0..10 {
            let image = crate::numbers::read_digit(source, Path::new(&prefix), digit)?
                .unwrap_or_else(|| crate::numbers::fallback_digit(digit as u8));
            let bytes = crate::preview_png(&image)?;
            hash.update(&bytes);
            digits.push(bytes);
        }
        let id = format!("user-{:x}", hash.finalize());
        let name = if Path::new(&prefix)
            .parent()
            .is_some_and(|p| !p.as_os_str().is_empty())
        {
            prefix.clone()
        } else {
            format!(
                "{} / {prefix}",
                source.file_name().unwrap_or_default().to_string_lossy()
            )
        };
        let font = NumberFont {
            id: id.clone(),
            name,
            prefix: "default".into(),
        };
        let folder = root.join(&id);
        if !folder.join("font.json").is_file() {
            let stage = tempfile::Builder::new().prefix(".font-").tempdir_in(root)?;
            for (digit, bytes) in digits.iter().enumerate() {
                fs::write(stage.path().join(format!("default-{digit}.png")), bytes)?;
            }
            fs::write(
                stage.path().join("font.json"),
                serde_json::to_vec_pretty(&font)?,
            )?;
            fs::rename(stage.path(), &folder)?;
        }
        fonts.push(font);
    }
    Ok(fonts)
}
pub(crate) fn managed_id(prefix: &Path) -> Option<String> {
    let normalized = prefix.to_string_lossy().replace('\\', "/");
    let id = normalized
        .strip_prefix("fonts/triplestack/")?
        .strip_suffix("/default")?;
    (!id.is_empty() && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
        .then(|| id.to_owned())
}
pub(crate) fn resolve(prefix: &Path) -> Result<Option<Vec<RgbaImage>>> {
    let Some(id) = managed_id(prefix) else {
        return Ok(None);
    };
    if id == "fallback" {
        return Ok(Some((0..10).map(crate::numbers::fallback_digit).collect()));
    }
    let mut digits = Vec::new();
    if let Some(font) = bundled().iter().find(|font| font.id == id) {
        let mut archive = zip::ZipArchive::new(Cursor::new(BUNDLED))?;
        for digit in 0..10 {
            let mut image = None;
            for hd in [true, false] {
                let name = format!("{}-{digit}{}.png", font.prefix, if hd { "@2x" } else { "" });
                if let Ok(mut entry) = archive.by_name(&name) {
                    let mut bytes = Vec::new();
                    entry.read_to_end(&mut bytes)?;
                    if let Ok(raw) = image::load_from_memory(&bytes) {
                        let raw = raw.to_rgba8();
                        image = Some(if hd {
                            crate::images::resize_circle(
                                &raw,
                                ((raw.width() / 2).max(1), (raw.height() / 2).max(1)),
                            )
                            .into_owned()
                        } else {
                            raw
                        });
                        break;
                    }
                }
            }
            digits.push(image.unwrap_or_else(|| crate::numbers::fallback_digit(digit)));
        }
    } else {
        let folder = personal_root().join(&id);
        for digit in 0..10 {
            digits.push(
                crate::numbers::read_digit(&folder, Path::new("default"), digit)?
                    .unwrap_or_else(|| crate::numbers::fallback_digit(digit as u8)),
            );
        }
    }
    Ok(Some(digits))
}
pub fn validate_prefix(source: &str) -> Result<String> {
    Ok(crate::numbers::prefix(source)?
        .to_string_lossy()
        .replace('\\', "/"))
}
/// Fill only absent/unreadable glyphs; valid transparent number sprites remain intentional.
pub fn ensure_complete(skin: &Path, draft: Option<&str>) -> Result<()> {
    let numbers = crate::numbers::load(skin, draft)?;
    let root = skin.canonicalize()?;
    for digit in 0..10 {
        let path = numbers.path(skin, digit, false);
        let sd_valid = path
            .canonicalize()
            .ok()
            .is_some_and(|p| p.starts_with(&root))
            && crate::read_image(&path).is_ok();
        if !sd_valid {
            write_digit(&root, &path, &numbers.digits[digit])?;
        }
        let hd = numbers.path(skin, digit, true);
        if hd.is_file() && crate::read_image(&hd).is_err() {
            let image = &numbers.digits[digit];
            let doubled =
                crate::images::resize_circle(image, (image.width() * 2, image.height() * 2));
            write_digit(&root, &hd, &doubled)?;
        }
    }

    Ok(())
}

fn write_digit(root: &Path, path: &Path, image: &RgbaImage) -> Result<()> {
    let mut ancestor = path.parent().context("Invalid number font path")?;
    while !ancestor.exists() {
        ancestor = ancestor.parent().context("Invalid font folder")?;
    }
    ensure!(
        ancestor.canonicalize()?.starts_with(root),
        "Number font folder leaves the skin"
    );
    fs::create_dir_all(path.parent().unwrap())?;
    let bytes = crate::preview_png(image)?;
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    use std::io::Write;
    temporary.write_all(&bytes)?;
    temporary.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_five_bundled_fonts_resolve_and_materialize_as_portable_pngs() {
        assert_eq!(bundled().len(), 5);
        for font in bundled() {
            let directory = tempfile::tempdir().unwrap();
            let mut ini =
                crate::skin_ini::SkinIni::from_source("[General]\nCursorExpand: 0\n".into());
            let before = ini.source().to_owned();
            ini.set_hit_circle_prefix(&font.skin_prefix()).unwrap();
            assert!(ini.source().contains("CursorExpand: 0"));
            let prefix = PathBuf::from(font.skin_prefix());
            let expected = resolve(&prefix).unwrap().unwrap();
            assert_eq!(expected.len(), 10);
            crate::save_skin_ini_with_numbers(directory.path(), &mut ini).unwrap();
            for digit in 0..10 {
                let path = directory
                    .path()
                    .join(format!("{}-{digit}.png", prefix.display()));
                assert_eq!(crate::read_image(&path).unwrap(), expected[digit]);
            }
            let local = crate::numbers::load(directory.path(), None).unwrap();
            assert_eq!(local.digits, expected);
            assert!(ini.undo());
            assert_eq!(ini.source(), before);
        }
    }
    #[test]
    fn incomplete_corrupt_and_empty_sets_produce_all_ten_digits() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path();
        fs::write(skin.join("default-0.png"), b"corrupt").unwrap();
        fs::write(skin.join("default-1@2x.png"), b"").unwrap();
        image::RgbaImage::new(1, 1)
            .save(skin.join("default-2.png"))
            .unwrap();
        ensure_complete(skin, None).unwrap();
        assert!(crate::read_image(&skin.join("default-1@2x.png")).is_ok());
        for digit in 0..10 {
            let image = crate::read_image(&skin.join(format!("default-{digit}.png"))).unwrap();
            if digit == 2 {
                assert_eq!(image.dimensions(), (1, 1));
                assert!(image.pixels().all(|p| p[3] == 0));
            } else {
                assert!(image.pixels().any(|p| p[3] > 0));
            }
        }
        let library = tempfile::tempdir().unwrap();
        let fonts = import_into(skin, library.path()).unwrap();
        assert_eq!(fonts.len(), 1);
        let copied = library.path().join(&fonts[0].id);
        for digit in 0..10 {
            assert!(crate::read_image(&copied.join(format!("default-{digit}.png"))).is_ok());
        }
        assert_eq!(
            import_into(skin, library.path()).unwrap()[0].id,
            fonts[0].id
        );
    }
    #[test]
    fn font_setting_preserves_comments_other_font_keys_and_undo() {
        let original = "[Fonts]\r\nHitCirclePrefix: old // keep this\r\nScorePrefix: fonts/hud/score\r\nComboPrefix: score\r\n";
        let mut ini = crate::skin_ini::SkinIni::from_source(original.into());
        ini.set_hit_circle_prefix("fonts/triplestack/nm-wc1/default")
            .unwrap();
        assert!(ini.source().contains("// keep this"));
        assert!(ini.source().contains("ScorePrefix: fonts/hud/score"));
        assert!(ini.source().contains("ComboPrefix: score"));
        assert!(!ini.source().replace("\r\n", "").contains('\n'));
        assert!(ini.set_hit_circle_prefix("../../other/default").is_err());
        assert!(ini.undo());
        assert_eq!(ini.source(), original);
    }
}
