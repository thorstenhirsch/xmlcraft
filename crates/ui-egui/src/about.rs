//! The About window: what XmlCraft is and who made it. The credits are compiled into the binary (`contributors/contributors.json`, made by
//! `cargo xtask contributors`; `docs/contributors.md`) and so is the NOTICE of third-party material: nothing is read at run time.

use egui::vec2;
use serde_json::Value;

use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, widgets};

/// The credits as the generator wrote them.
const CONTRIBUTORS_JSON: &str = include_str!("../../../contributors/contributors.json");
/// The third-party material the product carries, with its licences.
const NOTICE: &str = include_str!("../../../NOTICE");

/// Someone who contributed, as far as the credits say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Contributor {
    pub login: String,
    /// The name they chose to be shown by (their own `people.toml` entry, verified by the generator).
    pub display_name: Option<String>,
    /// The name they said is theirs, shown only when the viewer asks for it.
    pub real_name: Option<String>,
    pub prs: u64,
    pub commits: u64,
    pub lines_added: u64,
    pub lines_deleted: u64,
}

/// An AI model named in the `Co-Authored-By` trailers of commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Model {
    pub name: String,
    pub commits: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Credits {
    pub commits: u64,
    pub contributors: Vec<Contributor>,
    pub models: Vec<Model>,
}

/// The credits in `json`. Total: anything that is not what the generator writes gives fewer credits, not an error.
pub(crate) fn credits_from(json: &str) -> Credits {
    let Ok(value) = serde_json::from_str::<Value>(json) else { return Credits::default() };
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let number = |v: &Value, key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
    let rows = |key: &str| value.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    Credits {
        commits: number(&value, "commits"),
        contributors: rows("contributors")
            .iter()
            .filter_map(|c| {
                Some(Contributor {
                    login: text(c, "login")?,
                    display_name: text(c, "display_name"),
                    real_name: text(c, "real_name"),
                    prs: number(c, "prs"),
                    commits: number(c, "commits"),
                    lines_added: number(c, "lines_added"),
                    lines_deleted: number(c, "lines_deleted"),
                })
            })
            .collect(),
        models: rows("models").iter().filter_map(|m| Some(Model { name: text(m, "name")?, commits: number(m, "commits") })).collect(),
    }
}

/// The name a contributor is shown by: the display name they chose (or, when the viewer asks for them and they gave one, their real name),
/// and always their username.
pub(crate) fn shown_name(contributor: &Contributor, real_names: bool) -> String {
    let chosen = if real_names { contributor.real_name.as_ref().or(contributor.display_name.as_ref()) } else { contributor.display_name.as_ref() };
    match chosen {
        Some(name) => format!("{name} (@{})", contributor.login),
        None => format!("@{}", contributor.login),
    }
}

/// The state of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AboutForm {
    credits: Credits,
    real_names: bool,
}

impl XmlCraftApp {
    /// Show the About window.
    pub(crate) fn open_about(&mut self) {
        self.open_about_with_credits(CONTRIBUTORS_JSON);
    }

    /// Show the About window with the credits in `json` (what `cargo xtask contributors` writes) instead of the ones compiled in. For
    /// tests: the credits of a build are the history of the repository.
    pub fn open_about_with_credits(&mut self, json: &str) {
        self.dialog = Some(Dialog::About(Box::new(AboutForm { credits: credits_from(json), real_names: false })));
    }

    /// Draw the window; it closes with the button, Escape or a click outside.
    pub(crate) fn show_about(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let t = Tokens::get(ctx);
        let mut close = false;
        let response = egui::Modal::new(egui::Id::unique("xmlcraft-about")).show(ctx, |ui| {
            ui.set_width(520.0);
            if let Dialog::About(form) = &mut dialog {
                close = form_ui(ui, &t, form);
            }
        });
        if !close && !response.should_close() {
            self.dialog = Some(dialog);
        }
    }
}

fn form_ui(ui: &mut egui::Ui, t: &Tokens, form: &mut AboutForm) -> bool {
    let mut close = false;
    ui.vertical_centered(|ui| {
        ui.add(egui::Image::new(egui::include_image!("../../../assets/app-icon/xmlcraft.svg")).fit_to_exact_size(vec2(64.0, 64.0)));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(crate::APP_NAME).font(theme::semibold(22.0)).color(t.text));
        ui.label(egui::RichText::new(fmt(tl!("Version {version}"), &[("version", crate::VERSION)])).font(theme::regular(13.0)).color(t.text_muted));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!("An open-source, clean-room XML editor")).font(theme::regular(13.0)).color(t.text));
        ui.label(egui::RichText::new(tl!("MIT or Apache-2.0, at your option")).font(theme::regular(12.0)).color(t.text_muted));
        ui.label(egui::RichText::new("© 2026 Thorsten Hirsch and the XmlCraft contributors").font(theme::regular(12.0)).color(t.text_muted));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(tl!(
                "Independent of Altova GmbH. XMLSpy is a trademark of Altova GmbH, named only to say what kind of editor this is."
            ))
            .font(theme::regular(11.5))
            .color(t.text_faint),
        );
    });
    ui.add_space(10.0);
    // A fixed height: the window does not change size while its text is laid out, so what is under the pointer stays there.
    egui::ScrollArea::vertical().min_scrolled_height(300.0).max_height(300.0).auto_shrink([false, false]).show(ui, |ui| {
        widgets::section_title(ui, tl!("Contributors"));
        if form.credits.contributors.is_empty() {
            ui.label(egui::RichText::new(tl!("No contributors are listed in this build.")).font(theme::regular(12.5)).color(t.text_muted));
        }
        if form.credits.contributors.iter().any(|c| c.real_name.is_some()) {
            ui.add(widgets::checkbox(&mut form.real_names, tl!("Show the real names people gave")));
        }
        for c in &form.credits.contributors {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(shown_name(c, form.real_names)).font(theme::medium(12.5)).color(t.text));
                let detail = fmt(
                    tl!("{prs} pull requests, {commits} commits, +{added} −{deleted} lines"),
                    &[
                        ("prs", &c.prs.to_string()),
                        ("commits", &c.commits.to_string()),
                        ("added", &c.lines_added.to_string()),
                        ("deleted", &c.lines_deleted.to_string()),
                    ],
                );
                ui.label(egui::RichText::new(detail).font(theme::regular(11.5)).color(t.text_faint));
            });
        }
        if !form.credits.models.is_empty() {
            ui.add_space(8.0);
            widgets::section_title(ui, tl!("AI models"));
            ui.label(egui::RichText::new(tl!("Named as co-authors of commits.")).font(theme::regular(11.5)).color(t.text_muted));
            for m in &form.credits.models {
                ui.label(
                    egui::RichText::new(fmt(tl!("{name}: {commits} commits"), &[("name", &m.name), ("commits", &m.commits.to_string())]))
                        .font(theme::regular(12.5))
                        .color(t.text),
                );
            }
        }
        ui.add_space(8.0);
        widgets::section_title(ui, tl!("Third-party material"));
        ui.label(egui::RichText::new(NOTICE).font(theme::mono(10.5)).color(t.text_muted));
    });
    ui.add_space(10.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button(tl!("OK")).clicked() {
            close = true;
        }
    });
    close
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contributor(login: &str, display: Option<&str>, real: Option<&str>) -> Contributor {
        Contributor {
            login: login.to_owned(),
            display_name: display.map(str::to_owned),
            real_name: real.map(str::to_owned),
            prs: 1,
            commits: 2,
            lines_added: 3,
            lines_deleted: 4,
        }
    }

    #[test]
    fn the_credits_are_read_from_what_the_generator_writes() {
        let json = r#"{"schema":1,"commits":135,"contributors":[
            {"login":"alice","display_name":"Al","real_name":" Alice A ","prs":3,"commits":9,"lines_added":100,"lines_deleted":7},
            {"login":"bob","display_name":null,"real_name":"","prs":0,"commits":1},
            {"display_name":"nobody has a login"}],
          "models":[{"name":"Model A","commits":5},{"commits":1}]}"#;
        let credits = credits_from(json);
        assert_eq!(credits.commits, 135);
        assert_eq!(credits.contributors.len(), 2, "a row without a username is nobody");
        assert_eq!(
            credits.contributors[0],
            Contributor {
                login: "alice".into(),
                display_name: Some("Al".into()),
                real_name: Some("Alice A".into()),
                prs: 3,
                commits: 9,
                lines_added: 100,
                lines_deleted: 7
            }
        );
        assert_eq!((credits.contributors[1].display_name.clone(), credits.contributors[1].real_name.clone()), (None, None), "empty is none");
        assert_eq!(credits.models, [Model { name: "Model A".into(), commits: 5 }]);
    }

    #[test]
    fn what_is_not_the_credits_gives_none() {
        for json in ["", "not json", "[]", "null", "{\"contributors\": 5}", "{\"contributors\": [5, null, \"x\"]}"] {
            assert_eq!(credits_from(json), Credits::default(), "{json:?}");
        }
    }

    #[test]
    fn a_person_is_shown_by_the_name_they_chose_and_always_by_their_username() {
        let c = contributor("alice", Some("Al"), Some("Alice A"));
        assert_eq!(shown_name(&c, false), "Al (@alice)");
        assert_eq!(shown_name(&c, true), "Alice A (@alice)", "real names only when the viewer asks");
        // A real name nobody gave is not made up: the display name stands in, then the username alone.
        assert_eq!(shown_name(&contributor("bob", Some("Bobby"), None), true), "Bobby (@bob)");
        assert_eq!(shown_name(&contributor("carol", None, Some("Carol C")), false), "@carol", "a real name is not shown unless asked for");
        assert_eq!(shown_name(&contributor("dave", None, None), true), "@dave");
    }

    #[test]
    fn the_credits_that_ship_are_well_formed() {
        let credits = credits_from(CONTRIBUTORS_JSON);
        assert!(credits.commits > 0 && !credits.contributors.is_empty(), "run `cargo xtask contributors --write`");
        assert!(NOTICE.contains("Third-party material"));
    }
}
