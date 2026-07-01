//! Tabler Icons (https://tabler.io/icons, MIT licensed). The webfont is bundled
//! and registered as a *fallback* font, so these Private-Use-Area codepoints
//! render anywhere normal text does (the primary fonts lack PUA glyphs, so egui
//! falls through to Tabler for them). Codepoints come from the Tabler webfont CSS.

use eframe::egui;

const TABLER_TTF: &[u8] = include_bytes!("../assets/tabler-icons.ttf");

/// Register the Tabler font as a fallback in the proportional + monospace
/// families. Call once during app creation.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("tabler".to_owned(), egui::FontData::from_static(TABLER_TTF));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("tabler".to_owned());
    }
    ctx.set_fonts(fonts);
}

pub const EYE: &str = "\u{ea9a}";
pub const EYE_OFF: &str = "\u{ecf0}";
pub const TRASH: &str = "\u{eb41}";
pub const PLUS: &str = "\u{eb0b}";
pub const CHEVRON_DOWN: &str = "\u{ea5f}";
pub const CHEVRON_RIGHT: &str = "\u{ea61}";
