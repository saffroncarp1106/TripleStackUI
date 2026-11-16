use eframe::egui::{
    self, Color32, FontId, TextFormat, Ui,
    text::{CCursor, CCursorRange, LayoutJob},
};
use std::ops::Range;

#[derive(Default)]
pub struct EditorSearch {
    open: bool,
    query: String,
    matches: Vec<Range<usize>>,
    current: usize,
    last_query: String,
    last_text: String,
}

fn matches(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
        .map(|pattern| pattern.find_iter(text).map(|m| m.range()).collect())
        .unwrap_or_default()
}

impl EditorSearch {
    fn refresh(&mut self, text: &str) -> bool {
        if self.last_query == self.query && self.last_text == text {
            return false;
        }
        self.matches = matches(text, &self.query);
        if self.last_query != self.query {
            self.current = 0;
        }
        self.current = self.current.min(self.matches.len().saturating_sub(1));
        self.last_query.clone_from(&self.query);
        self.last_text = text.to_owned();
        true
    }

    fn advance(&mut self, previous: bool) {
        if !self.matches.is_empty() {
            self.current = if previous {
                (self.current + self.matches.len() - 1) % self.matches.len()
            } else {
                (self.current + 1) % self.matches.len()
            };
        }
    }

    pub fn show(&mut self, ui: &mut Ui, code: &mut String) -> bool {
        let editor_id = ui.id().with("skin-ini-code-editor");
        let search_id = ui.id().with("skin-ini-find");
        let shortcut = ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F));
        let open_clicked = if !self.open {
            ui.small_button("Search").clicked()
        } else {
            false
        };
        let opening = shortcut || open_clicked;
        if opening {
            self.open = true;
        }
        let mut jump = self.refresh(code) && self.open;
        let mut previous = false;
        let mut next = false;
        if self.open {
            let searching = ui.memory(|memory| memory.has_focus(search_id));
            if searching {
                previous = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter));
                next = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            }
            let mut close =
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            ui.horizontal(|ui| {
                ui.label("Find:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(search_id)
                        .hint_text("Search skin.ini")
                        .desired_width((ui.available_width() - 10.0).max(50.0)),
                );
                if opening {
                    response.request_focus();
                    response.scroll_to_me(Some(egui::Align::Center));
                }
            });
            jump |= self.refresh(code);
            ui.horizontal_wrapped(|ui| {
                previous |= ui
                    .add_enabled(!self.matches.is_empty(), egui::Button::new("Previous"))
                    .clicked();
                next |= ui
                    .add_enabled(!self.matches.is_empty(), egui::Button::new("Next"))
                    .clicked();
                ui.label(if self.query.is_empty() {
                    "Type to search".to_owned()
                } else if self.matches.is_empty() {
                    "No matches".to_owned()
                } else {
                    format!("{} of {}", self.current + 1, self.matches.len())
                });
                close |= ui.small_button("Close").clicked();
            });
            if previous || next {
                self.advance(previous);
                jump = true;
            }
            if close {
                self.open = false;
                ui.memory_mut(|memory| memory.request_focus(editor_id));
            }
        }
        jump |= opening;
        let selected = if self.open {
            self.matches.get(self.current).cloned()
        } else {
            None
        };
        let query = if self.open {
            self.query.clone()
        } else {
            String::new()
        };
        let current = self.current;
        let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
            let source = text.as_str();
            let mut job = LayoutJob::default();
            job.wrap.max_width = wrap_width;
            let format = TextFormat {
                font_id: FontId::monospace(12.0),
                color: ui.visuals().text_color(),
                ..Default::default()
            };
            let mut end = 0;
            for (index, found) in matches(source, &query).into_iter().enumerate() {
                job.append(&source[end..found.start], 0.0, format.clone());
                job.append(
                    &source[found.clone()],
                    0.0,
                    TextFormat {
                        background: if index == current {
                            Color32::from_rgb(180, 115, 20)
                        } else {
                            Color32::from_rgb(85, 70, 30)
                        },
                        color: Color32::WHITE,
                        ..format.clone()
                    },
                );
                end = found.end;
            }
            job.append(&source[end..], 0.0, format);
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        egui::ScrollArea::both()
            .id_salt("skin-ini-source")
            .max_height(220.0)
            .show(ui, |ui| {
                let mut output = egui::TextEdit::multiline(code)
                    .id(editor_id)
                    .font(FontId::monospace(12.0))
                    .code_editor()
                    .desired_width(ui.available_width())
                    .desired_rows(12)
                    .layouter(&mut layouter)
                    .show(ui);
                if jump
                    && !output.response.changed()
                    && let Some(found) = selected
                {
                    let start = code[..found.start].chars().count();
                    let end = start + code[found].chars().count();
                    output.state.cursor.set_char_range(Some(CCursorRange::two(
                        CCursor::new(start),
                        CCursor::new(end),
                    )));
                    output.state.store(ui.ctx(), editor_id);
                    let rect = output
                        .galley
                        .pos_from_cursor(CCursor::new(start))
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(rect.expand(12.0), Some(egui::Align::Center));
                    ui.ctx().request_repaint();
                }
                output.response.changed()
            })
            .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_case_insensitive_search_handles_unicode_and_empty_queries() {
        assert_eq!(
            matches("\u{30b3}\u{30f3}\u{30dc} Combo1 COMBO1", "combo1"),
            vec![10..16, 17..23]
        );
        assert_eq!(matches("[General] General", "[general]"), vec![0..9]);
        assert!(matches("Combo1", "").is_empty());
        assert!(matches("Combo1", "missing").is_empty());
    }
    #[test]
    fn navigation_wraps_and_refreshes_after_source_changes() {
        let mut search = EditorSearch {
            query: "Combo".into(),
            ..Default::default()
        };
        assert!(search.refresh("Combo1 Combo2"));
        search.advance(true);
        assert_eq!(search.current, 1);
        search.advance(false);
        assert_eq!(search.current, 0);
        search.advance(false);
        search.refresh("Combo2");
        assert_eq!(search.current, 0);
        assert_eq!(search.matches, vec![0..5]);
        search.refresh("");
        search.advance(true);
        assert!(search.matches.is_empty());
    }
    #[test]
    fn keyboard_search_navigates_and_closes_without_modifying_source() {
        let ctx = egui::Context::default();
        let mut search = EditorSearch::default();
        let original = "[Colours]\nCombo1: 1,2,3\nCombo2: 4,5,6\n";
        let mut code = original.to_owned();
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let mut frame = |search: &mut EditorSearch, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        search.show(ui, &mut code);
                    });
                },
            );
        };
        frame(&mut search, vec![]);
        frame(&mut search, vec![key(egui::Key::F, egui::Modifiers::CTRL)]);
        assert!(search.open);
        frame(&mut search, vec![egui::Event::Text("combo".into())]);
        assert_eq!(search.query, "combo");
        assert_eq!(search.matches.len(), 2);
        frame(
            &mut search,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(search.current, 1);
        frame(
            &mut search,
            vec![key(egui::Key::Enter, egui::Modifiers::SHIFT)],
        );
        assert_eq!(search.current, 0);
        frame(
            &mut search,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(!search.open);
        assert_eq!(code, original);
    }
}
