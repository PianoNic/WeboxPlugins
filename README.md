<p align="center">
  <img src="assets/webox-plugins.svg" width="180" alt="Webox Plugins Logo" />
</p>
<p align="center">
  <strong>Webox Plugins</strong><br/>
  The official plugin repository for Webox. Built by CI, installed with one click.
</p>
<p align="center">
  <a href="https://github.com/PianoNic/WeboxPlugins"><img src="https://badgetrack.pianonic.ch/badge?tag=weboxplugins&label=visits&color=07C1F5&style=flat" alt="visits" /></a>
  <a href="https://github.com/PianoNic/Webox"><img src="https://img.shields.io/badge/For-Webox-07C1F5.svg" alt="Webox" /></a>
  <a href="https://raw.githubusercontent.com/PianoNic/WeboxPlugins/repo/index.json"><img src="https://img.shields.io/badge/Repository-index.json-07C1F5.svg" alt="Repository index" /></a>
  <a href="CONTRIBUTING.md"><img src="https://img.shields.io/badge/Add-a%20plugin-07C1F5.svg" alt="Add a plugin" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-07C1F5.svg" alt="MIT license" /></a>
</p>

---

## What is this?

[Webox](https://github.com/PianoNic/Webox) can be extended with native plugins: slash commands, message filters, colour themes. A plugin can have its own settings, which Webox shows on the plugin's page. This repository holds the official ones, one crate per plugin, in a single Cargo workspace.

On every push to `main`, CI builds every plugin for Windows and Linux, hashes the binaries and publishes them, together with an index, to the [`repo`](https://github.com/PianoNic/WeboxPlugins/tree/repo) branch. Webox reads that index, lists the plugins under **Einstellungen > Plugins > Verfügbar**, and installs, updates and removes them without a restart. This works like extension repositories in Aniyomi.

## Plugins

| Plugin | What it does |
| --- | --- |
| [Slash Commands](plugins/slash-commands) | `/shrug`, `/tableflip` and `/count`, and turns `:)` into a smiley. Settings: the smiley switch, your own `/shrug` text, a counter reset. |
| [Nachrichtenprotokoll](plugins/message-logger) | Deleted messages stay visible, marked as deleted (red text or a red overlay), and edited messages show their earlier versions struck through. Settings: deletes, edits, ignore your own, which spaces, keep after a restart. Stored only on this computer. |
| [Pride Theme](plugins/pride-theme) | Webox in the colours of a Pride flag: accents, a wash behind rail and header, rainbow card rims, a flag band on the rail, avatar rings, rainbow toggles, scrollbars and send button, confetti when sending, a June "Happy Pride!". Eight flags (Rainbow, Progress Pride, Trans, Bi, Lesbian, Pan, Non-binary, Ace); every decoration is a switch. Text stays readable (WCAG AA) on every theme, dark and light. |

## Installing

Webox has this repository built in. Open **Einstellungen > Plugins**, switch to **Verfügbar** and press **Installieren**.

Any other repository built the same way can be added under **Repositories** with the URL of its `index.min.json`:

```
https://raw.githubusercontent.com/PianoNic/WeboxPlugins/repo/index.min.json
```

> [!WARNING]
> Plugins are native code. They run inside Webox with full access to your account and your signed-in session. Only install plugins from sources you trust.

Webox only loads what the index promises. Each binary is checked against the SHA-256 and size listed in the index, both during the download and again before it is loaded. Downloads use HTTPS only, and redirects may lead only to GitHub's own hosts.

## Building

```sh
cargo test
cargo build --release
cargo run --release -p xtask -- collect --out dist        # binaries + manifest for this platform
cargo run -p xtask -- index --dist dist --repo repo       # index.json, index.min.json and files/ as published
```

plugin-api comes from the Webox repository as a git dependency. To build against a local Webox checkout instead, run
`pwsh scripts/use-local-api.ps1 ../Webox` (or `scripts/use-local-api.sh ../Webox`); `--off` switches back.

The index format and how Webox checks it are described in Webox's [`docs/plugins.md`](https://github.com/PianoNic/Webox/blob/main/docs/plugins.md).

## License

MIT, see [LICENSE](LICENSE).
