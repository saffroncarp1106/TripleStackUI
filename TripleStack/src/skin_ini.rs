use anyhow::{Context, Result, anyhow};
use std::{fs, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComboColor {
    pub number: u32,
    pub rgb: [u8; 3],
}

#[derive(Clone, Copy)]
enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Latin1,
}

pub struct SkinIni {
    text: String,
    undo_history: Vec<String>,
    encoding: Encoding,
    newline: &'static str,
    pub colors: Vec<ComboColor>,
}

impl SkinIni {
    pub fn source(&self) -> &str {
        &self.text
    }
    pub fn from_source(text: String) -> Self {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let colors = parse_colors(&text);
        Self {
            text,
            undo_history: Vec::new(),
            encoding: Encoding::Utf8,
            newline,
            colors,
        }
    }
    pub fn hit_circle_prefix(&self) -> Result<String> {
        crate::number_fonts::validate_prefix(self.source())
    }
    pub fn set_hit_circle_prefix(&mut self, prefix: &str) -> Result<()> {
        anyhow::ensure!(!prefix.contains(['\r', '\n']), "Invalid number-font prefix");
        let prefix =
            crate::number_fonts::validate_prefix(&format!("[Fonts]\nHitCirclePrefix: {prefix}"))?;
        let mut lines: Vec<String> = self.text.lines().map(str::to_owned).collect();
        let mut in_fonts = false;
        let mut insertion = None;
        let mut replaced = false;
        for (index, line) in lines.iter_mut().enumerate() {
            let content = line.split("//").next().unwrap_or("").trim();
            if content.starts_with('[') {
                in_fonts = content.eq_ignore_ascii_case("[Fonts]");
            } else if in_fonts
                && let Some((key, _)) = content.split_once(':')
                && key.trim().eq_ignore_ascii_case("HitCirclePrefix")
            {
                let key = line.split_once(':').unwrap().0.to_owned();
                let comment = line
                    .find("//")
                    .map(|i| format!(" {}", &line[i..]))
                    .unwrap_or_default();
                *line = format!("{key}: {prefix}{comment}");
                replaced = true;
            }
            if in_fonts {
                insertion = Some(index + 1);
            }
        }
        if !replaced {
            let setting = format!("HitCirclePrefix: {prefix}");
            if let Some(index) = insertion {
                lines.insert(index, setting);
            } else {
                lines.push("[Fonts]".into());
                lines.push(setting);
            }
        }
        let mut text = lines.join(self.newline);
        if self.text.ends_with('\n') {
            text.push_str(self.newline);
        }
        self.set_source(text);
        Ok(())
    }

    pub fn set_source(&mut self, text: String) {
        if self.text == text {
            return;
        }
        if self.undo_history.len() == 100 {
            self.undo_history.remove(0);
        }
        self.undo_history.push(self.text.clone());
        self.replace_source(text);
    }

    fn replace_source(&mut self, text: String) {
        self.newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        self.colors = parse_colors(&text);
        self.text = text;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_history.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(text) = self.undo_history.pop() else {
            return false;
        };
        self.replace_source(text);
        true
    }

    pub fn set_color(&mut self, number: u32, rgb: [u8; 3]) {
        if let Some(color) = self.colors.iter_mut().find(|color| color.number == number) {
            color.rgb = rgb;
        } else {
            self.colors.push(ComboColor { number, rgb });
        }
        self.set_source(self.rewritten_text());
    }

    pub fn remove_color(&mut self, number: u32) {
        self.colors.retain(|color| color.number != number);
        self.set_source(self.rewritten_text());
    }

    pub fn cursor_expand(&self) -> bool {
        let mut general = false;
        let mut enabled = true;
        for line in self.text.lines() {
            let content = line.split("//").next().unwrap_or("").trim();
            if content.starts_with('[') && content.ends_with(']') {
                general = content.eq_ignore_ascii_case("[General]");
            } else if general
                && let Some((key, value)) = content.split_once(':')
                && key.trim().eq_ignore_ascii_case("CursorExpand")
            {
                match value.trim() {
                    "0" => enabled = false,
                    "1" => enabled = true,
                    _ => {}
                }
            }
        }
        enabled
    }

    pub fn set_cursor_expand(&mut self, enabled: bool) {
        let value = u8::from(enabled);
        let mut lines: Vec<String> = self.text.lines().map(str::to_owned).collect();
        let mut general = false;
        let mut insertion = None;
        let mut replaced = false;
        for (index, line) in lines.iter_mut().enumerate() {
            let content = line.split("//").next().unwrap_or("").trim();
            if content.starts_with('[') && content.ends_with(']') {
                general = content.eq_ignore_ascii_case("[General]");
            } else if general
                && let Some((key, _)) = content.split_once(':')
                && key.trim().eq_ignore_ascii_case("CursorExpand")
            {
                let prefix = line.split_once(':').unwrap().0;
                let comment = line
                    .find("//")
                    .map(|i| format!(" {}", &line[i..]))
                    .unwrap_or_default();
                *line = format!("{prefix}: {value}{comment}");
                replaced = true;
            }
            if general {
                insertion = Some(index + 1);
            }
        }
        if !replaced {
            let setting = format!("CursorExpand: {value}");
            if let Some(index) = insertion {
                lines.insert(index, setting);
            } else {
                lines.push("[General]".to_owned());
                lines.push(setting);
            }
        }
        let mut text = lines.join(self.newline);
        if self.text.ends_with('\n') {
            text.push_str(self.newline);
        }
        self.set_source(text);
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("Reading {}", path.display()))?;
        let (text, encoding) = if bytes.starts_with(&[0xff, 0xfe]) {
            (decode_utf16(&bytes[2..], true)?, Encoding::Utf16Le)
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            (decode_utf16(&bytes[2..], false)?, Encoding::Utf16Be)
        } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            (String::from_utf8(bytes[3..].to_vec())?, Encoding::Utf8Bom)
        } else if let Ok(text) = String::from_utf8(bytes.clone()) {
            (text, Encoding::Utf8)
        } else {
            (
                bytes.into_iter().map(char::from).collect(),
                Encoding::Latin1,
            )
        };
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut ini = Self {
            text,
            undo_history: Vec::new(),
            encoding,
            newline,
            colors: Vec::new(),
        };
        ini.colors = parse_colors(&ini.text);
        Ok(ini)
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.text = self.rewritten_text();
        let text = &self.text;
        let bytes = match self.encoding {
            Encoding::Utf8 => text.as_bytes().to_vec(),
            Encoding::Utf8Bom => [vec![0xef, 0xbb, 0xbf], text.as_bytes().to_vec()].concat(),
            Encoding::Utf16Le => [
                vec![0xff, 0xfe],
                text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            ]
            .concat(),
            Encoding::Utf16Be => [
                vec![0xfe, 0xff],
                text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
            ]
            .concat(),
            Encoding::Latin1 => text
                .chars()
                .map(|c| if (c as u32) <= 255 { c as u8 } else { b'?' })
                .collect(),
        };
        fs::write(path, bytes).with_context(|| format!("Writing {}", path.display()))?;
        Ok(())
    }

    fn rewritten_text(&self) -> String {
        let mut section = false;
        let mut lines = Vec::new();
        let mut replaced = Vec::new();
        let mut insertion = None;
        for line in self.text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                if section {
                    insertion = Some(lines.len());
                }
                section = trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("colours")
                    || trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("colors")
                    || trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("combo");
            }
            if section {
                if let Some((number, original)) = parse_combo(line) {
                    if let Some(color) = self.colors.iter().find(|c| c.number == number) {
                        if color.rgb == original {
                            lines.push(line.to_owned());
                        } else {
                            let prefix =
                                line.split_once(':').map(|(key, _)| key).unwrap_or("Combo1");
                            let comment = line
                                .find("//")
                                .map(|index| format!(" {}", &line[index..]))
                                .unwrap_or_default();
                            lines.push(format!(
                                "{prefix}: {},{},{}{}",
                                color.rgb[0], color.rgb[1], color.rgb[2], comment
                            ));
                        }
                        replaced.push(number);
                    }
                    continue;
                }
            }
            lines.push(line.to_owned());
        }
        if section {
            insertion = Some(lines.len());
        }
        let additions: Vec<_> = self
            .colors
            .iter()
            .filter(|c| !replaced.contains(&c.number))
            .map(|c| format!("Combo{}: {},{},{}", c.number, c.rgb[0], c.rgb[1], c.rgb[2]))
            .collect();
        if let Some(index) = insertion {
            lines.splice(index..index, additions);
        } else if !additions.is_empty() {
            lines.push("[Colours]".to_owned());
            lines.extend(additions);
        }
        let mut text = lines.join(self.newline);
        if self.text.ends_with('\n') {
            text.push_str(self.newline);
        }
        text
    }
}

fn decode_utf16(bytes: &[u8], le: bool) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err(anyhow!("Invalid UTF-16 skin.ini"));
    }
    let units = bytes
        .chunks_exact(2)
        .map(|b| {
            if le {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        })
        .collect::<Vec<_>>();
    Ok(String::from_utf16(&units)?)
}

fn parse_combo(line: &str) -> Option<(u32, [u8; 3])> {
    let line = line.split("//").next()?.trim();
    let (key, value) = line.split_once(':')?;
    let number = key
        .trim()
        .to_ascii_lowercase()
        .strip_prefix("combo")?
        .parse()
        .ok()?;
    let parts = value
        .split(',')
        .map(str::trim)
        .take(3)
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if parts.len() != 3 {
        return None;
    }
    Some((number, [parts[0], parts[1], parts[2]]))
}

fn parse_colors(text: &str) -> Vec<ComboColor> {
    let mut section = false;
    let mut colors: Vec<ComboColor> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("colours")
                || trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("colors")
                || trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("combo");
        } else if section && let Some((number, rgb)) = parse_combo(line) {
            if let Some(existing) = colors.iter_mut().find(|c| c.number == number) {
                existing.rgb = rgb;
            } else {
                colors.push(ComboColor { number, rgb });
            }
        }
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_colors_stay_deleted_after_saving_and_reloading() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("skin.ini");
        fs::write(&path, "[Colours]\r\nCombo1: 255,0,0\r\nCombo2: 0,255,0\r\nSliderBorder: 255,255,255\r\n[General]\r\nName: Example\r\n").unwrap();
        let mut ini = SkinIni::load(&path).unwrap();
        ini.remove_color(1);
        assert_eq!(ini.colors.len(), 1);
        assert_eq!(ini.colors[0].number, 2);
        assert!(!ini.source().contains("Combo1:"));
        assert!(ini.source().contains("SliderBorder: 255,255,255\r\n"));
        assert!(ini.source().contains("Name: Example\r\n"));
        ini.save(&path).unwrap();
        let mut loaded = SkinIni::load(&path).unwrap();
        assert_eq!(loaded.colors, ini.colors);
        loaded.remove_color(2);
        loaded.save(&path).unwrap();
        assert!(SkinIni::load(&path).unwrap().colors.is_empty());
    }
    #[test]
    fn color_controls_and_source_stay_in_sync_in_both_directions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("skin.ini");
        fs::write(&path, "[General]\nName: Example\n").unwrap();
        let mut ini = SkinIni::load(&path).unwrap();
        // Adding to a skin without a color section must create visible source.
        ini.set_color(1, [255, 192, 0]);
        ini.set_color(2, [10, 20, 30]);
        assert!(ini.source().contains("[Colours]\nCombo1: 255,192,0"));
        assert_eq!(parse_colors(ini.source()), ini.colors);
        ini.set_color(2, [40, 50, 60]);
        assert!(ini.source().contains("Combo2: 40,50,60"));
        assert_eq!(parse_colors(ini.source()), ini.colors);
        ini.remove_color(1);
        assert!(!ini.source().contains("Combo1:"));
        assert_eq!(parse_colors(ini.source()), ini.colors);
        // Typed additions, edits, and deletions refresh the control/preview data.
        ini.set_source("[Colours]\nCombo2: 1,2,3\nCombo3: 4,5,6\n".to_owned());
        assert_eq!(
            ini.colors,
            vec![
                ComboColor {
                    number: 2,
                    rgb: [1, 2, 3]
                },
                ComboColor {
                    number: 3,
                    rgb: [4, 5, 6]
                },
            ]
        );
        ini.set_source("[Colours]\nCombo3: 7,8,9\n".to_owned());
        assert_eq!(
            ini.colors,
            vec![ComboColor {
                number: 3,
                rgb: [7, 8, 9]
            }]
        );
        ini.remove_color(3);
        assert!(parse_colors(ini.source()).is_empty());
        ini.set_color(1, [100, 110, 120]);
        assert_eq!(parse_colors(ini.source()), ini.colors);
    }
    #[test]
    fn undo_restores_source_and_colors_for_every_edit_type() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("skin.ini");
        let original = "[Colours]\r\nCombo1: 255,0,0 // red\r\n";
        fs::write(&path, original).unwrap();
        let mut ini = SkinIni::load(&path).unwrap();
        assert!(!ini.can_undo());
        let mut states = vec![ini.source().to_owned()];
        ini.set_color(2, [10, 20, 30]);
        states.push(ini.source().to_owned());
        ini.set_color(1, [40, 50, 60]);
        states.push(ini.source().to_owned());
        ini.remove_color(1);
        states.push(ini.source().to_owned());
        ini.set_source("[Colours]\nCombo3: 7,8,9\n".to_owned());
        // A no-op edit must not consume an undo step.
        ini.set_source(ini.source().to_owned());
        for expected in states.iter().rev() {
            assert!(ini.undo());
            assert_eq!(ini.source(), expected);
            assert_eq!(ini.colors, parse_colors(expected));
        }
        assert_eq!(ini.source(), original);
        assert!(!ini.undo());
        assert!(!ini.can_undo());
        ini.save(&path).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }
    #[test]
    fn cursor_expand_updates_only_general_preserving_comments_and_undo() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("skin.ini");
        let original = "[General]\r\nName: Example\r\nCursorExpand: 1 // click\r\nCursorExpand: 1\r\n[Other]\r\nCursorExpand: 1\r\n[Colours]\r\nCombo1: 1,2,3\r\n";
        fs::write(&path, original).unwrap();
        let mut ini = SkinIni::load(&path).unwrap();
        assert!(ini.cursor_expand());
        ini.set_cursor_expand(false);
        assert!(!ini.cursor_expand());
        assert!(
            ini.source()
                .contains("CursorExpand: 0 // click\r\nCursorExpand: 0\r\n")
        );
        assert!(ini.source().contains("[Other]\r\nCursorExpand: 1"));
        assert_eq!(
            ini.colors,
            vec![ComboColor {
                number: 1,
                rgb: [1, 2, 3]
            }]
        );
        ini.save(&path).unwrap();
        assert!(!SkinIni::load(&path).unwrap().cursor_expand());
        assert!(ini.undo());
        assert_eq!(ini.source(), original);
        assert!(ini.cursor_expand());
    }

    #[test]
    fn cursor_expand_can_be_inserted_and_changed_through_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("skin.ini");
        for original in [
            "",
            "[General]\nName: Example\n[Colours]\nCombo1: 1,2,3\n",
            "[Colours]\nCombo1: 1,2,3",
        ] {
            fs::write(&path, original).unwrap();
            let mut ini = SkinIni::load(&path).unwrap();
            assert!(ini.cursor_expand());
            ini.set_cursor_expand(false);
            assert!(!ini.cursor_expand());
            assert_eq!(ini.source().matches("[General]").count(), 1);
            ini.set_cursor_expand(true);
            assert!(ini.cursor_expand());
            ini.set_source(ini.source().replace("CursorExpand: 1", "CursorExpand: 0"));
            assert!(!ini.cursor_expand());
        }
    }
}
