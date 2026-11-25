# Adding a plugin

Every plugin is a crate under `plugins/<name>/`. CI builds everything in the workspace that has a
`[package.metadata.webox]` table, so a new folder is all it takes.

## 1. Copy an example

```sh
cp -r plugins/slash-commands plugins/my-plugin
```

Use `slash-commands` for a plugin with commands or message hooks, and `pride-theme` for a theme that colours Webox's accents.

## 2. Name it

In `plugins/my-plugin/Cargo.toml`:

```toml
[package]
name = "collabhost_my_plugin"   # the library file name: collabhost_my_plugin.dll / libcollabhost_my_plugin.so
version = "0.1.0"
description = "One sentence: what it does."
authors = ["Your GitHub name"]
homepage = "https://github.com/PianoNic/WeboxPlugins/tree/main/plugins/my-plugin"

[package.metadata.webox]
name = "My Plugin"              # shown in Webox
pkg = "dev.webox.plugins.my-plugin"  # reverse-DNS id, lowercase, never changes once published
# icon = "icon.png"             # optional, next to Cargo.toml
# min-webox = "46.10.0"         # optional, lowest Webox version it needs
```

- Keep the `collabhost_` prefix. Webox looks for it when loading loose plugin files next to the program.
- The `pkg` id is the plugin's identity: Webox installs it to `plugins/<pkg>/<version>/` and keys the on/off switch by it.
  Changing it later makes a new plugin, and the old one stays installed.
- `version` and `description` are read from Cargo.toml by the code (`env!("CARGO_PKG_VERSION")`,
  `env!("CARGO_PKG_DESCRIPTION")`) and by the index generator, so there is only one place to edit them.

## 3. Write it

Implement `collabhost_plugin_api::Plugin` and end with `export_plugin!(MyPlugin);`. Every hook has a default that does
nothing:

| Hook | Called when |
| --- | --- |
| `start(host)` / `stop()` | the plugin is switched on / off (also on install, update, uninstall) |
| `on_send(text)` | before a message is sent: `None` = unchanged, `Some("")` = do not send, `Some(t)` = send `t` |
| `on_message(author, text)` | a message is shown: `None` = as is, `Some(t)` = show `t` |
| `command(name, args)` | `/name args` for a name listed in `COMMANDS`: the text to send, or `None` |
| `palette()` | for full themes: backgrounds, text, accent and links as `0xRRGGBBAA`, `0` keeps Webox's colour |
| `accents(theme)` | for accent themes: link, focus, selection, unread, badge, rail, button, control, mention, scrollbar colours and a gradient, computed for the active base theme (`collabhost_plugin_api::color` has WCAG contrast helpers) |
| `settings()` / `settings_changed(values)` / `settings_action(id)` | the plugin's settings form, see below |

`Host` lets a plugin show a toast, send a message, write to the plugin log, and keep small string values between runs
(`host.get` / `host.set`).

### Settings

A plugin can have settings that the user changes on its page in Webox (**Settings > Plugins > your plugin >
Settings**). Declare them, and Webox draws the form, validates and saves the values, and tells the plugin:

```rust
use collabhost_plugin_api::{Schema, Values};

fn settings(&self) -> Option<Schema> {
    Some(Schema::new()
        .section("Replacements", "What is replaced when sending.")
        .toggle("smiley", "Replace \":)\" with 🙂", "", true)          // on / off
        .text("shrug_text", "Text for /shrug", "", "¯\\_(ツ)_/¯")         // one line
        .multiline("notes", "Notes", "", "")                             // text area
        .password("token", "API key", "")                                // masked, kept in the OS keyring
        .number("delay", "Delay (s)", "", 2.0, 0.0, 10.0, 0.5)           // default, min, max, step
        .choice("mode", "Mode", "", "a", &[("a", "Simple"), ("b", "Exact")])
        .button("reset_count", "Reset counter", ""))                     // calls settings_action
}

// All values, every time something changes and once before every start.
fn settings_changed(&mut self, v: &Values) {
    self.smiley = v.bool("smiley").unwrap_or(true);
    self.shrug = v.text("shrug_text").unwrap_or_default().to_string();
}

// A button was pressed: an optional message for a toast.
fn settings_action(&mut self, id: &str) -> Option<String> { (id == "reset_count").then(|| "Counter reset".into()) }
```

- Give the fields in `Default` the same defaults as the schema. Webox sends the saved values before `start`.
- Keep the `id`s stable. A renamed id loses the saved value, and a value that no longer fits the schema falls back to the
  default.
- Write labels, descriptions, toasts and errors in English. Labels are shown as written.
- `host.settings()` returns the current values at any time.
- Settings need contract 1.2, accent themes 1.3, decorations 1.4.

### Translations

Write every text in English; Webox shows it as is unless the plugin ships a translation. To ship one, copy `src/i18n.rs`
and `translations/de.tsv` from any plugin here, add `mod i18n;`, list `i18n.strings` in `EVENTS` and answer it in
`on_event` with `i18n::strings(payload)`. Each `translations/<lang>.tsv` line is `English<TAB>translation` (`\n` for a
line break); add a language by adding its file to `TABLES` in `i18n.rs`. Webox translates the name, the settings page,
menu entries, dialogs and toasts itself; wrap texts the plugin builds at run time (formatted toasts, the result of a
settings button, log lines) in `i18n::t(...)`, fill `{placeholders}` after translating, and call `i18n::sync(host)` in
`start` so `t` knows the language (Webox method `i18n.language`). The `i18n_tests` test fails
when a settings text has no German line. Webox without translation support never sends the event, so the plugin stays
English there (Webox `docs/plugins.md`, "Translations").

### Decorations (themes)

A theme can also return `decor(theme) -> Option<Decor>`. It declares background washes (gradient stops, angle, opacity and
an optional gradient rim for the window, the list card and the content card) and switches on decorations and effects
that Webox draws in your `stripes` colours (`collabhost_plugin_api::deco`: flag band on the rail, avatar rings, gradient
outlines on the selected row and tab, gradient toggles, scrollbars and progress bars, a flag next to the name, confetti
when sending, a ripple on reactions, a June sparkle). Keep every wash only as strong as the text on it allows: see
`plugins/pride-theme/src/decor.rs` (`safe_opacity`) and its tests, which check primary and secondary text over every
wash for every flag on dark and light themes. The full format is described in Webox's `docs/plugins.md` ("Plugin
  settings"). [Slash Commands](plugins/slash-commands/src/lib.rs) is a complete example.

Rules that keep Webox safe and stable:

- Do not keep threads running after `stop()` returns. Webox may unload the library right after.
- Panics are caught at the boundary, but a hook that panicked counts as "no change". Do not rely on it.
- Plugins run with the user's session. Never send data anywhere the user did not ask for. Pull requests that do are
  rejected.

## 4. Test it

```sh
cargo run -p xtask -- check                 # metadata: unique pkg, valid version
cargo test -p collabhost_my_plugin
cargo build --release -p collabhost_my_plugin
```

To try it in Webox, copy `target/release/collabhost_my_plugin.dll` (Linux: `libcollabhost_my_plugin.so`) into Webox's
plugin folder (**Settings > Plugins > Open plugin folder**) and press **Reload**.

## 5. Open a pull request

Use a conventional commit title in the past tense, e.g. `feat: added My Plugin`. CI builds the plugin for Windows and
Linux. After the merge it appears in every Webox under **Available** within a day, or right away after
**Update**.

## Updating a plugin

Change the code, **bump `version`** in its Cargo.toml (semver: patch for fixes, minor for features), and open a pull
request. Webox offers the update to everyone who has an older version installed. A merge without a version bump replaces
the binary under the same version, which installed copies never pick up, so always bump.

## Plugin API versions

Each binary records the plugin-api contract it was built against (`contract` in the index). Webox installs a plugin only
if the major version matches its own and the minor version is not newer. When Webox raises the contract, the plugins here
are rebuilt by the next CI run. Bump their versions so that installed copies update.
