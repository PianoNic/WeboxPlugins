# Nachrichtenprotokoll

Equicord's MessageLogger for Webox. Deleted messages stay visible, marked as deleted, and edited messages show their earlier
versions, optionally as a coloured diff. Webox draws everything; the plugin only decides what to keep.

It needs a Webox with the contract 1.6 events `message.deleted` / `message.edited` and the method `message.keep`; on an
older Webox it says so in a toast. The menus, the history dialog, diffs and folding need the newer names (`message.menu`,
`space.menu`, `message.edit_tag_clicked`, `ui.dialog`, `message.keep` 2); an older Webox simply leaves them out. Webox
notices a delete or an edit when it loads the space (opening it, and after edits, deletes and reactions there), and only for
messages it had loaded: what happened while Webox was closed is not logged.

## Settings (Equicord's, same names and defaults)

| Setting (id) | Default | |
| --- | --- | --- |
| Darstellung gelöschter Nachrichten (`deleteStyle`) | Roter Text | or Rote Überlagerung |
| Gelöschte Nachrichten protokollieren (`logDeletes`) | on | |
| Gelöschte Nachrichten einklappen (`collapseDeleted`) | off | like blocked messages: "N gelöschte Nachrichten", a click unfolds them |
| Anhänge gelöschter Nachrichten behalten (`logDeletedAttachments`) | on | off: a kept deleted message loses its pictures, files and link cards. (Webex edits cannot remove an attachment, so this is about deleted messages.) |
| Bearbeitungen protokollieren (`logEdits`) | on | |
| Bearbeitungsverlauf in der Nachricht anzeigen (`inlineEdits`) | on | off: only "(bearbeitet)"; its dialog shows the versions |
| Änderungen farbig anzeigen (`showEditDiffs`) | off | each earlier text as a diff against the next one: added green, removed red |
| Hinzufügungen und Entfernungen getrennt anzeigen (`separatedDiffs`) | off | only with diffs: the first version with what was removed, the current text with what was added |
| Bots ignorieren (`ignoreBots`) | on | Webex bots (`@webex.bot`, `@sparkbot.io` addresses) |
| Eigene Nachrichten ignorieren (`ignoreSelf`) | off | |
| Eigene Bearbeitungen ignorieren (`ignoreSelfEdits`) | off | |
| Ignorierte Personen (`ignoreUsers`) | | names or e-mail addresses |
| Ignorierte Bereiche (`ignoreChannels`) | | space names or ids |
| Ignorierte Teams (`ignoreGuilds`) | | team names or ids (Discord's guilds are Webex teams) |
| Protokoll auch nach Neustart behalten (`persist`) | off | Webox-only: stores the log in Webox's plugin data |
| Protokoll leeren | | forgets everything |

Lists take one entry per line or comma, case-insensitive. Settings apply at once to what is on screen.

## Menus and the history dialog

| Equicord | Webox | Shown on |
| --- | --- | --- |
| Toggle Deleted Highlight | Gelöscht-Markierung umschalten | a kept deleted message: red off / on (this session) |
| Enable / Disable Diff View | Änderungsvergleich aktivieren / deaktivieren | a message with logged edits, while "Änderungen farbig anzeigen" is on (this session) |
| Clear Message History | Nachrichtenverlauf löschen | a logged message: a deleted one goes as Webox deletes any message, an edited one loses its history |
| Clear Message Log (channel menu) | Protokoll leeren (space menu in the list) | a space with something logged |
| HistoryModal (click on "(edited)") | Bearbeitungsverlauf (click on "(bearbeitet)" or "Bearbeitet") | one tab per version with its time and the current one, a greyed tab when the original was never seen, the switch "Unterschiede anzeigen" diffing each version against the newest |

Not taken over: Equicord's hover "Clear history" button (the menu entry does the same; deleted messages have no toolbar in
Webox) and its Discord-only fixes (nonce, `+:emoji:` target, ephemeral messages).

Privacy: the log is kept on this computer only, in memory, and on disk only with "Nach Neustart behalten" (switching that
off empties the stored copy). It is never sent anywhere. At most 500 deleted and 500 edited messages are kept, with up to 20
earlier texts each.
