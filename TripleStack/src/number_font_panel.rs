use eframe::egui::{self, Color32, Ui};
use std::path::{Path, PathBuf};
use triplestack::{
    number_fonts::{self, NumberFont},
    skin_ini::SkinIni,
};

#[derive(Default)]
pub struct NumberFontPanel {
    skin: Option<PathBuf>,
    skin_fonts: Vec<String>,
    imported: Vec<NumberFont>,
    error: Option<String>,
    notice: Option<String>,
}
fn short(name: &str) -> String {
    let mut label: String = name.chars().take(48).collect();
    if name.chars().count() > 48 {
        label.push('\u{2026}');
    }
    label
}
impl NumberFontPanel {
    fn refresh(&mut self, skin: &Path) {
        self.skin = Some(skin.to_owned());
        match number_fonts::discover(skin) {
            Ok(fonts) => self.skin_fonts = fonts,
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        match number_fonts::imported() {
            Ok(fonts) => self.imported = fonts,
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }
    pub fn show(&mut self, ui: &mut Ui, skin: Option<&Path>, ini: &mut SkinIni) -> bool {
        let Some(skin) = skin else {
            return false;
        };
        if self.skin.as_deref() != Some(skin) {
            self.refresh(skin);
        }
        ui.label(
            egui::RichText::new("Hit-circle number font")
                .strong()
                .color(Color32::from_rgb(255, 102, 204)),
        );
        let current = ini.hit_circle_prefix().unwrap_or_else(|_| "default".into());
        let mut entries = vec![
            ("Skin / default".to_owned(), "default".to_owned()),
            (
                "Built-in fallback numbers".into(),
                "fonts/triplestack/fallback/default".into(),
            ),
        ];
        entries.extend(
            self.skin_fonts
                .iter()
                .filter(|prefix| prefix.as_str() != "default")
                .map(|prefix| (format!("Skin / {prefix}"), prefix.clone())),
        );
        entries.extend(
            number_fonts::bundled()
                .iter()
                .map(|font| (format!("Example / {}", font.name), font.skin_prefix())),
        );
        entries.extend(
            self.imported
                .iter()
                .map(|font| (format!("Imported / {}", font.name), font.skin_prefix())),
        );
        let label = entries
            .iter()
            .find(|(_, prefix)| prefix == &current)
            .map(|(name, _)| name.as_str())
            .unwrap_or(&current);
        let mut selected = None;
        egui::ComboBox::from_id_salt("number-font-selector")
            .width(ui.available_width())
            .height(180.0)
            .truncate()
            .selected_text(short(label))
            .show_ui(ui, |ui| {
                let row_height = ui.spacing().interact_size.y.max(24.0);
                egui::ScrollArea::vertical()
                    .id_salt("number-font-options")
                    .max_height(180.0)
                    .show_rows(ui, row_height, entries.len(), |ui, range| {
                        for index in range {
                            let (name, prefix) = &entries[index];
                            if ui
                                .add_sized(
                                    [ui.available_width(), row_height],
                                    egui::Button::selectable(prefix == &current, short(name))
                                        .truncate(),
                                )
                                .on_hover_text(format!("{name}\n{prefix}"))
                                .clicked()
                            {
                                selected = Some(prefix.clone());
                                ui.close();
                            }
                        }
                    });
            });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Import font folder").clicked() {
                let directory = skin.join("fonts");
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Choose numbered PNG font sets (default-0.png to default-9.png)");
                if directory.is_dir() {
                    dialog = dialog.set_directory(directory);
                }
                if let Some(folder) = dialog.pick_folder() {
                    match number_fonts::import_folder(&folder) {
                        Ok(fonts) => {
                            self.error = None;
                            self.notice = Some(format!(
                                "Imported {} font sets. Missing digits were filled automatically.",
                                fonts.len()
                            ));
                            if fonts.len() == 1 {
                                selected = Some(fonts[0].skin_prefix());
                            }
                            self.refresh(skin);
                        }
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
            }
            if ui.button("Refresh fonts").clicked() {
                self.refresh(skin);
            }
        });
        ui.small("Select a font to replace the hit-circle numbers. Score and combo-counter fonts stay separate.");
        ui.small("Missing or unreadable digits use fallback numbers. Saving or exporting includes a complete 0-9 set.");
        let mut changed = false;
        if let Some(prefix) = selected {
            match ini.set_hit_circle_prefix(&prefix) {
                Ok(()) => {
                    changed = prefix != current;
                    self.error = None;
                }
                Err(error) => self.error = Some(format!("{error:#}")),
            }
        }
        if let Some(notice) = &self.notice {
            ui.add(egui::Label::new(notice).wrap());
        }
        if let Some(error) = &self.error {
            ui.add(egui::Label::new(egui::RichText::new(error).color(Color32::LIGHT_RED)).wrap());
        }
        changed
    }
}
