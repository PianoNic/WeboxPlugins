# Auto Reply

Answers incoming messages in your own voice. Off by default; by default it only suggests the reply in a toast ("Suggest
in a toast"), "Send automatically" posts it after the delay. It answers 1:1 chats, optionally also groups where you are
@-mentioned, filtered by an allow or deny list of display names. It never answers your own messages, skips a message when
you replied yourself during the delay, runs one reply per space at a time and at most one per space per cooldown (default
10 min), so two auto-responders cannot ping-pong. The prompt has the last 30 messages of the space and up to 25 of your
own messages as style examples (this space first, then your messages seen elsewhere since start, kept in memory only;
its own earlier replies are left out), and asks for the reply text only, in the incoming message's language, as short as
you write, or `NO_REPLY` (only for messages that just acknowledge or close the conversation; a greeting or a question
always gets a reply). Every outcome goes to the plugin log (Settings > Plugins); in suggestion mode each one also shows as
a toast, so a missing suggestion always says why: "No reply needed", "Provider error: HTTP 401: …", an empty or
reasoning-only answer, no history. Provider errors show as a toast in both modes. On a host without `message.received` / `space.send` / `space.history` it says so in a toast.

Providers (each runs as a child process in the temp folder, killed on `stop`; the path is configurable):

| Provider | Invocation |
|---|---|
| Claude Code | `claude -p --output-format json --tools "" --system-prompt <instructions> --no-session-persistence --strict-mcp-config --disable-slash-commands --setting-sources "" [--model M]`, prompt on stdin, reads `result` / `is_error`. Without `--tools ""` and `--setting-sources ""` each call would carry the user's whole Claude Code setup. |
| Codex | `codex exec --skip-git-repo-check --sandbox read-only --color never [-m M] -`, prompt on stdin, the final message on stdout. |
| Cursor | `cursor-agent -p --output-format json [--model M] <prompt>`, reads `result`. |
| Custom endpoint | `curl -sS --max-time 120 -w "\n%{http_code}" -K -`: URL, headers (`Authorization: Bearer <token>`) and the chat-completions body go in curl's config on stdin, so the token is never on a command line; reads `choices[0].message.content` (a string or text parts, `<think>…</think>` removed) / `error.message`, and reports errors with the HTTP status. The token is a password setting (OS keyring). |

"Test connection" runs the provider once (the plugin must be switched on) and shows the answer or the error as a toast.
