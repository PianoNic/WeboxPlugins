//! What the Pride Theme asks Webox to draw (plugin-api 1.4 `Decor`): a soft diagonal wash of the flag behind the rail and
//! header, a gentle wash and a gradient rim on the list and conversation cards, and the decorations and effects the person
//! leaves switched on. Every wash is only as strong as the text on it allows: primary and secondary text keep WCAG AA (4.5:1)
//! over every stop, or at least the contrast they had without the wash where the theme itself is below that.
use collabhost_plugin_api::color::{contrast, mix, readable, rgb, AA_GRAPHIC, AA_TEXT};
use collabhost_plugin_api::{deco, Decor, ThemeInfo, Wash, GRADIENT_MAX};

/// (setting id, label, description, bit, default) of every decoration and effect toggle.
pub const TOGGLES: [(&str, &str, &str, u64, bool); 17] = [
    ("rail_stripe", "Flaggenband am linken Rand", "Ein senkrechtes Band in den Farben der Flagge entlang der Seitenleiste.", deco::RAIL_STRIPE, true),
    ("header_stripe", "Linie unter der Kopfzeile", "Ein dünner Verlauf unter der Kopfzeile.", deco::HEADER_STRIPE, true),
    ("avatar_self", "Ring um Ihr Profilbild", "", deco::AVATAR_RING_SELF, true),
    ("avatar_all", "Ringe um alle Profilbilder", "", deco::AVATAR_RING_ALL, false),
    ("presence_ring", "Ring um Statuspunkte", "", deco::PRESENCE_RING, false),
    ("selected_row", "Rand um die ausgewählte Zeile", "", deco::SELECTED_ROW, true),
    ("tab_pills", "Rand um den aktiven Tab", "„Alle“, „Nachrichten“ und die anderen Tabs.", deco::TAB_PILLS, true),
    ("nav_underline", "Unterstrich in der Einstellungsnavigation", "", deco::NAV_UNDERLINE, true),
    ("toggle_track", "Schalter im Flaggenverlauf", "", deco::TOGGLE_TRACK, true),
    ("badge_cycle", "Ungelesen-Punkte in den Flaggenfarben", "Von Zeile zu Zeile die nächste Farbe.", deco::BADGE_CYCLE, true),
    ("scrollbar", "Bildlaufleisten im Flaggenverlauf", "", deco::SCROLLBAR, true),
    ("progress", "Fortschrittsbalken im Flaggenverlauf", "", deco::PROGRESS, true),
    ("send_button", "Senden-Knopf im Flaggenverlauf", "", deco::SEND_BUTTON, true),
    ("name_flag", "Flagge neben Ihrem Namen", "Im Profilmenü.", deco::NAME_FLAG, true),
    ("send_burst", "Konfetti und Herzen beim Senden", "Eine kurze Animation am Senden-Knopf (unter einer Sekunde).", deco::SEND_BURST, true),
    ("reaction_ripple", "Regenbogenwelle bei Reaktionen", "Wenn Sie eine Reaktion hinzufügen.", deco::REACTION_RIPPLE, true),
    ("june_sparkle", "„Happy Pride!“ im Juni", "Einmal im Jahr, beim ersten Start im Juni.", deco::JUNE_SPARKLE, true),
];
/// Toggles that are effects (shown in their own section); the rest are decorations.
pub fn is_effect(bit: u64) -> bool { bit >= deco::SEND_BURST }

/// Strongest wash opacities: behind rail and header, on the list card, on the conversation card (message text sits there).
pub const WINDOW_MAX: f64 = 0.18;
pub const LIST_MAX: f64 = 0.10;
pub const CONTENT_MAX: f64 = 0.05;

/// The contrast a text colour must keep on `base` with a wash over it: AA, or what it had without the wash if that is less.
fn needed(text: u32, base: u32) -> f64 { AA_TEXT.min(contrast(text, base)) }

/// The strongest opacity up to `max` at which `texts` keep their contrast over every stop laid on `base`.
pub fn safe_opacity(stops: &[u32], base: u32, texts: &[u32], max: f64) -> f64 {
    let ok = |op: f64| stops.iter().all(|s| { let b = mix(base, *s, op); texts.iter().all(|t| contrast(*t, b) + 1e-9 >= needed(*t, base)) });
    let mut op = max;
    while op > 0.0 && !ok(op) { op -= 0.005; }
    op.max(0.0)
}

fn wash(stops: &[u32], angle: f32, opacity: f64, border: f32) -> Wash {
    let mut w = Wash { stops_len: stops.len().min(GRADIENT_MAX) as u32, angle, opacity: opacity as f32, border, ..Default::default() };
    w.stops[..w.stops_len as usize].copy_from_slice(&stops[..w.stops_len as usize]);
    w
}

/// Decorations for a flag's `stripes` on a base theme. `window` / `cards`: the washes, `flags`: the decoration bits.
pub fn decor_for(stripes: &[u32], th: &ThemeInfo, window: bool, cards: bool, flags: u64) -> Decor {
    let raw: Vec<u32> = stripes.iter().map(|s| rgb(*s)).take(GRADIENT_MAX).collect();
    let texts: Vec<u32> = [th.text, th.text_secondary].into_iter().filter(|c| *c != 0).collect();
    let mut d = Decor { flags, ..Default::default() };
    if window { d.window = wash(&raw, 35.0, safe_opacity(&raw, th.surface, &texts, WINDOW_MAX), 0.0); }
    if cards {
        d.list_card = wash(&raw, 90.0, safe_opacity(&raw, th.background, &texts, LIST_MAX), 1.5);
        d.content_card = wash(&raw, 0.0, safe_opacity(&raw, th.background, &texts, CONTENT_MAX), 1.5);
    }
    // the decorations' colours: the flag's, only moved where a stripe would vanish on this theme (3:1 for a graphic)
    let deco: Vec<u32> = raw.iter().map(|s| readable(*s, th.background, AA_GRAPHIC)).collect();
    d.stripes_len = deco.len() as u32;
    d.stripes[..deco.len()].copy_from_slice(&deco);
    d
}
