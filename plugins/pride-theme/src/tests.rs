use super::*;
use collabhost_plugin_api::{deco, Wash};

/// Representative base themes: Webox's default dark and light colours.
fn theme(light: bool) -> ThemeInfo {
    if light { ThemeInfo { light: 1, background: rgb(0xFAFAFA), surface: rgb(0xF0F0F0), text: rgb(0x121212), text_secondary: rgb(0x545454), ..Default::default() } }
    else { ThemeInfo { light: 0, background: rgb(0x262626), surface: rgb(0x1A1A1A), text: rgb(0xF7F7F7), text_secondary: rgb(0xB2B2B2), ..Default::default() } }
}

#[test]
fn every_flag_is_readable_on_dark_and_light() {
    for (name, _, stripes) in FLAGS {
        for light in [false, true] {
            let th = theme(light);
            let a = accents_for(stripes, &th, true);
            let bg = th.background;
            let ctx = format!("{name} light={light}");
            assert!(contrast(a.link, bg) >= AA_TEXT, "link {ctx}");
            assert!(contrast(a.mention, bg) >= AA_TEXT, "mention {ctx}");
            assert!(contrast(a.button, a.button_text) >= AA_TEXT, "button {ctx}");
            assert!(contrast(a.badge, a.badge_text) >= AA_TEXT, "badge {ctx}");
            for (role, c) in [("focus", a.focus), ("unread", a.unread), ("control", a.control), ("scrollbar", a.scrollbar_hover)] {
                assert!(contrast(c, bg) >= AA_GRAPHIC, "{role} {ctx}");
            }
            assert!(contrast(a.rail_selected, th.text) >= AA_TEXT, "rail icon {ctx}");
            assert!(a.gradient_len >= 3 && a.gradient_len as usize <= GRADIENT_MAX, "gradient {ctx}");
        }
    }
}

/// Text over a wash: every stop laid over the base at the wash's opacity.
fn text_ok(w: &Wash, base: u32, th: &ThemeInfo, ctx: &str) {
    for s in &w.stops[..w.stops_len as usize] {
        let b = mix(base, *s, w.opacity as f64);
        for t in [th.text, th.text_secondary] {
            assert!(contrast(t, b) >= AA_TEXT.min(contrast(t, base)) - 1e-6, "{ctx}: {:08X} on {:08X} at {}", t, s, w.opacity);
        }
    }
}

#[test]
fn text_stays_readable_over_every_wash() {
    for (name, _, stripes) in FLAGS {
        for light in [false, true] {
            let th = theme(light);
            let d = decor::decor_for(stripes, &th, true, true, 0);
            text_ok(&d.window, th.surface, &th, &format!("{name} light={light} window"));
            text_ok(&d.list_card, th.background, &th, &format!("{name} light={light} list"));
            text_ok(&d.content_card, th.background, &th, &format!("{name} light={light} content"));
            assert!(d.content_card.opacity as f64 <= decor::CONTENT_MAX + 1e-6 && d.window.opacity as f64 <= decor::WINDOW_MAX + 1e-6);
            assert!(d.window.stops_len as usize == stripes.len().min(GRADIENT_MAX), "{name}: the wash has every stripe");
            for c in &d.stripes[..d.stripes_len as usize] { assert!(contrast(*c, th.background) >= AA_GRAPHIC, "{name} light={light} stripe {:08X}", c); }
        }
    }
}

#[test]
fn rainbow_wash_is_actually_visible() {
    // the point of it: on the default themes the rainbow lies clearly behind rail and header
    for light in [false, true] { assert!(decor::decor_for(FLAGS[0].2, &theme(light), true, true, 0).window.opacity >= 0.10, "light={light}"); }
}

#[test]
fn rainbow_spreads_its_six_colours() {
    let a = accents_for(FLAGS[0].2, &theme(false), false);
    assert_eq!(a.gradient_len, 0, "no gradient when switched off");
    let roles = [a.unread, a.scrollbar_hover, a.mention, a.button, a.link, a.focus];
    let distinct: std::collections::HashSet<u32> = roles.iter().copied().collect();
    assert_eq!(distinct.len(), 6);
}

#[test]
fn settings_pick_flag_washes_and_decorations() {
    let mut p = Pride::default();
    assert!(p.flags & deco::AVATAR_RING_SELF != 0 && p.flags & deco::AVATAR_RING_ALL == 0, "own avatar on, others off by default");
    let json = p.settings().unwrap().to_json();
    for (id, ..) in decor::TOGGLES { assert!(json.contains(&format!("\"id\":\"{id}\"")), "{id}"); }
    p.settings_changed(&Values::parse(r#"{"flag":"trans","gradient":false,"wash_window":false,"send_burst":false,"avatar_all":true}"#));
    assert_eq!((FLAGS[p.flag].0, p.gradient), ("trans", false));
    assert!(p.flags & deco::SEND_BURST == 0 && p.flags & deco::AVATAR_RING_ALL != 0 && p.flags & deco::RAIL_STRIPE != 0);
    let d = p.decor(&ThemeInfo::default()).unwrap();
    assert_eq!(d.window.stops_len, 0, "window wash off");
    assert!(d.list_card.stops_len > 0);
    p.settings_changed(&Values::parse(r#"{"flag":"nope"}"#));
    assert_eq!(p.flag, 0);
    assert!(p.accents(&ThemeInfo::default()).is_some());
}
