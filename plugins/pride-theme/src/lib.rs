//! Pride Theme: Webox in the colours of a Pride flag. The accents (links, focus, selection, unread dots, badges, the selected
//! rail item, primary buttons, controls, mentions, the composer line as a gradient) take the flag's colours, spread across the
//! roles; a soft wash of the flag lies behind the rail and header and a gentle one with a gradient rim on the cards; Webox
//! draws the decorations and effects left switched on (decor.rs). Backgrounds stay the base theme's underneath and body text is
//! never recoloured, so it works on every theme, dark and light: every colour is moved in lightness just enough for WCAG AA on
//! the active theme (4.5:1 for text and text on a fill, 3:1 for dots, rings and control fills), and every wash is only as
//! strong as the text on it allows.
pub mod decor;

use collabhost_plugin_api::color::{contrast, fill_for_text, is_grey, mix, readable, rgb, text_on, AA_GRAPHIC, AA_TEXT};
use collabhost_plugin_api::{export_plugin, Accents, Decor, Plugin, Schema, ThemeInfo, Values, GRADIENT_MAX};

/// (setting value, label, stripes top to bottom as 0xRRGGBB): the flags' standard colours.
pub const FLAGS: [(&str, &str, &[u32]); 8] = [
    ("rainbow", "Rainbow", &[0xE40303, 0xFF8C00, 0xFFED00, 0x008026, 0x004DFF, 0x750787]),
    // Progress Pride: the rainbow plus the chevron (white, pink, light blue, brown, black)
    ("progress", "Progress Pride", &[0xE40303, 0xFF8C00, 0xFFED00, 0x008026, 0x004DFF, 0x750787, 0xFFFFFF, 0xF5A9B8, 0x5BCEFA, 0x613915, 0x000000]),
    ("trans", "Trans", &[0x5BCEFA, 0xF5A9B8, 0xFFFFFF, 0xF5A9B8, 0x5BCEFA]),
    ("bi", "Bi", &[0xD60270, 0xD60270, 0x9B4F96, 0x0038A8, 0x0038A8]),
    ("lesbian", "Lesbian", &[0xD52D00, 0xEF7627, 0xFF9A56, 0xFFFFFF, 0xD162A4, 0xB55690, 0xA30262]),
    ("pan", "Pan", &[0xFF218C, 0xFFD800, 0x21B1FF]),
    ("nonbinary", "Non-binary", &[0xFCF434, 0xFFFFFF, 0x9C59D1, 0x2C2C2C]),
    ("ace", "Asexual", &[0x000000, 0xA3A3A3, 0xFFFFFF, 0x800080]),
];

struct Pride { flag: usize, gradient: bool, wash_window: bool, wash_cards: bool, flags: u64 }
impl Default for Pride {
    fn default() -> Self {
        let flags = decor::TOGGLES.iter().filter(|t| t.4).fold(0, |a, t| a | t.3);
        Pride { flag: 0, gradient: true, wash_window: true, wash_cards: true, flags }
    }
}

/// The distinct coloured (not grey) stripes, in flag order.
fn hues(stripes: &[u32]) -> Vec<u32> {
    let mut v: Vec<u32> = vec![];
    for s in stripes.iter().map(|s| rgb(*s)).filter(|s| !is_grey(*s)) { if !v.contains(&s) { v.push(s) } }
    if v.is_empty() { v.push(rgb(stripes[0])) }
    v
}

/// The accents for one flag on one base theme.
pub fn accents_for(stripes: &[u32], th: &ThemeInfo, gradient: bool) -> Accents {
    let bg = th.background;
    let h = hues(stripes);
    // Six roles spread over the flag's colours: on the rainbow red, orange, yellow, green, blue, violet.
    let pick = |k: usize| h[(k * (h.len() - 1) + 2) / 5];
    let (unread, rail, mention, button, link, focus) = (pick(0), pick(1), pick(2), pick(3), pick(4), pick(5));
    let (badge, badge_text) = fill_for_text(readable(unread, bg, AA_GRAPHIC), AA_TEXT);
    let (button, button_text) = fill_for_text(button, AA_TEXT);
    let control = readable(focus, bg, AA_GRAPHIC);
    // the tick / dot on a checked control: black or white, the control moved until it reads (3:1 for a graphic)
    let (control, control_icon) = { let (f, t) = fill_for_text(control, AA_GRAPHIC); if contrast(f, bg) >= AA_GRAPHIC { (f, t) } else { (control, text_on(control)) } };
    // The selected rail circle carries the rail's icon in the primary text colour: tint it only as far as that stays readable.
    let rail_selected = (1..=12).rev().map(|i| mix(bg, rail, i as f64 * 0.05)).find(|c| contrast(*c, th.text) >= AA_TEXT).unwrap_or(bg);
    let mut a = Accents {
        accent: control, link: readable(link, bg, AA_TEXT), focus: readable(focus, bg, AA_GRAPHIC), selection: readable(link, bg, AA_GRAPHIC),
        unread: readable(unread, bg, AA_GRAPHIC), badge, badge_text, rail_selected, button, button_text, control, control_icon,
        mention: readable(mention, bg, AA_TEXT), scrollbar_hover: readable(rail, bg, AA_GRAPHIC), ..Default::default()
    };
    if gradient {
        let stops: Vec<u32> = stripes.iter().map(|s| readable(rgb(*s), bg, AA_GRAPHIC)).take(GRADIENT_MAX).collect();
        a.gradient_len = stops.len() as u32;
        a.gradient[..stops.len()].copy_from_slice(&stops);
    }
    a
}

/// A host that sent no colours (e.g. a capability probe): assume the default dark theme.
fn filled(th: &ThemeInfo) -> ThemeInfo {
    if th.background != 0 { return *th; }
    ThemeInfo { background: rgb(0x262626), surface: rgb(0x1A1A1A), text: rgb(0xF7F7F7), text_secondary: rgb(0xB2B2B2), ..*th }
}

impl Plugin for Pride {
    const NAME: &'static str = "Pride Theme";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");

    fn settings(&self) -> Option<Schema> {
        let opts: Vec<(&str, &str)> = FLAGS.iter().map(|(v, l, _)| (*v, *l)).collect();
        let mut s = Schema::new()
            .section("Flag", "All colours, backgrounds and decorations come from this flag. Text always stays easy to read.")
            .choice("flag", "Flag", "", "rainbow", &opts)
            .toggle("gradient", "Focus line as a gradient", "The line above the message field in all the colours of the flag.", true)
            .section("Backgrounds", "As strong as the text on them allows (WCAG AA).")
            .toggle("wash_window", "Wash behind the sidebar and header", "A soft diagonal gradient of the flag.", true)
            .toggle("wash_cards", "List and conversation", "A hint of the flag and a coloured rim; especially subtle behind messages.", true)
            .section("Decorations", "");
        for (id, label, desc, bit, def) in decor::TOGGLES { if !decor::is_effect(bit) { s = s.toggle(id, label, desc, def); } }
        s = s.section("Effects", "Short and never blocking input; off when Windows asks for fewer animations.");
        for (id, label, desc, bit, def) in decor::TOGGLES { if decor::is_effect(bit) { s = s.toggle(id, label, desc, def); } }
        Some(s)
    }
    fn settings_changed(&mut self, v: &Values) {
        self.flag = v.text("flag").and_then(|f| FLAGS.iter().position(|x| x.0 == f)).unwrap_or(0);
        self.gradient = v.bool("gradient").unwrap_or(true);
        self.wash_window = v.bool("wash_window").unwrap_or(true);
        self.wash_cards = v.bool("wash_cards").unwrap_or(true);
        self.flags = decor::TOGGLES.iter().filter(|t| v.bool(t.0).unwrap_or(t.4)).fold(0, |a, t| a | t.3);
    }
    fn accents(&mut self, th: &ThemeInfo) -> Option<Accents> { Some(accents_for(FLAGS[self.flag].2, &filled(th), self.gradient)) }
    fn decor(&mut self, th: &ThemeInfo) -> Option<Decor> {
        Some(decor::decor_for(FLAGS[self.flag].2, &filled(th), self.wash_window, self.wash_cards, self.flags))
    }
}

export_plugin!(Pride);

#[cfg(test)]
mod tests;
