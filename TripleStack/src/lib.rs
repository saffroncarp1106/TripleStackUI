mod images;

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
const FOLLOWPOINT: &[u8] = include_bytes!("../../followpoint.png");

#[derive(Clone, Copy)]
pub struct Options {
    pub copy: bool,
    pub force_resolution: Option<u32>,
    pub use_rafis: bool,
    pub overlay_on_top: bool,
    pub brighter_followpoint: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            copy: true,
            force_resolution: Some(160),
            use_rafis: true,
            overlay_on_top: false,
            brighter_followpoint: false,
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
    pub circle: RgbaImage,
    pub numbered: Option<RgbaImage>,
}

pub fn preview(skin: &Path, options: Options) -> Result<Preview> {
    let circle = select_circle(skin, options)?;
    let numbered = match number_paths(skin)?.into_iter().next() {
        Some(path) => {
            let size = canvas_size(&circle, options);
            let resized = images::resize_circle(&circle, size);
            Some(images::stack_number(&read_image(&path)?, &resized))
        }
        None => None,
    };
    Ok(Preview { circle, numbered })
}

pub fn process(skin: &Path, options: Options) -> Result<PathBuf> {
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
    delete_2x_assets(&destination)?;
    if options.brighter_followpoint {
        replace_followpoints(&destination)?;
    }
    let overlay_path = destination.join("hitcircleoverlay.png");
    remove_if_present(&overlay_path)?;
    let circle = select_circle(&destination, options)?;
    save_image(&circle, &overlay_path)?;

    let resized = images::resize_circle(&circle, canvas_size(&circle, options));
    for path in number_paths(&destination)? {
        let number = read_image(&path)?;
        save_image(&images::stack_number(&number, &resized), &path)?;
    }
    update_skin_ini(
        &destination.join("skin.ini"),
        options.force_resolution.unwrap_or(160),
    )?;
    Ok(destination)
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

fn number_paths(skin: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(skin)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".png")
            && name
                .strip_prefix("default-")
                .is_some_and(|rest| rest.as_bytes().first().is_some_and(u8::is_ascii_digit))
        {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

fn delete_2x_assets(skin: &Path) -> Result<()> {
    for entry in fs::read_dir(skin)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("@2x")
        {
            remove_if_present(&entry.path())?;
        }
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Removing {}", path.display())),
    }
}

fn replace_followpoints(skin: &Path) -> Result<()> {
    for entry in fs::read_dir(skin)? {
        let path = entry?.path();
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if stem
            .strip_prefix("followpoint-")
            .is_some_and(|index| index.len() == 1 && index.as_bytes()[0].is_ascii_digit())
        {
            remove_if_present(&path)?;
        }
    }
    let base = skin.join("followpoint.png");
    // The original treats replacement as best-effort and falls back to the skin's image.
    if fs::write(&base, FOLLOWPOINT).is_ok() || base.exists() {
        for frame in 0..10 {
            let _ = fs::copy(&base, skin.join(format!("followpoint-{frame}.png")));
        }
    }
    Ok(())
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
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
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
