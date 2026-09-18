//! The two themes, and the font the window draws them in.
//!
//! A file list is a dense, high-frequency surface: the eye scans hundreds of
//! rows looking for one. So the styling stays quiet — one accent colour, a
//! single strong weight for names, everything else recessive.
//!
//! The colours are the same tokens the web-view front end uses, so the two
//! windows are genuinely comparable: what differs between them is how the
//! pixels get there, not what they look like.

use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    pub fn flipped(self) -> Self {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

/// The colours a theme is made of.
pub struct Palette {
    pub bg: egui::Color32,
    pub panel: egui::Color32,
    pub panel_2: egui::Color32,
    pub border: egui::Color32,
    pub text: egui::Color32,
    pub muted: egui::Color32,
    pub accent: egui::Color32,
    pub accent_soft: egui::Color32,
    pub danger: egui::Color32,
    /// Every other row, barely there.
    pub stripe: egui::Color32,
}

const DARK: Palette = Palette {
    bg: rgb(0x0f, 0x11, 0x17),
    panel: rgb(0x16, 0x19, 0x22),
    panel_2: rgb(0x1c, 0x20, 0x2b),
    border: rgb(0x27, 0x2c, 0x3a),
    text: rgb(0xe5, 0xe7, 0xeb),
    muted: rgb(0x8b, 0x93, 0xa7),
    accent: rgb(0x81, 0x8c, 0xf8),
    accent_soft: egui::Color32::from_rgba_premultiplied(0x21, 0x24, 0x40, 0xff),
    danger: rgb(0xf8, 0x71, 0x71),
    stripe: rgb(0x13, 0x16, 0x1d),
};

const LIGHT: Palette = Palette {
    bg: rgb(0xf7, 0xf8, 0xfa),
    panel: rgb(0xff, 0xff, 0xff),
    panel_2: rgb(0xf1, 0xf3, 0xf7),
    border: rgb(0xe1, 0xe4, 0xea),
    text: rgb(0x18, 0x1c, 0x26),
    muted: rgb(0x64, 0x6c, 0x7c),
    accent: rgb(0x4f, 0x46, 0xe5),
    accent_soft: rgb(0xe4, 0xe2, 0xfb),
    danger: rgb(0xdc, 0x26, 0x26),
    stripe: rgb(0xf1, 0xf3, 0xf7),
};

const fn rgb(r: u8, g: u8, b: u8) -> egui::Color32 {
    egui::Color32::from_rgb(r, g, b)
}

pub fn palette(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// Row height, in points. Fixed, because the virtual scroller measures the
/// result set in whole rows and a million of them have to add up.
pub const ROW_HEIGHT: f32 = 26.0;

/// Install a theme on the context.
///
/// The preference is pinned rather than followed: Ferret has its own switch in
/// the toolbar, and a window that changed colour because Windows did would be
/// ignoring it.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let (preference, slot) = match theme {
        Theme::Dark => (egui::ThemePreference::Dark, egui::Theme::Dark),
        Theme::Light => (egui::ThemePreference::Light, egui::Theme::Light),
    };
    ctx.set_theme(preference);

    let p = palette(theme);
    let mut visuals = match theme {
        Theme::Dark => egui::Visuals::dark(),
        Theme::Light => egui::Visuals::light(),
    };

    visuals.panel_fill = p.panel;
    visuals.window_fill = p.panel;
    visuals.faint_bg_color = p.stripe;
    // Where text is typed, and the scroll area behind the results.
    visuals.extreme_bg_color = p.bg;
    visuals.window_stroke = egui::Stroke::new(1.0, p.border);
    visuals.hyperlink_color = p.accent;
    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = egui::Stroke::new(1.0, p.accent);
    visuals.error_fg_color = p.danger;
    visuals.warn_fg_color = p.danger;

    // Text and chrome. `noninteractive` is labels and separators; the other
    // three are a widget at rest, under the pointer, and being pressed.
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
    visuals.widgets.noninteractive.weak_bg_fill = p.panel;

    visuals.widgets.inactive.bg_fill = p.panel_2;
    visuals.widgets.inactive.weak_bg_fill = p.panel_2;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.border);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);

    visuals.widgets.hovered.bg_fill = p.accent_soft;
    visuals.widgets.hovered.weak_bg_fill = p.accent_soft;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.accent);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text);

    visuals.widgets.active.bg_fill = p.accent_soft;
    visuals.widgets.active.weak_bg_fill = p.accent_soft;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, p.accent);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.text);

    visuals.widgets.open.bg_fill = p.panel_2;
    visuals.widgets.open.weak_bg_fill = p.panel_2;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, p.border);

    let corner = egui::CornerRadius::same(6);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = corner;
    }

    ctx.set_visuals_of(slot, visuals);
}

/// Sizes and spacing. Applied once, at startup, to both themes — only the
/// colours differ between them.
pub fn apply_style(ctx: &egui::Context) {
    ctx.all_styles_mut(set_sizes);
}

fn set_sizes(style: &mut egui::Style) {
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(13.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(13.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(17.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(12.5, egui::FontFamily::Monospace),
    );

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.interact_size.y = 24.0;
    // A dense list should not have a fat scrollbar stealing width from paths.
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.floating = false;
}

/// Draw the window in Segoe UI rather than egui's bundled font.
///
/// Two reasons, and the second is the real one. It is the font every other
/// Windows list is set in, so the window stops looking like a game. And it
/// covers the accented Latin, Greek and Cyrillic that turn up in real file
/// names, where the bundled font would show empty boxes.
///
/// Failing to find it is not an error: the bundled font still renders the whole
/// interface, and only unusual file names suffer.
pub fn install_fonts(ctx: &egui::Context) {
    const REGULAR: &str = r"C:\Windows\Fonts\segoeui.ttf";
    const SEMIBOLD: &str = r"C:\Windows\Fonts\seguisb.ttf";

    let mut fonts = egui::FontDefinitions::default();
    let mut installed = Vec::new();

    for (name, path) in [("segoe", REGULAR), ("segoe-semibold", SEMIBOLD)] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                name.to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            installed.push(name.to_owned());
        }
    }

    if installed.is_empty() {
        return;
    }

    // In front of the bundled font, not instead of it: anything Segoe UI has no
    // glyph for still falls through to egui's own, and then to the emoji font.
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        for name in installed.iter().rev() {
            family.insert(0, name.clone());
        }
    }

    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_themes_are_actually_different() {
        assert_ne!(palette(Theme::Dark).bg, palette(Theme::Light).bg);
        assert_ne!(palette(Theme::Dark).text, palette(Theme::Light).text);
        assert_eq!(Theme::Dark.flipped(), Theme::Light);
        assert_eq!(Theme::Light.flipped().flipped(), Theme::Light);
    }

    /// Light text on a light background is the classic way a theme ships broken,
    /// so the contrast is asserted rather than eyeballed.
    #[test]
    fn text_stands_off_its_background_in_both_themes() {
        for theme in [Theme::Dark, Theme::Light] {
            let p = palette(theme);
            for (name, background) in [("bg", p.bg), ("panel", p.panel), ("panel_2", p.panel_2)] {
                let gap = luminance(p.text) - luminance(background);
                assert!(
                    gap.abs() > 0.45,
                    "{theme:?}: text is too close to {name} ({gap:.2})"
                );
            }
            // Muted text is meant to recede, but still has to be readable.
            let gap = luminance(p.muted) - luminance(p.bg);
            assert!(gap.abs() > 0.2, "{theme:?}: muted text is too faint");
        }
    }

    /// Rough perceptual brightness, 0 to 1. Enough to catch a theme where two
    /// colours were pasted from the wrong half of the table.
    fn luminance(colour: egui::Color32) -> f32 {
        let [r, g, b, _] = colour.to_array().map(|c| c as f32 / 255.0);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }
}
