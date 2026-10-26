//! Vector icons: embedded Lucide SVGs, recoloured to white, rasterized by egui_extras at the exact on-screen pixel size, and
//! tinted per use (never use Unicode glyphs as icons).

use std::sync::OnceLock;

use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use craft_ui::icons::{SvgIconSet, SvgStyle};

use egui::{Color32, Rect, Response, Stroke, Vec2};

use crate::icon_data::ICONS;
use crate::theme::{self, Tokens};

/// The icon shown for a name that is not in the catalog.
const FALLBACK: &str = "square";

fn white_icons() -> &'static SvgIconSet {
    static MAP: OnceLock<SvgIconSet> = OnceLock::new();
    MAP.get_or_init(|| SvgIconSet::new(ICONS, SvgStyle { current_color: "#ffffff", stroke_width: Some("1.75") }, "bytes://icons/", FALLBACK))
}

#[cfg(test)]
pub fn exists(name: &str) -> bool {
    white_icons().contains(name)
}

pub(crate) fn source(name: &str) -> egui::ImageSource<'static> {
    // An unknown name is cached under the resolved fallback name.
    white_icons().source(white_icons().canonical_name(name).unwrap_or(FALLBACK))
}

pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    egui::Image::new(source(name)).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

pub fn paint(ui: &egui::Ui, rect: Rect, name: &str, size: f32, tint: Color32) {
    image(name, size, tint).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(size)));
}

/// Square icon button: transparent until hovered; `selected` gets the accent treatment.
pub fn button(ui: &mut egui::Ui, name: &str, box_size: f32, selected: bool, tooltip: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    // The shared button paints no fill of its own: the eased press wash below replaces it.
    let clear = |icon| ButtonVisuals { fill: Color32::TRANSPARENT, stroke: Stroke::NONE, icon };
    let style = IconButtonStyle {
        normal: clear(t.icon),
        hovered: clear(t.icon),
        pressed: Some(clear(t.icon)),
        selected: clear(t.accent_text),
        corner_radius: t.radius.into(),
        focus_stroke: Stroke::new(1.0, t.accent),
    };
    let label = if tooltip.is_empty() { name } else { tooltip };
    // The wash eases from the response, so reserve its place under the icon and focus ring.
    let wash = ui.painter().add(egui::Shape::Noop);
    let mut painted = None;
    let resp = IconButton::new(label, Vec2::splat(box_size), &style).selected(selected).show(ui, |_, rect, tint| painted = Some((rect, tint)));
    if let Some((rect, tint)) = painted {
        let press = theme::Press::track(ui, &resp);
        let mut shapes = Vec::with_capacity(2);
        let fill = press.wash_color(&t, selected);
        if fill.a() > 0 {
            shapes.push(egui::Shape::rect_filled(rect, t.radius, fill));
        }
        if selected {
            shapes.push(egui::Shape::rect_stroke(
                rect.shrink(0.5),
                t.radius,
                Stroke::new(1.0, t.accent.gamma_multiply(0.45)),
                egui::StrokeKind::Inside,
            ));
        }
        ui.painter().set(wash, egui::Shape::Vec(shapes));
        // The glyph shrinks a little and drops a pixel while the pointer is down.
        let size = (box_size * 0.5).round() * (1.0 - 0.08 * press.down);
        paint(ui, rect.translate(press.offset()), name, size, tint);
    }
    let resp = theme::hand(resp);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_file_is_embedded() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .expect("assets/icons exists")
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".svg")).map(str::to_owned))
            .collect();
        files.sort();
        let mut embedded: Vec<&str> = ICONS.iter().map(|(n, _)| *n).collect();
        embedded.sort_unstable();
        assert_eq!(files, embedded, "regenerate crates/ui-egui/src/icon_data.rs from assets/icons");
    }

    #[test]
    fn the_fallback_icon_exists_and_unknown_names_use_it() {
        assert!(exists(FALLBACK));
        assert!(!exists("no-such-icon"));
        let egui::ImageSource::Bytes { uri, bytes } = source("no-such-icon") else { panic!("expected SVG bytes") };
        let egui::ImageSource::Bytes { bytes: fallback, .. } = source(FALLBACK) else { panic!("expected SVG bytes") };
        assert_eq!(uri.as_ref(), format!("bytes://icons/{FALLBACK}.svg"));
        assert_eq!(bytes.as_ref(), fallback.as_ref());
    }

    #[test]
    fn icons_are_recoloured_for_tinting_and_thinned() {
        let egui::ImageSource::Bytes { bytes, .. } = source("code-xml") else { panic!("expected SVG bytes") };
        let svg = String::from_utf8_lossy(bytes.as_ref());
        assert!(svg.contains("#ffffff") && !svg.contains("currentColor"), "{svg}");
        assert!(svg.contains("stroke-width=\"1.75\""), "{svg}");
    }

    #[test]
    fn the_icons_the_shell_uses_exist() {
        for name in [
            "house",
            "plus",
            "x",
            "search",
            "sun",
            "moon",
            "monitor",
            "circle-help",
            "panel-left",
            "list-tree",
            "braces",
            "panel-bottom",
            "panel-right",
            "file-code",
            "chevron-right",
            "chevron-down",
            "triangle-alert",
            "circle-check",
            "circle-x",
            "info",
            "save",
            "folder-open",
            "wand-sparkles",
        ] {
            assert!(exists(name), "missing icon {name}");
        }
    }
}
