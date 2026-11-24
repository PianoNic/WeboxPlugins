# Nachrichtenprotokoll

Deleted messages stay visible, marked as deleted, and edited messages show their earlier versions, like Equicord's
MessageLogger. A deleted message keeps its place in the conversation: its text in red with "(gelöscht)" below it, or, with
"Überlagerung", in its normal colour under a red wash. An edited message shows the earlier texts struck through and greyed
above the current one, with "(bearbeitet)" below it. Webox draws both; the plugin only decides what to keep.

It needs a Webox with the contract 1.6 events `message.deleted` / `message.edited` and the method `message.keep`; on an
older Webox it says so in a toast. Webox notices a delete or an edit when it loads the space (opening it, and after edits,
deletes and reactions there), and only for messages it had loaded: what happened while Webox was closed is not logged.

| Setting | Default | |
| --- | --- | --- |
| Gelöschte Nachrichten anzeigen | on | off: deleted messages disappear as usual |
| Bearbeitungen protokollieren | on | earlier texts above edited messages |
| Eigene Nachrichten ignorieren | off | your own deleted / edited messages are left out |
| Darstellung gelöschter Nachrichten | Text rot | or Überlagerung (red wash) |
| Bereiche | Alle Bereiche | "Nur in diesen Bereichen" / "Bereiche ausnehmen", with a list of space names (one per line, case-insensitive; a space id works too) |
| Gelöschte Nachrichten auch nach Neustart behalten | off | stores the log in Webox's plugin data (`plugin-data.json`) and shows it again after a restart |
| Protokoll leeren | | forgets everything |

Privacy: the log is kept on this computer only, in memory, and on disk only with "Nach Neustart behalten" (switching that
off empties the stored copy). It is never sent anywhere. Changing a setting applies at once to what is on screen. At most
500 deleted and 500 edited messages are kept, with up to 20 earlier texts each.

Not taken over from Equicord: ignoring bots (Webex messages do not say whether a bot wrote them), user lists, deleted
attachments (Webox keeps the deleted message with its pictures and files while it is on screen, but after a restart only the
text), and diff views of edits.
