pub mod beatmap_audio;
mod images;
pub mod number_fonts;
mod numbers;
pub mod skin_ini;

use anyhow::{Context, Result, anyhow, ensure};
use image::{ExtendedColorType, ImageEncoder, ImageReader, RgbaImage, codecs::png::PngEncoder};
use regex::Regex;
use std::{
    fs::{self, File},
    io::{self, Cursor, ErrorKind},
    path::{Path, PathBuf},
    sync::LazyLock,
};
use walkdir::WalkDir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const RAFIS_CIRCLE: &[u8] = include_bytes!("../../hitcircle.png");

#[derive(Clone, Copy)]
pub struct Options {
    pub copy: bool,
    pub force_resolution: Option<u32>,
    pub use_rafis: bool,
    pub overlay_on_top: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            copy: true,
            force_resolution: Some(160),
            use_rafis: true,
            overlay_on_top: false,
        }
    }
}

pub struct MissingCircle(PathBuf);

impl std::fmt::Display for MissingCircle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The selected skin has no hitcircle.png ({}).",
            self.0.display()
        )
    }
}

impl std::fmt::Debug for MissingCircle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for MissingCircle {}

pub struct Preview {
    pub original_circle: RgbaImage,
    pub circle: RgbaImage,
    pub numbered: Option<RgbaImage>,
    pub followpoints: Vec<(usize, RgbaImage)>,
    pub assets: Vec<(&'static str, RgbaImage)>,
}

pub fn preview(skin: &Path, options: Options) -> Result<Preview> {
    preview_with_ini(skin, options, None)
}

pub fn preview_with_ini(skin: &Path, options: Options, draft: Option<&str>) -> Result<Preview> {
    let numbers = numbers::load(skin, draft)?;
    let original_circle = if skin.join("hitcircle.png").is_file() {
        read_image(&skin.join("hitcircle.png"))?
    } else {
        rafis_circle()?
    };
    let circle = select_circle(skin, options)?;
    let resized = images::resize_circle(&circle, canvas_size(&circle, options));
    let numbered = Some(images::stack_number(&numbers.digits[1], &resized));
    let mut followpoint_paths = Vec::new();
    for entry in fs::read_dir(skin)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        if let Some(index) = name
            .strip_prefix("followpoint-")
            .and_then(|name| name.strip_suffix(".png"))
            .and_then(|index| index.parse::<usize>().ok())
        {
            followpoint_paths.push((index, path));
        }
    }
    followpoint_paths.sort_by_key(|(index, _)| *index);
    let followpoints = if followpoint_paths.is_empty() {
        let base = skin.join("followpoint.png");
        if base.is_file() {
            vec![(0, read_image(&base)?)]
        } else {
            Vec::new()
        }
    } else {
        followpoint_paths
            .into_iter()
            .map(|(index, path)| Ok((index, read_image(&path)?)))
            .collect::<Result<Vec<_>>>()?
    };
    let mut assets = Vec::new();
    for (label, file) in [
        ("Approach circle", "approachcircle.png"),
        ("Hitcircle overlay", "hitcircleoverlay.png"),
        ("Hitcircle", "hitcircle.png"),
        ("300", "hit300.png"),
        ("100", "hit100.png"),
        ("50", "hit50.png"),
        ("Cursor", "cursor.png"),
        ("Trail", "cursortrail.png"),
    ] {
        let mut path = skin.join(file);
        if label == "Cursor" && !path.is_file() {
            path = skin.join("cursor@2x.png");
        }
        if path.is_file() {
            assets.push((label, read_image(&path)?));
        }
    }
    assets.push(("Combo number", numbers.digits[1].clone()));
    // Use the same processing path as export, specifically for combo number 1.
    if let Some((_, number)) = assets.iter().find(|(label, _)| *label == "Combo number") {
        let resized = images::resize_circle(&circle, canvas_size(&circle, options));
        let stacked_one = images::stack_number(number, &resized);
        assets.push(("After combo number", stacked_one));
    }
    Ok(Preview {
        original_circle,
        circle,
        numbered,
        followpoints,
        assets,
    })
}

pub fn process(skin: &Path, options: Options) -> Result<PathBuf> {
    ensure!(
        !skin.join("client.realm").is_file(),
        "Choose a skin working copy with the osu!lazer skin browser, rather than processing the lazer data directory"
    );
    ensure!(
        skin.is_dir(),
        "Skin folder does not exist: {}",
        skin.display()
    );
    if let Some(size) = options.force_resolution {
        ensure!(
            (100..=160).contains(&size),
            "Offset must be between 100 and 160"
        );
    }
    let destination = if options.copy {
        copy_skin(skin)?
    } else {
        skin.to_owned()
    };
    let numbers = numbers::load(&destination, None)?;
    // Only remove HD counterparts of assets that are actually being replaced.
    remove_if_present(&destination.join("hitcircleoverlay@2x.png"))?;
    let overlay_path = destination.join("hitcircleoverlay.png");
    remove_if_present(&overlay_path)?;
    let circle = select_circle(&destination, options)?;
    save_image(&circle, &overlay_path)?;

    let resized = images::resize_circle(&circle, canvas_size(&circle, options));
    for (digit, number) in numbers.digits.iter().enumerate() {
        let path = numbers.path(&destination, digit, false);
        fs::create_dir_all(path.parent().unwrap())?;
        ensure!(
            path.parent()
                .unwrap()
                .canonicalize()?
                .starts_with(destination.canonicalize()?),
            "Number output leaves the skin folder"
        );
        save_image(&images::stack_number(number, &resized), &path)?;
        remove_if_present(&numbers.path(&destination, digit, true))?;
    }
    update_skin_ini(
        &destination.join("skin.ini"),
        options.force_resolution.unwrap_or(160),
    )?;
    Ok(destination)
}

pub fn save_skin_ini_with_numbers(skin: &Path, ini: &mut skin_ini::SkinIni) -> Result<()> {
    number_fonts::ensure_complete(skin, Some(ini.source()))?;
    ini.save(&skin.join("skin.ini"))
}

pub fn apply_skin_ini_draft(skin: &Path, source: &str, overlap: u32) -> Result<()> {
    let path = skin.join("skin.ini");
    let mut ini = skin_ini::SkinIni::load(&path)?;
    ini.set_source(source.to_owned());
    save_skin_ini_with_numbers(skin, &mut ini)?;
    update_skin_ini(&path, overlap)
}

fn select_circle(skin: &Path, options: Options) -> Result<RgbaImage> {
    let hitcircle = skin.join("hitcircle.png");
    if options.overlay_on_top && hitcircle.exists() {
        let mut base = read_image(&hitcircle)?;
        let rafis = rafis_circle()?;
        let resized = images::resize_circle(&rafis, base.dimensions());
        images::composite(&mut base, &resized);
        Ok(base)
    } else if options.overlay_on_top || options.use_rafis {
        rafis_circle()
    } else if hitcircle.exists() {
        read_image(&hitcircle)
    } else {
        Err(MissingCircle(hitcircle).into())
    }
}

fn rafis_circle() -> Result<RgbaImage> {
    Ok(image::load_from_memory_with_format(RAFIS_CIRCLE, image::ImageFormat::Png)?.into_rgba8())
}

fn read_image(path: &Path) -> Result<RgbaImage> {
    let open = || -> Result<_> {
        Ok(ImageReader::new(io::BufReader::new(File::open(path)?))
            .with_guessed_format()?
            .decode()?
            .into_rgba8())
    };
    open().with_context(|| format!("Reading {}", path.display()))
}

fn save_image(image: &RgbaImage, path: &Path) -> Result<()> {
    image
        .save_with_format(path, image::ImageFormat::Png)
        .with_context(|| format!("Writing {}", path.display()))
}

pub fn preview_png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    PngEncoder::new(Cursor::new(&mut output)).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )?;
    Ok(output)
}

pub fn native_resolution(skin: &Path) -> Result<Option<(u32, u32)>> {
    let path = skin.join("hitcircle.png");
    if !path.exists() {
        return Ok(None);
    }
    let dimensions = ImageReader::open(path)?
        .with_guessed_format()?
        .into_dimensions()?;
    Ok(Some(dimensions))
}

fn canvas_size(circle: &RgbaImage, options: Options) -> (u32, u32) {
    options
        .force_resolution
        .map_or_else(|| circle.dimensions(), |size| (size, size))
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Removing {}", path.display())),
    }
}

fn copy_skin(skin: &Path) -> Result<PathBuf> {
    let name = skin
        .file_name()
        .context("Select a skin folder inside a drive")?;
    let parent = skin.parent().context("The skin folder has no parent")?;
    let mut base_name = name.to_os_string();
    base_name.push("@3xStack");
    let mut copy = parent.join(&base_name);
    for index in 1.. {
        match fs::create_dir(&copy) {
            Ok(()) => break,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let mut next = base_name.clone();
                next.push(format!("({index})"));
                copy = parent.join(next);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Creating {}", copy.display()));
            }
        }
    }

    for entry in WalkDir::new(skin).min_depth(1).follow_links(true) {
        let entry = entry?;
        let path = copy.join(entry.path().strip_prefix(skin)?);
        if entry.file_type().is_dir() {
            fs::create_dir(&path).with_context(|| format!("Creating {}", path.display()))?;
        } else {
            fs::copy(entry.path(), &path)
                .with_context(|| format!("Copying {}", entry.path().display()))?;
        }
    }
    Ok(copy)
}

fn update_skin_ini(path: &Path, overlap: u32) -> Result<()> {
    static OVERLAP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?mi)^\s*HitCircleOverlap\s*:.*\n").expect("overlap expression")
    });
    static HAS_FONTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?mi)^\[Fonts\]").expect("section expression"));
    static FONTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?mi)^\[Fonts\]\s*$").expect("header expression"));

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("Reading {}", path.display())),
    };
    let utf16 = match bytes.as_slice() {
        [0xff, 0xfe, rest @ ..] if rest.len() % 2 == 0 => String::from_utf16(
            &rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .collect::<Vec<_>>(),
        )
        .ok(),
        [0xfe, 0xff, rest @ ..] if rest.len() % 2 == 0 => String::from_utf16(
            &rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_be_bytes(*b))
                .collect::<Vec<_>>(),
        )
        .ok(),
        _ => None,
    };
    let text = utf16.unwrap_or_else(|| bytes.into_iter().map(char::from).collect());
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut text = OVERLAP.replace_all(&text, "").into_owned();
    let value = format!("HitCircleOverlap: {overlap}");
    if HAS_FONTS.is_match(&text) {
        if let Some(header) = FONTS.find(&text) {
            text.insert_str(header.end(), &format!("\n{value}"));
        }
    } else {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("[Fonts]\n{value}\n"));
    }
    let mut encoded = vec![0xff, 0xfe];
    for unit in text.replace('\n', "\r\n").encode_utf16() {
        encoded.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(path, encoded).with_context(|| format!("Writing {}", path.display()))
}

pub fn export_osk(skin: &Path, output: &Path) -> Result<()> {
    let parent = output
        .parent()
        .context("The export file has no parent folder")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let temporary_path = temporary.path().to_owned();
    {
        let mut archive = ZipWriter::new(temporary.as_file_mut());
        let settings = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for entry in WalkDir::new(skin).min_depth(1) {
            let entry = entry?;
            let path = entry.path();
            if path == output || path == temporary_path || path.is_dir() {
                continue;
            }
            let relative = path.strip_prefix(skin)?;
            let name = relative
                .to_str()
                .ok_or_else(|| anyhow!("Invalid filename: {}", relative.display()))?;
            let mut file =
                File::open(path).with_context(|| format!("Reading {}", path.display()))?;
            archive.start_file(
                name.replace('\\', "/"),
                settings.large_file(file.metadata()?.len() >= u64::from(u32::MAX)),
            )?;
            io::copy(&mut file, &mut archive)?;
        }
        archive.finish()?;
    }
    temporary.persist(output)?;
    Ok(())
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn lazer_data_directories_cannot_be_processed_as_skin_folders() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("client.realm"), b"database").unwrap();
        assert!(process(directory.path(), Options::default()).is_err());
        assert_eq!(
            fs::read(directory.path().join("client.realm")).unwrap(),
            b"database"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
    #[test]
    fn animated_after_assets_match_exported_overlay_and_number_one() {
        let directory = tempfile::tempdir().unwrap();
        let skin = directory.path();
        let original = RgbaImage::from_pixel(128, 128, image::Rgba([100, 100, 100, 200]));
        let number = RgbaImage::from_pixel(30, 50, image::Rgba([255, 255, 255, 180]));
        original.save(skin.join("hitcircle.png")).unwrap();
        number.save(skin.join("default-1.png")).unwrap();
        let options = Options {
            copy: false,
            ..Options::default()
        };
        let draft = preview(skin, options).unwrap();
        let generated = &draft
            .assets
            .iter()
            .find(|(label, _)| *label == "After combo number")
            .unwrap()
            .1;
        assert_eq!(read_image(&skin.join("default-1.png")).unwrap(), number);
        process(skin, options).unwrap();
        assert_eq!(&read_image(&skin.join("default-1.png")).unwrap(), generated);
        assert_eq!(
            read_image(&skin.join("hitcircleoverlay.png")).unwrap(),
            draft.circle
        );
    }
}
