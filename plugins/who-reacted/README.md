# Who Reacted

Shows who reacted without opening the list: next to each reaction chip on a message, small round avatars of the people
who reacted that way (photo, or initials without one), slightly overlapping, up to a limit and then "+N". Hovering an
avatar shows the person's name. Inspired by Equicord's [WhoReacted](https://github.com/Equicord/Equicord/tree/main/src/plugins/whoReacted).

Webox draws the avatars itself, from the people and photo caches it already has (no extra network requests). The plugin
only switches them on and configures them through the host method `reactions.show_avatars` (Webox plugin API 1.6), in
`start` and on every settings change. Switched off, Webox draws its chips as before.

| Setting | Default | Meaning |
|---|---|---|
| Show avatars | on | show the avatars at all |
| Show at most | 3 | at most this many avatars per reaction (1-10); the rest is "+N" |
| Size | Small | Small (14 pt) or Medium (18 pt) |
| Hide your own reaction | off | leave your own avatar out (and out of "+N") |

On a Webox that does not offer `reactions.show_avatars` it says so in a toast and does nothing else.

Compared with Equicord's plugin: no profile popout on click (a click on the count still opens Webox's reactors list), and
no fetching of reactors: Webox already receives them with the message.
