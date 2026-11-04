//! Preferences: the settings the app keeps between runs, in one window. Theme, the size of the editor's text, validating while typing,
//! what Entry Helpers insert, working offline, the cache of downloaded schemas, and the catalogs the person named. Everything here takes
//! effect at once; there is nothing to apply or cancel.

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};
use crate::kit::{self, PillKind};
use crate::theme::ThemePreference;
use crate::widgets;

impl XmlCraftApp {
    /// Use the catalog at `path`, before the schemas that ship with the app (a path that is listed already is not added twice).
    pub fn add_user_catalog(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() || self.user_catalogs.iter().any(|p| p == path) {
            return;
        }
        self.user_catalogs.push(path.to_owned());
        self.catalogs_changed();
    }

    /// Stop using the catalog at `index` of [`XmlCraftApp::user_catalogs`].
    pub fn remove_user_catalog(&mut self, index: usize) {
        if index < self.user_catalogs.len() {
            self.user_catalogs.remove(index);
            self.catalogs_changed();
        }
    }

    /// The catalogs the person named.
    pub fn user_catalogs(&self) -> &[String] {
        &self.user_catalogs
    }

    /// The resolvers made from now on use the catalogs as they are, and what was asked for before is asked for again: a schema that no
    /// catalog knew may be found now.
    fn catalogs_changed(&mut self) {
        self.network.set_user_catalogs(self.user_catalogs.clone());
        self.forget_schema_requests();
    }

    /// Delete the downloaded schemas; they are downloaded again when a document names one.
    pub fn clear_download_cache(&mut self) {
        let removed = self.network.clear_cache();
        self.status = fmt(tl!("Deleted {n} downloaded files"), &[("n", &removed.to_string())]);
        self.forget_schema_requests();
    }

    pub(crate) fn show_preferences(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };
        let (done, closed) = kit::dialog(
            ctx,
            "xmlcraft-preferences",
            560.0,
            "settings",
            tl!("Preferences"),
            tl!("Every change takes effect at once and is kept for the next time."),
            |ui| self.preferences_ui(ctx, ui),
        );
        if !done && !closed {
            self.dialog = Some(dialog);
        }
    }

    fn preferences_ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) -> bool {
        kit::section(ui, tl!("Appearance"), |ui| {
            kit::row(ui, tl!("Theme"), "", |ui| {
                kit::segmented(
                    ui,
                    &mut self.theme_preference,
                    &[(ThemePreference::System, tl!("System")), (ThemePreference::Light, tl!("Light")), (ThemePreference::Dark, tl!("Dark"))],
                );
            });
            kit::row(ui, tl!("Editor text size"), tl!("Also in the View menu: Zoom In, Zoom Out and Actual Size."), |ui| {
                ui.horizontal(|ui| {
                    if crate::icons::button(ui, "minus", 28.0, false, tl!("Zoom Out")).clicked() {
                        self.zoom_editor(Some(-1));
                    }
                    ui.label(format!("{}%", (self.editor_zoom() * 100.0).round() as i32));
                    if crate::icons::button(ui, "plus", 28.0, false, tl!("Zoom In")).clicked() {
                        self.zoom_editor(Some(1));
                    }
                    if kit::pill(ui, PillKind::Ghost, None, tl!("Actual Size")).clicked() {
                        self.zoom_editor(None);
                    }
                });
            });
        });
        kit::section(ui, tl!("Editing"), |ui| {
            kit::row(ui, tl!("Validation"), tl!("A document that names a grammar is checked in the background after you stop typing."), |ui| {
                ui.add(widgets::checkbox(&mut self.validate_typing, tl!("Validate while typing")));
            });
            kit::row(ui, tl!("Entry Helpers"), tl!("Inserting an element also writes what its declaration requires."), |ui| {
                ui.add(widgets::checkbox(&mut self.entry_mandatory, tl!("Insert required content")));
            });
        });
        kit::section(ui, tl!("Schemas"), |ui| {
            kit::row(
                ui,
                tl!("Downloads"),
                tl!("Offline, a schema or a DTD that is not on this computer is taken from the copies downloaded before."),
                |ui| {
                    let mut offline = self.work_offline;
                    if ui.add(widgets::checkbox(&mut offline, tl!("Work offline"))).changed() {
                        self.set_work_offline(offline);
                    }
                },
            );
            let stats = self.network.cache_stats();
            kit::row(ui, tl!("Downloaded files"), tl!("Schemas and DTDs kept so that they are not downloaded again."), |ui| {
                ui.horizontal(|ui| {
                    ui.label(fmt(tl!("{n} files, {size}"), &[("n", &stats.entries.to_string()), ("size", &human_size(stats.bytes))]));
                    ui.add_enabled_ui(stats.entries > 0, |ui| {
                        if kit::pill(ui, PillKind::Secondary, Some("trash-2"), tl!("Clear Cache")).clicked() {
                            self.clear_download_cache();
                        }
                    });
                });
            });
            kit::row(ui, tl!("Catalogs"), tl!("OASIS catalogs you add are consulted before the schemas that ship with the app."), |ui| {
                let mut remove = None;
                for (i, path) in self.user_catalogs.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if crate::icons::button(ui, "x", 24.0, false, &fmt(tl!("Remove {path}"), &[("path", path)])).clicked() {
                            remove = Some(i);
                        }
                        ui.add(egui::Label::new(path.as_str()).truncate());
                    });
                }
                if let Some(i) = remove {
                    self.remove_user_catalog(i);
                }
                if kit::pill(ui, PillKind::Secondary, Some("plus"), tl!("Add Catalog…")).clicked() {
                    self.pick_catalog(ctx);
                }
            });
        });
        kit::close_footer(ui, tl!("Done"))
    }
}

/// A size in bytes as a person reads it.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_written_with_the_unit_it_needs() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert!(human_size(u64::MAX).ends_with("GB"));
    }
}
