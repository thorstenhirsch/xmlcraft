//! Design tokens and egui style: neutral chrome, white panels, one blue accent (the same look as PdfCraft).
//! Every custom widget reads `Tokens::get`.

use std::sync::Arc;

use egui::epaint::text::{FontTweak, HintingTarget, SmoothHinting};
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Response, Stroke, Vec2, Visuals};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeKind {
    #[default]
    Light,
    Dark,
}

/// The saved user choice, independent of the light/dark colours currently displayed. A fresh install follows the system; a
/// desktop that says nothing gets Light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    /// The colours to draw. `system` is what the operating system says, `None` when it says nothing; `fallback` is supplied by
    /// the caller.
    pub fn resolve(self, system: Option<bool>, fallback: ThemeKind) -> ThemeKind {
        match self {
            Self::Light => ThemeKind::Light,
            Self::Dark => ThemeKind::Dark,
            Self::System => match system {
                Some(true) => ThemeKind::Dark,
                Some(false) => ThemeKind::Light,
                None => fallback,
            },
        }
    }

    /// The next choice when the theme button is clicked: System, Light, Dark, and round again.
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub kind: ThemeKind,
    /// Title/tab strip.
    pub titlebar: Color32,
    /// Mode bar, panels.
    pub chrome: Color32,
    pub panel: Color32,
    /// The area behind the document.
    pub pasteboard: Color32,
    pub card: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub selected: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub accent_soft: Color32,
    pub field: Color32,
    pub badge_new: Color32,
    pub page_shadow: Color32,
    pub radius: u8,
}

/// The id under which the current tokens live in egui's temporary data.
const TOKENS_ID: &str = "xmlcraft-theme";

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Light => Self {
                kind,
                titlebar: Color32::from_rgb(0xE9, 0xE9, 0xEB),
                chrome: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                pasteboard: Color32::from_rgb(0xF1, 0xF1, 0xF3),
                card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                border: Color32::from_rgb(0xDA, 0xDA, 0xDE),
                divider: Color32::from_rgb(0xE8, 0xE8, 0xEB),
                text: Color32::from_rgb(0x22, 0x22, 0x26),
                text_muted: Color32::from_rgb(0x5E, 0x5E, 0x66),
                text_faint: Color32::from_rgb(0x6B, 0x6B, 0x73),
                icon: Color32::from_rgb(0x44, 0x44, 0x4B),
                hover: Color32::from_rgb(0xF0, 0xF0, 0xF3),
                pressed: Color32::from_rgb(0xE4, 0xE4, 0xE9),
                selected: Color32::from_rgb(0xE6, 0xEE, 0xFD),
                accent: Color32::from_rgb(0x1B, 0x63, 0xE0),
                accent_text: Color32::from_rgb(0x17, 0x55, 0xC4),
                accent_soft: Color32::from_rgb(0xE3, 0xEC, 0xFD),
                field: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                badge_new: Color32::from_rgb(0x1B, 0x63, 0xE0),
                page_shadow: Color32::from_black_alpha(34),
                radius: 6,
            },
            ThemeKind::Dark => Self {
                kind,
                titlebar: Color32::from_rgb(0x1B, 0x1B, 0x1E),
                chrome: Color32::from_rgb(0x26, 0x26, 0x2A),
                panel: Color32::from_rgb(0x26, 0x26, 0x2A),
                pasteboard: Color32::from_rgb(0x19, 0x19, 0x1C),
                card: Color32::from_rgb(0x2E, 0x2E, 0x33),
                border: Color32::from_rgb(0x3C, 0x3C, 0x43),
                divider: Color32::from_rgb(0x33, 0x33, 0x39),
                text: Color32::from_rgb(0xEC, 0xEC, 0xEF),
                text_muted: Color32::from_rgb(0xAE, 0xAE, 0xB6),
                text_faint: Color32::from_rgb(0x97, 0x97, 0x9E),
                icon: Color32::from_rgb(0xD4, 0xD4, 0xDA),
                hover: Color32::from_rgb(0x34, 0x34, 0x3A),
                pressed: Color32::from_rgb(0x3E, 0x3E, 0x45),
                selected: Color32::from_rgb(0x23, 0x3A, 0x63),
                accent: Color32::from_rgb(0x4B, 0x8B, 0xF5),
                accent_text: Color32::from_rgb(0x8C, 0xB6, 0xFA),
                accent_soft: Color32::from_rgb(0x24, 0x36, 0x57),
                field: Color32::from_rgb(0x1E, 0x1E, 0x22),
                badge_new: Color32::from_rgb(0x3D, 0x7D, 0xEE),
                page_shadow: Color32::from_black_alpha(120),
                radius: 6,
            },
        }
    }

    /// The tokens [`apply`] last installed; Light before any were.
    pub fn get(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::new(TOKENS_ID))).unwrap_or_else(|| Self::for_kind(ThemeKind::Light))
    }

    pub fn dark(&self) -> bool {
        self.kind == ThemeKind::Dark
    }
}

/// Hover and press amounts for chrome controls. The wash eases in; holding the pointer darkens it and drops the content one
/// pixel, so a click reads as a key settling.
#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub hover: f32,
    pub down: f32,
}

impl Press {
    pub fn track(ui: &egui::Ui, resp: &Response) -> Self {
        let hot = resp.hovered() || resp.is_pointer_button_down_on();
        let ease = egui::emath::easing::quadratic_out;
        // With animation effects off the wash and the press are simply there.
        let (hover, down) = if crate::motion::animated(ui.ctx()) { (0.12, 0.06) } else { (0.0, 0.0) };
        Self {
            hover: ui.ctx().animate_bool_with_time_and_easing(resp.id.with("chrome-hover"), hot, hover, ease),
            down: ui.ctx().animate_bool_with_time_and_easing(resp.id.with("chrome-down"), resp.is_pointer_button_down_on(), down, ease),
        }
    }

    pub fn fill(self, rest: Color32, hover: Color32, pressed: Color32) -> Color32 {
        rest.lerp_to_gamma(hover, self.hover).lerp_to_gamma(pressed, self.down)
    }

    /// One pixel down at a full press.
    pub fn offset(self) -> Vec2 {
        Vec2::new(0.0, self.down)
    }

    /// The colour [`Self::wash`] paints.
    pub fn wash_color(self, t: &Tokens, selected: bool) -> Color32 {
        if selected {
            self.fill(t.accent_soft, t.accent_soft.lerp_to_gamma(t.accent, 0.16), t.selected)
        } else {
            self.fill(Color32::TRANSPARENT, t.hover, t.pressed)
        }
    }

    /// Soft wash behind a control. `selected` keeps the accent fill.
    pub fn wash(self, ui: &egui::Ui, rect: Rect, radius: impl Into<CornerRadius>, selected: bool) {
        let fill = self.wash_color(&Tokens::get(ui.ctx()), selected);
        if fill.a() > 0 {
            ui.painter().rect_filled(rect, radius, fill);
        }
    }
}

/// Pointing hand, the cursor for a control that clicks.
pub fn hand(resp: Response) -> Response {
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The names of the embedded faces.
const INTER: &str = "Inter";
const INTER_MEDIUM: &str = "Inter-Medium";
const INTER_SEMIBOLD: &str = "Inter-SemiBold";
const JETBRAINS_MONO: &str = "JetBrainsMono";

pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontPolicyPlatform {
    Windows,
    Other,
}

fn current_font_policy_platform() -> FontPolicyPlatform {
    if cfg!(target_os = "windows") { FontPolicyPlatform::Windows } else { FontPolicyPlatform::Other }
}

/// Windows' low-DPI rendering of the monospace face needs asymmetric smooth hinting to keep vertical stems crisp, which
/// matters for a code editor; every other face and platform keeps egui's defaults.
fn primary_font_tweak(platform: FontPolicyPlatform, name: &str) -> FontTweak {
    let mut tweak = FontTweak::default();
    if platform == FontPolicyPlatform::Windows && name == JETBRAINS_MONO {
        tweak.hinting_target = HintingTarget::Smooth(SmoothHinting { symmetric_rendering: false, ..SmoothHinting::default() });
    }
    tweak
}

/// The interface fonts: Inter for text (regular, medium and semibold), JetBrains Mono for code, then egui's defaults as the
/// fallback in every family. Characters outside those faces (CJK, Arabic, Hebrew, Indic scripts) have no face yet: see
/// docs/gaps.md.
pub fn font_definitions() -> FontDefinitions {
    font_definitions_for_platform(current_font_policy_platform())
}

fn font_definitions_for_platform(platform: FontPolicyPlatform) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let faces: [(&str, &'static [u8]); 4] = [
        (INTER, include_bytes!("../../../assets/fonts/Inter-Regular.ttf")),
        (INTER_MEDIUM, include_bytes!("../../../assets/fonts/Inter-Medium.ttf")),
        (INTER_SEMIBOLD, include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf")),
        (JETBRAINS_MONO, include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf")),
    ];
    for (name, bytes) in faces {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes).tweak(primary_font_tweak(platform, name))));
    }
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, INTER.to_owned());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, JETBRAINS_MONO.to_owned());
    // Keep missing Latin/Cyrillic letters in the app's own faces (Bulgarian ѝ, absent from JetBrains Mono) before egui's defaults.
    fonts.families.entry(FontFamily::Monospace).or_default().insert(1, INTER.to_owned());
    let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (family, primary) in [("medium", INTER_MEDIUM), ("semibold", INTER_SEMIBOLD)] {
        let mut stack = vec![primary.to_owned()];
        stack.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(family.into()), stack);
    }
    fonts
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn apply(ctx: &egui::Context, kind: ThemeKind) {
    // Keep the native window inherited. A concrete egui theme pins macOS's NSWindow appearance and prevents winit from
    // delivering ThemeChanged. Our saved choice owns the palette; both egui branches receive it below.
    ctx.set_theme(egui::ThemePreference::System);
    let t = Tokens::for_kind(kind);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(TOKENS_ID), t));
    let mut v = if t.dark() { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.hover;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.override_text_color = Some(t.text);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if t.dark() { 110 } else { 38 }) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }) };
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(t.radius);
        w.fg_stroke.color = t.text;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = t.field;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.bg_fill = t.pressed;
    // Stock buttons (menus, dialogs) use the same pointing hand as the custom chrome.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.widgets.open.weak_bg_fill = t.hover;
    // Crisper text at 100–150 % scaling: glyphs sit on whole pixels instead of being rendered at quarter-pixel offsets, which
    // egui notes blurs them. In the light theme, a mild gamma darkens the antialiased edges of dark text (egui's default is
    // linear, which reads thin and grey next to the system's text); the dark theme keeps egui's own curve.
    v.text_options.subpixel_binning = false;
    if !t.dark() {
        v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Gamma(0.75);
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
        s.spacing.scroll.floating_width = 3.0;
        s.spacing.scroll.foreground_color = true;
        s.text_styles.insert(egui::TextStyle::Body, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Button, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Small, regular(11.0));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(17.0));
        s.text_styles.insert(egui::TextStyle::Monospace, mono(13.0));
        s.interaction.tooltip_delay = 0.35;
    });
    let style = ctx.global_style();
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// WCAG 2 contrast ratio between two opaque colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let lin = |v: u8| {
            let s = f32::from(v) / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (x, y) = (lum(a) + 0.05, lum(b) + 0.05);
    x.max(y) / x.min(y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accent_is_the_one_pdfcraft_uses() {
        assert_eq!(Tokens::for_kind(ThemeKind::Light).accent, Color32::from_rgb(0x1B, 0x63, 0xE0));
        assert_eq!(Tokens::for_kind(ThemeKind::Dark).accent, Color32::from_rgb(0x4B, 0x8B, 0xF5));
    }

    #[test]
    fn applied_tokens_are_what_widgets_read_back() {
        let ctx = egui::Context::default();
        assert_eq!(Tokens::get(&ctx).kind, ThemeKind::Light, "Light before anything is applied");
        apply(&ctx, ThemeKind::Dark);
        assert_eq!(Tokens::get(&ctx).kind, ThemeKind::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, Tokens::for_kind(ThemeKind::Dark).panel);
        apply(&ctx, ThemeKind::Light);
        assert_eq!(Tokens::get(&ctx).kind, ThemeKind::Light);
    }

    /// "Use system setting" draws what the operating system says, dark or light.
    #[test]
    fn following_the_system_draws_what_the_system_says() {
        assert_eq!(ThemePreference::System.resolve(Some(true), ThemeKind::Light), ThemeKind::Dark);
        assert_eq!(ThemePreference::System.resolve(Some(false), ThemeKind::Dark), ThemeKind::Light);
    }

    /// An operating system that says nothing leaves the fallback standing, and an explicit Light or Dark is never overridden by
    /// what the system says.
    #[test]
    fn an_answerless_system_leaves_the_choice_and_an_explicit_one_stands() {
        assert_eq!(ThemePreference::System.resolve(None, ThemeKind::Light), ThemeKind::Light);
        assert_eq!(ThemePreference::System.resolve(None, ThemeKind::Dark), ThemeKind::Dark);
        assert_eq!(ThemePreference::Light.resolve(Some(true), ThemeKind::Dark), ThemeKind::Light);
        assert_eq!(ThemePreference::Dark.resolve(Some(false), ThemeKind::Light), ThemeKind::Dark);
    }

    #[test]
    fn the_theme_button_cycles_through_every_choice() {
        let start = ThemePreference::System;
        assert_eq!(start.next(), ThemePreference::Light);
        assert_eq!(start.next().next(), ThemePreference::Dark);
        assert_eq!(start.next().next().next(), start);
    }

    #[test]
    fn press_darkens_then_sinks() {
        let rest = Color32::from_rgb(0xFF, 0xFF, 0xFF);
        let hover = Color32::from_rgb(0xF0, 0xF0, 0xF3);
        let pressed = Color32::from_rgb(0xE4, 0xE4, 0xE9);
        let idle = Press { hover: 0.0, down: 0.0 };
        assert_eq!(idle.fill(rest, hover, pressed), rest);
        assert_eq!(idle.offset(), Vec2::ZERO);
        let held = Press { hover: 1.0, down: 1.0 };
        assert_eq!(held.fill(rest, hover, pressed), pressed);
        assert_eq!(held.offset(), Vec2::new(0.0, 1.0));
    }

    #[test]
    fn text_is_readable_on_every_surface() {
        // The faintest text (hints, captions) must still reach WCAG AA.
        for kind in [ThemeKind::Light, ThemeKind::Dark] {
            let t = Tokens::for_kind(kind);
            for (name, fg) in [("text", t.text), ("text_muted", t.text_muted), ("text_faint", t.text_faint)] {
                for bg in [t.chrome, t.panel, t.card, t.pasteboard] {
                    let r = contrast(fg, bg);
                    assert!(r >= 4.5, "{kind:?} {name} on {bg:?}: {r:.2}:1, WCAG AA needs 4.5:1");
                }
            }
        }
    }

    #[test]
    fn contrast_of_black_on_white_is_21() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
    }

    #[test]
    fn every_family_the_style_uses_has_an_embedded_primary_face() {
        let fonts = font_definitions();
        let first = |family: FontFamily| fonts.families.get(&family).and_then(|s| s.first().cloned());
        assert_eq!(first(FontFamily::Proportional).as_deref(), Some(INTER));
        assert_eq!(first(FontFamily::Monospace).as_deref(), Some(JETBRAINS_MONO));
        assert_eq!(first(FontFamily::Name("medium".into())).as_deref(), Some(INTER_MEDIUM));
        assert_eq!(first(FontFamily::Name("semibold".into())).as_deref(), Some(INTER_SEMIBOLD));
        // egui's own faces stay behind ours as the fallback.
        assert!(fonts.families[&FontFamily::Proportional].len() > 1);
    }

    #[test]
    fn windows_hinting_changes_only_the_jetbrains_mono_face() {
        let other = font_definitions_for_platform(FontPolicyPlatform::Other);
        let windows = font_definitions_for_platform(FontPolicyPlatform::Windows);
        assert_eq!(other.families, windows.families);
        for (name, data) in &other.font_data {
            if name == JETBRAINS_MONO {
                assert_ne!(windows.font_data[name].tweak, data.tweak, "Windows hints the monospace face differently");
            } else {
                assert_eq!(windows.font_data[name].tweak, data.tweak, "{name}");
            }
        }
    }
}
