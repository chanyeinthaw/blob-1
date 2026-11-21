# Feature map

Where each feature lives: code, CLI, model tools and storage.

## Crates

| crate | role |
|---|---|
| `agent` | CLI and composition root: `serve`, `chat`, configuration commands |
| `harness` | chat harness, tasks, sessions, prompts, messenger tools, calendar, watches, utility model |
| `memory` | chat memory: append-only messages, summary tree, views, zoom |
| `llm` | `Llm` trait and request types, quota fallback |
| `llm-rig` | `Llm` over rig for OpenAI Responses and Chat Completions providers |
| `environment` | tool types (`read`, `write`, `edit`, `bash`) and the `Environment` trait |
| `environment-local` | local implementation of the environment tools, skill sync |
| `messenger` | `Messenger` and `Lookup` traits and neutral message types |
| `messenger-discord` | Discord messenger over twilight |
| `mcp` | MCP client manager: stdio and Streamable HTTP transports, OAuth, deferred tools |
| `storage` | storage traits, `Meta`, XML rendering of model input |
| `storage-sqlite` | SQLite implementation of every store |

## Web settings

- Code: `crates/agent/src/web.rs`, `crates/storage-sqlite/src/web_auth.rs`, `web/src/session.ts`, `web/src/api/{client,ws,mock,protocol}.ts`, `web/src/page/settings/{identity,models,providers,discord,mcp}.ts`
- `agent serve --web <addr>` serves the app (default `127.0.0.1:7878`); `agent web password set` sets the password.
- `agent serve --public-url <url>` sets the absolute http(s) URL the app is publicly reached at (e.g. `https://silicon.tailcd7ccb.ts.net:7878` via Tailscale Serve proxying to `--web 127.0.0.1:7878`): the OAuth redirect URI and `mcp.login` URL use it instead of the Host header, the WebSocket Origin check accepts exactly that origin, and the session cookie gets `Secure` when it is https. `serve` persists it so `agent mcp login` prints the same URL; running `serve` without it clears the stored value.
- The session cookie is `HttpOnly; SameSite=Lax` so the cross-site OAuth callback's top-level GET still carries it.
- HTTP endpoints: `/api/session`, `/api/login`, `/api/logout`, `/api/mcp/login`, `/api/mcp/callback`, `/ws`.
- Tables: `web_password`, `web_sessions`, `public_url`.
- Discord starts once `DISCORD_BOT_TOKEN` is set and a main model resolves, including while `serve` is already running; otherwise `serve` runs web-only until then. A Discord failure exits the process.
- Model and provider changes apply to the running bot without a restart: each turn, spawned task, compaction pass and utility call resolves from current settings; a run already in progress keeps its model, a task keeps the model it was spawned with for its lifetime, and a role whose settings no longer resolve keeps its last working model. The `model`, `models`, `provider` and `reasoning` commands run while `serve` holds the data directory; `serve`, `chat` and `skill` still need it exclusively.
- The mock transport is enabled only with `VITE_MOCK=1`.

## Chat

The bot is one chat that never ends. It coordinates and talks; tasks do the work.

- Code: `crates/harness/src/chat.rs` (`ChatHarness`, `CHAT_PROMPT`, `MESSENGER_PROMPT`), `crates/harness/src/prompt.rs`
- Runs: a run ends when the agent is done. Task reports and reminders steer a live run or wake an idle agent (`resume`).
- Input: every outside input is one XML element with escaped text: `<message>`, `<report>`, `<reminder>`, `<scheduled>`, `<checkin>`
- Final reply `NO_UPDATES` ends a messenger turn silently.
- Tools: `zoom`, `date`, `spawn`, `models`, `tasks`, `tell`, `stop`, plus messenger, calendar, watch and MCP tools
- MCP tool results carrying png, jpg, gif or webp images reach the model as images when the main model accepts image input; the memory echo keeps the `[N image(s) omitted]` note
- CLI: `agent chat <message>` (one message, then waits for its tasks to report)

## Identity

- Code: `crates/harness/src/chat.rs` (`DEFAULT_IDENTITY`)
- Prompt: `# Identity` block at the top of the chat system prompt, default `You are Blob-1.`, read every turn
- CLI: `agent identity show | set <file|-> | reset`
- Table: `identity`

## Memory

- Code: `crates/memory/src/lib.rs`, `crates/storage-sqlite/src/memory.rs`
- Append-only rows with JSON `meta`; binary summary tree; chat view (128,000 bytes) and seen view (32,000 bytes); 512-byte nodes; `zoom`, `date`
- Compaction model summarizes in the background
- Tables: `memory_messages`, `memory_nodes`, `memory_views`, `memory_view_parts`

## Tasks

Background workers spawned by the chat.

- Code: `crates/harness/src/tasks.rs`, `crates/harness/src/session.rs`, `crates/harness/src/compaction.rs`
- Tools: `read`, `write`, `edit`, `bash`, read-only `zoom` and `date`, read-only `history`, `search`, `participants`, `message`; never `send`; plus MCP tools
- Reports: `<report task status [for]>`; a 15-minute watch per run sends `<reminder task running last_activity [for]>`
- `spawn` with `for: "m…"` links a task to a messenger message
- Images: `read` returns png, jpg, gif and webp files as images on models with image input
- Tables: `sessions`, `messages`, `message_images`, `task_choices`

## Models

- Code: `crates/agent/src/run.rs` (`setup`, `chat_harness`, `ModelFactory`), `crates/llm-rig/src/lib.rs`
- Main model: `agent model set <provider>/<model>`, `agent reasoning set <level>`
- Compaction model: `agent model compaction <provider>/<model> [--reasoning]`
- Utility model: `agent model utility <provider>/<model> [--reasoning] | off`
- Per model: `agent model map`, `agent model context`, `agent model input <provider/model|provider> <text|image>`
- Providers: `agent provider add | list | remove`, `agent models refresh | list`
- Tables: `providers`, `models`, `model_reasoning`, `model_images`, `settings`, `utility_model`

## Utility model

One-shot generation: one request, no tools, no memory, no session.

- Code: `crates/harness/src/utility.rs` (`Utility::generate`, `parse_inline`)
- Used by the reply-mode gate and thread titles; without it, both are off

## Messengers (Discord)

- Code: `crates/messenger/src/lib.rs`, `crates/messenger-discord/src/{lib,gateway,render}.rs`, `crates/harness/src/messenger.rs`, `crates/agent/src/run.rs` (`serve`, `admit`)
- Gate: DMs, mentions of the bot and replies to the bot, from allowlisted users
- Reply-mode gate: with a utility model, a plain-channel message is answered inline or in a thread (5-second budget, fallback thread)
- Threads: replies to a plain-channel message start a thread from it; bot-created threads get a generated title
- Pings: one ping per message per turn; follow-ups in a bot run post plainly
- Acknowledgment: 👀 on admit; ⏳ while a `for` task runs, then ✅ or ⚠️
- Typing: pulses every 8 seconds in the conversation while a message is unanswered (up to 10 minutes) and while the bot answers a task report
- References: replied-to message as `<quote>`; attachments as `<attachment>`
- Tools: `send` (chat only), `history`, `search`, `participants`, `message` (chat and tasks)
- CLI: `agent serve` (needs `DISCORD_BOT_TOKEN`), `agent messenger allow | disallow | list`
- Tables: `destinations`, `allowed_users`

## Calendar

- Code: `crates/harness/src/schedule.rs` (`run_schedules`), `crates/storage-sqlite/src/schedule.rs`
- One-off (`at`) or recurring (`cron`) jobs with their own timezone and optional `to` or `reply`
- Fires as `<scheduled id due [late] [to] [for] [home]>` while `serve` runs; a missed job fires once, marked late
- Tools: `schedule`, `unschedule`, `schedules`
- CLI: `agent schedule add | list | remove`, `agent timezone show | set`, `agent home show | set | clear`
- Tables: `schedules`, `clock`, `home`

## Watches and check-ins

- Code: `crates/harness/src/watch.rs`, `crates/harness/src/schedule.rs` (`next_checkin`), `crates/storage-sqlite/src/{watch,checkin}.rs`
- Watches: things the bot keeps an eye on, with optional `to`, `reply` and `until`
- Check-ins: every interval within an hour window on chosen weekdays, only when watches exist; fire as `<checkin time [home]>` with a `<watch>` per item
- Results with no destination and no home go in the final reply, kept in memory and printed by `serve`
- Tools: `watch`, `unwatch`, `watches`
- CLI: `agent watch list | remove`, `agent checkin show | set --every --days --hours | off`
- Tables: `watches`, `checkin`

## Skills

- Code: `crates/environment-local/src/lib.rs` (sync), `crates/storage-sqlite/src/skill.rs`, `crates/agent/src/import.rs`
- Stored in the database, synced to the skills directory with a `.managed.json` manifest; tasks load them
- CLI: `agent skill add | list | remove`
- Tables: `skills`, `skill_files`

## MCP

Blob-1 is an MCP client: chat and background tasks can call tools, read resources and use prompts from configured servers.

- Code: `crates/mcp/src/{lib,oauth,tools}.rs`, `crates/storage-sqlite/src/mcp.rs`, `crates/agent/src/web.rs` (`/api/mcp/login`, `/api/mcp/callback`), `web/src/page/settings/mcp.ts`
- Servers are stdio (command, args, env) or Streamable HTTP (url, static headers whose values are env var names); no legacy SSE
- One shared `mcp::Mcp` per `serve` keeps one connection per server for chat and tasks; config changes apply without a restart
- Server ids are lowercase letters, digits and `-` (no `_`: it separates `server__tool` wire names)
- Connect waits up to 30s; each MCP request times out at 60s (tool calls 5 min). A dead, timed-out or failed transport drops the connection so the next call reconnects; in-flight calls keep their own handle — disabling or removing a server lets running calls finish — and a dropped `tools/call` reports its outcome as unknown rather than retrying. The `mcp.list` status probe never drops a connection on timeout — it reports the last known tool count instead. Stdio servers run in their own process group, killed explicitly whenever the connection closes even if the leader already exited, so wrappers like `npx` can't strand grandchildren; `serve`/`chat`/`mcp tools` close and await them on exit
- OAuth 2.1: protected-resource and authorization-server metadata, dynamic client registration, PKCE, refresh; `agent mcp login <id>` or the settings page opens the flow — fetching the `WWW-Authenticate` challenge with an initialize POST when no prior connect captured one — `GET /api/mcp/callback` finishes it; tokens and client registration (including the discovery challenge, used to rebuild the client for refresh) persist. Enable/disable keeps them; replacing a server's transport config clears its OAuth data; the registration travels with the pending login state so concurrent logins can't clobber each other
- Tools: deferred discovery — `mcp_search` loads a server's tools into the session as `server__tool` (sanitized, hash suffix on collisions) and a loaded tool dispatches only once it has been advertised to a completion, `mcp_resources`, `mcp_read`, `mcp_prompts`, `mcp_prompt`; the client does not advertise sampling, roots or elicitation and ignores logging and progress notifications
- Servers connect lazily on first use; statuses are `connected`, `connecting` (a connect attempt is in flight), `idle` (enabled, never connected), `needs-login`, `error` (with text) and `disconnected` (disabled)
- Web protocol: `mcp.list`, `mcp.add`, `mcp.remove`, `mcp.setEnabled`, `mcp.login`
- CLI: `agent mcp list`, `agent mcp add <id> [--env K=V] -- <command> [args]`, `agent mcp add <id> --url <url> [--header 'Name: ENV_VAR']`, `agent mcp remove <id>`, `agent mcp enable | disable <id>`, `agent mcp login <id>` (prints the stored `--public-url` base when `serve` set one, else `--web`), `agent mcp tools <id>`
- Tables: `mcp_servers`, `mcp_tokens`, `mcp_oauth_states`

## Dev server

- Files: `Dockerfile`, `compose.yaml`
- `docker compose up -d --build` runs `agent serve` with host networking and the `agent-data` volume; `CPA1_KEY` and `DISCORD_BOT_TOKEN` come from the shell
- The runtime stage is `ubuntu:24.04` with the utility packages plus Node.js LTS (`npm`, `npx`, `corepack`) and Python 3 with `uv`/`uvx`, so stdio MCP servers can run inside the container
