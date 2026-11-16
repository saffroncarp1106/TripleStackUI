//! Resolve only the active hit-circle font, including nested prefixes and HD-only skins.
use crate::{images, read_image, skin_ini::SkinIni};
use ab_glyph::{Font, FontRef};
use anyhow::{Result, ensure};
use image::{Rgba, RgbaImage};
use std::{
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

pub(crate) struct Numbers {
    pub prefix: PathBuf,
    pub digits: Vec<RgbaImage>,
}
impl Numbers {
    pub fn path(&self, skin: &Path, digit: usize, hd: bool) -> PathBuf {
        number_path(skin, &self.prefix, digit, hd)
    }
}
fn number_path(skin: &Path, prefix: &Path, digit: usize, hd: bool) -> PathBuf {
    skin.join(format!(
        "{}-{digit}{}.png",
        prefix.display(),
        if hd { "@2x" } else { "" }
    ))
}
pub(crate) fn prefix(source: &str) -> Result<PathBuf> {
    let mut in_fonts = false;
    let mut value = "default";
    for line in source.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_fonts = line.eq_ignore_ascii_case("[Fonts]");
        } else if in_fonts
            && let Some((key, entry)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("HitCirclePrefix")
            && !entry.trim().is_empty()
        {
            value = entry.trim().trim_matches('"');
        }
    }
    let path = PathBuf::from(value.replace('\\', "/"));
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            && !value.contains(':'),
        "HitCirclePrefix must be a path inside the skin folder"
    );
    Ok(path)
}
pub(crate) fn load(skin: &Path, draft: Option<&str>) -> Result<Numbers> {
    let ini;
    let source = if let Some(source) = draft {
        source
    } else if skin.join("skin.ini").is_file() {
        ini = SkinIni::load(&skin.join("skin.ini"))?;
        ini.source()
    } else {
        ""
    };
    let prefix = prefix(source)?;
    let mut digits = Vec::with_capacity(10);
    let mut managed = None;
    let mut managed_loaded = false;
    for digit in 0..10 {
        let mut image = read_digit(skin, &prefix, digit)?;
        if image.is_none() && crate::number_fonts::managed_id(&prefix).is_some() {
            if !managed_loaded {
                managed = crate::number_fonts::resolve(&prefix).ok().flatten();
                managed_loaded = true;
            }
            image = managed
                .as_ref()
                .and_then(|digits| digits.get(digit))
                .cloned();
        } else if image.is_none() {
            image = read_digit(skin, Path::new("default"), digit)?;
        }
        digits.push(image.unwrap_or_else(|| fallback_digit(digit as u8)));
    }
    Ok(Numbers { prefix, digits })
}
pub(crate) fn read_digit(skin: &Path, prefix: &Path, digit: usize) -> Result<Option<RgbaImage>> {
    for hd in [true, false] {
        let path = number_path(skin, prefix, digit, hd);
        if !path.is_file() {
            continue;
        }
        ensure!(
            path.canonicalize()?.starts_with(skin.canonicalize()?),
            "Number sprite leaves the skin folder"
        );
        // A corrupt or empty file is missing for rendering purposes; keep trying safe fallbacks.
        let Ok(raw) = read_image(&path) else {
            continue;
        };
        return Ok(Some(if hd {
            images::resize_circle(&raw, ((raw.width() / 2).max(1), (raw.height() / 2).max(1)))
                .into_owned()
        } else {
            raw
        }));
    }
    Ok(None)
}
pub(crate) fn fallback_digit(digit: u8) -> RgbaImage {
    static FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
        FontRef::try_from_slice(include_bytes!(
            "../../Source_Code_Pro/static/SourceCodePro-Bold.ttf"
        ))
        .expect("bundled digit font")
    });
    let glyph = FONT.glyph_id(char::from(b'0' + digit)).with_scale(72.0);
    let outline = FONT
        .outline_glyph(glyph)
        .expect("bundled font contains digits");
    let bounds = outline.px_bounds();
    let mut image = RgbaImage::new(
        bounds.width().ceil() as u32 + 4,
        bounds.height().ceil() as u32 + 4,
    );
    outline.draw(|x, y, coverage| {
        image.put_pixel(
            x + 2,
            y + 2,
            Rgba([255, 255, 255, (coverage * 255.0).round() as u8]),
        )
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_custom_hd_fonts_and_preserves_unrelated_sprites() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path();
        std::fs::create_dir_all(skin.join("fonts/custom")).unwrap();
        std::fs::write(
            skin.join("skin.ini"),
            "[Fonts]\nHitCirclePrefix: fonts/custom/digit\nScorePrefix: score\n",
        )
        .unwrap();
        for digit in 0..10 {
            RgbaImage::from_pixel(40, 60, Rgba([digit as u8, 200, 255, 255]))
                .save(skin.join(format!("fonts/custom/digit-{digit}@2x.png")))
                .unwrap();
        }
        let blank = RgbaImage::new(128, 128);
        blank.save(skin.join("hitcircle.png")).unwrap();
        let unrelated = RgbaImage::from_pixel(24, 30, Rgba([2, 3, 4, 255]));
        unrelated.save(skin.join("score-1@2x.png")).unwrap();
        unrelated.save(skin.join("default-1.png")).unwrap();
        let numbers = load(skin, None).unwrap();
        assert_eq!(numbers.digits[1].dimensions(), (20, 30));
        let options = crate::Options {
            copy: false,
            use_rafis: false,
            ..Default::default()
        };
        let preview = crate::preview(skin, options).unwrap();
        crate::process(skin, options).unwrap();
        assert_eq!(
            read_image(&skin.join("fonts/custom/digit-1.png")).unwrap(),
            preview.numbered.unwrap()
        );
        for digit in 0..10 {
            assert_eq!(
                read_image(&skin.join(format!("fonts/custom/digit-{digit}.png")))
                    .unwrap()
                    .dimensions(),
                (160, 160)
            );
            assert!(
                !skin
                    .join(format!("fonts/custom/digit-{digit}@2x.png"))
                    .exists()
            );
        }
        assert_eq!(read_image(&skin.join("score-1@2x.png")).unwrap(), unrelated);
        assert_eq!(read_image(&skin.join("default-1.png")).unwrap(), unrelated);
        assert!(
            SkinIni::load(&skin.join("skin.ini"))
                .unwrap()
                .source()
                .contains("HitCirclePrefix: fonts/custom/digit")
        );
    }
    #[test]
    fn missing_digits_get_fallbacks_but_intentional_transparency_is_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path();
        RgbaImage::new(1, 1)
            .save(skin.join("default-1.png"))
            .unwrap();
        let numbers = load(skin, None).unwrap();
        assert!(numbers.digits[0].pixels().any(|p| p[3] > 0));
        assert_eq!(numbers.digits[1].dimensions(), (1, 1));
        assert!(numbers.digits[1].pixels().all(|p| p[3] == 0));
        RgbaImage::new(128, 128)
            .save(skin.join("hitcircle.png"))
            .unwrap();
        let options = crate::Options {
            copy: false,
            use_rafis: false,
            ..Default::default()
        };
        crate::process(skin, options).unwrap();
        assert!(skin.join("skin.ini").is_file());
        for digit in 0..10 {
            assert!(skin.join(format!("default-{digit}.png")).is_file());
        }
        assert!(prefix("[Fonts]\nHitCirclePrefix: ../outside/default").is_err());
        assert!(prefix("[Fonts]\nHitCirclePrefix: C:/outside/default").is_err());
    }
    #[test]
    fn draft_prefix_and_root_number_fallback_are_respected() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path();
        RgbaImage::from_pixel(23, 31, Rgba([255, 255, 255, 255]))
            .save(skin.join("default-1.png"))
            .unwrap();
        let numbers = load(
            skin,
            Some("[Fonts]\nHitCirclePrefix: fonts/missing/digit // custom path\n"),
        )
        .unwrap();
        assert_eq!(numbers.prefix, PathBuf::from("fonts/missing/digit"));
        assert_eq!(numbers.digits[1].dimensions(), (23, 31));
    }
    #[test]
    #[ignore = "requires TRIPLESTACK_TEST_NUMBER_SKIN"]
    fn supplied_nm_skin_previews_and_exports_its_active_custom_font() {
        let source = PathBuf::from(std::env::var_os("TRIPLESTACK_TEST_NUMBER_SKIN").unwrap());
        let original_ini = std::fs::read(source.join("skin.ini")).unwrap();
        let numbers = load(&source, None).unwrap();
        assert_eq!(numbers.prefix, PathBuf::from("fonts/wc1/default"));
        assert_eq!(numbers.digits[1].dimensions(), (50, 60));
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path().join("skin");
        for entry in walkdir::WalkDir::new(&source).follow_links(false) {
            let entry = entry.unwrap();
            if entry.file_type().is_file()
                && (entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
                    || entry.file_name() == "skin.ini")
            {
                let target = skin.join(entry.path().strip_prefix(&source).unwrap());
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
        let options = crate::Options {
            copy: false,
            use_rafis: false,
            ..Default::default()
        };
        let preview = crate::preview(&skin, options).unwrap();
        crate::process(&skin, options).unwrap();
        assert_eq!(
            read_image(&skin.join("fonts/wc1/default-1.png")).unwrap(),
            preview.numbered.unwrap()
        );
        assert!(skin.join("fonts/hud/score-1@2x.png").is_file());
        assert!(skin.join("hitcircle@2x.png").is_file());
        let output = directory.path().join("NM.osk");
        crate::export_osk(&skin, &output).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
        for digit in 0..10 {
            assert!(
                archive
                    .by_name(&format!("fonts/wc1/default-{digit}.png"))
                    .is_ok()
            );
        }
        assert_eq!(
            std::fs::read(source.join("skin.ini")).unwrap(),
            original_ini
        );
    }
}
