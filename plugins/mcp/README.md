# Webex MCP

Lets a local AI read your Webex through Webox: Claude Code, Claude Desktop, LM Studio, or any other client that speaks the
[Model Context Protocol](https://modelcontextprotocol.io). Webox runs the MCP server itself, so the AI sees what Webox has
loaded and needs no Webex login of its own. Read-only by default.

Needs a Webox with the host methods `spaces.list`, `space.history` (2), `messages.search`, `people.search`, `ui.confirm`,
`settings.set` and `clipboard.write` (docs/plugins.md in the Webox repository). On an older Webox the plugin says so in a toast
and starts no server.

## Tools

| Tool | Arguments | What it returns |
|---|---|---|
| `list_spaces` | `query?`, `limit?` (50, max 500) | spaces and 1:1 chats, latest activity first: `id`, `title`, `direct`, `unread`, `last`, `writable` |
| `read_messages` | `space` (id or exact title), `limit?` (30, max 200) | the newest messages, oldest first: `author`, `email`, `mine`, `text`, `date` / `time` (UTC), `id` |
| `search_messages` | `query`, `space?`, `limit?` (20, max 200) | loaded messages containing the text, newest first, with their space |
| `search_people` | `query`, `limit?` (20, max 200) | people by name or e-mail, with their presence (`status`) |
| `send_message` | `space`, `text` | only listed and callable when "Allow the AI to send messages" is on; every message is confirmed in Webox first |

The AI reads what Webox has loaded (the newest 200 messages of every space opened or active since Webox started); nothing
is fetched from Webex for it. `search_messages` searches those messages, like Webox's own search; Webex's server-side
search is not used.

## Settings (Settings > Plugins > Webex MCP)

| Setting | Default | |
|---|---|---|
| MCP server on | off | Nothing listens unless the plugin is switched on **and** this is on. |
| Port | 39847 | Address `http://127.0.0.1:<Port>/mcp`. |
| Token | generated | 64 hex characters from the OS random generator, made on the first start and kept as a password setting (Windows Credential Manager / Secret Service). "Show token" shows it, "Generate new token" replaces it (the old one stops working at once). |
| Copy connection | | puts the Claude Code command and an `mcp.json` with address and token on the clipboard |
| Allow the AI to send messages | off | adds `send_message` |
| Only these spaces | empty = all | one space title or id per line; other spaces do not exist for the AI (not listed, not readable, left out of search results) |
| Show recent calls | | the last calls (also in the plugin log on the Plugins page) |

## Security

- **Loopback only.** The server binds `127.0.0.1` and serves loopback peers only.
- **Token.** Every request needs `Authorization: Bearer <token>` (compared in constant time); without it: 401.
- **DNS rebinding / web pages.** The `Host` header must be `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>`, and an
  `Origin` header, if present, must be a page on this machine; otherwise: 403, as the MCP specification recommends.
- **Sending** is off by default. When on, every message opens a dialog in Webox (`Webex MCP: Claude Code wants to send to
  "Project Alpha": …` with **Send** / **Decline**); only Send sends. Closing the dialog, 2 minutes without an
  answer, or switching the plugin off count as Decline. Read-only spaces are refused before asking.
- **Log.** Every tool call goes to the plugin log with the client's name (from `initialize`), the tool and the space, never
  message text: `Claude Code: read_messages (Project Alpha) -> ok`. Refused requests are logged too.
- Message texts are written by other people. The server's instructions tell the AI to treat them as data, not
  instructions; still, an AI allowed to send can be talked into wanting to, which is why every message is confirmed.

## Connecting a client

Replace `TOKEN` (and the port, if changed). "Copy connection" fills both in.

**Claude Code**

```sh
claude mcp add --transport http webex http://127.0.0.1:39847/mcp --header "Authorization: Bearer TOKEN"
```

**Claude Desktop** (`claude_desktop_config.json`; Claude Desktop starts local servers as commands, so
[`mcp-remote`](https://www.npmjs.com/package/mcp-remote) bridges to the HTTP server; needs Node.js)

```json
{
  "mcpServers": {
    "webex": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "http://127.0.0.1:39847/mcp", "--header", "Authorization:${WEBEX_AUTH}"],
      "env": { "WEBEX_AUTH": "Bearer TOKEN" }
    }
  }
}
```

**LM Studio** (Program > Install > Edit `mcp.json`)

```json
{
  "mcpServers": {
    "webex": { "url": "http://127.0.0.1:39847/mcp", "headers": { "Authorization": "Bearer TOKEN" } }
  }
}
```

**Any other client** (`mcp.json` of Cursor, Windsurf, Ollama front ends, ...): Streamable HTTP at
`http://127.0.0.1:39847/mcp` with the header `Authorization: Bearer TOKEN`.

```json
{
  "mcpServers": {
    "webex": { "type": "http", "url": "http://127.0.0.1:39847/mcp", "headers": { "Authorization": "Bearer TOKEN" } }
  }
}
```

## Protocol

MCP Streamable HTTP, protocol versions 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05: `POST /mcp` with one JSON-RPC
message or a batch, answered as `application/json` (requests: 200, only notifications: 202). `initialize` returns an
`Mcp-Session-Id`; `DELETE /mcp` ends it. No server-sent events (`GET /mcp` answers 405). Methods: `initialize`, `ping`,
`tools/list`, `tools/call`, notifications. Built with `std` only (no HTTP crate): one accept thread and a short-lived thread per
connection, all ended before the plugin's `stop` returns.
