# Message Logger

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
| Delete style (`deleteStyle`) | Red text | or Red overlay |
| Log deleted messages (`logDeletes`) | on | |
| Collapse deleted messages (`collapseDeleted`) | off | like blocked messages: "N deleted messages", a click expands them |
| Log deleted attachments (`logDeletedAttachments`) | on | off: a kept deleted message loses its pictures, files and link cards. (Webex edits cannot remove an attachment, so this is about deleted messages.) |
| Log edits (`logEdits`) | on | |
| Show edits inline (`inlineEdits`) | on | off: only "(edited)"; its dialog shows the versions |
| Show edit diffs (`showEditDiffs`) | off | each earlier text as a diff against the next one: added green, removed red |
| Show separated diffs (`separatedDiffs`) | off | only with diffs: the first version with what was removed, the current text with what was added |
| Ignore bots (`ignoreBots`) | on | Webex bots (`@webex.bot`, `@sparkbot.io` addresses) |
| Ignore your own messages (`ignoreSelf`) | off | |
| Ignore your own edits (`ignoreSelfEdits`) | off | |
| Ignored people (`ignoreUsers`) | | names or e-mail addresses |
| Ignored spaces (`ignoreChannels`) | | space names or ids |
| Ignored teams (`ignoreGuilds`) | | team names or ids (Discord's guilds are Webex teams) |
| Keep the log after a restart (`persist`) | off | Webox only: stores the log in Webox's plugin data |
| Clear Message Log | | forgets everything |

Lists take one entry per line or comma, not case-sensitive. Settings apply at once to what is on screen.

## Menus and the history dialog

The entries carry Equicord's names.

| Entry | Shown on | What it does |
| --- | --- | --- |
| Toggle Deleted Highlight | a kept deleted message | red off / on (this session) |
| Enable Diff View / Disable Diff View | a message with logged edits, while "Show edit diffs" is on | the diff for this message off / on (this session) |
| Clear Message History | a logged message | a deleted one goes as Webox deletes any message, an edited one loses its history |
| Clear Message Log | the space menu in the list, on a space with something logged | forgets everything logged in that space |
| Message Edit History | a click on "(edited)" | one tab per version with its time and the current one, a greyed tab when the original was never seen, and the switch "Show Diff" diffing each version against the newest |

Not taken over: Equicord's hover "Clear history" button (the menu entry does the same; deleted messages have no toolbar in
Webox) and its Discord-only fixes (nonce, `+:emoji:` target, ephemeral messages).

Privacy: the log is kept on this computer only, in memory, and on disk only with "Keep the log after a restart" (switching
that off empties the stored copy). It is never sent anywhere. At most 500 deleted and 500 edited messages are kept, with up
to 20 earlier texts each.
