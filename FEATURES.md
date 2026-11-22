# Feature map

Where each feature lives: code, CLI, model tools and storage.

## Crates

| crate | role |
|---|---|
| `agent` | CLI and composition root: `serve`, `chat`, `update`, `launcher`, configuration commands |
| `harness` | chat harness, tasks, sessions, prompts, messenger tools, calendar, watches, utility model |
| `memory` | chat memory: append-only messages, summary tree, views, zoom |
| `llm` | `Llm` trait and request types, quota fallback |
| `llm-rig` | `Llm` over rig for OpenAI Responses and Chat Completions providers |
| `environment` | tool types (`read`, `write`, `edit`, `bash`) and the `Environment` trait |
| `environment-local` | local implementation of the environment tools, skill sync |
| `messenger` | `Messenger` and `Lookup` traits and neutral message types |
| `messenger-discord` | Discord messenger over twilight |
| `mcp` | MCP client manager: stdio and Streamable HTTP transports, OAuth, static `mcp_search`/`mcp_execute` tools |
| `storage` | storage traits, `Meta`, XML rendering of model input |
| `storage-sqlite` | SQLite implementation of every store |

## Web settings

- Code: `crates/agent/src/web.rs`, `crates/storage-sqlite/src/web_auth.rs`, `packages/contracts/src/protocol.ts` (wire schemas), `packages/client/src/{api,ws,session,mock}.ts` (API client), `apps/web/src/routes/*`, `apps/web/src/lib/{session.ts,shellMachines.tsx,useBackgroundRefresh.ts}` (cached session guard, shell-scoped persistent page machines, stale-while-revalidate rendering), and `apps/web/src/features/{session,identity,schedules,models,providers,chat,mcp}/` (pages)
- Settings navigation is instant: once signed in, `requireSession` returns from an in-memory cache (cleared on sign-out or session expiry), `Shell` prefetches all 6 `Atom.keepAlive` settings page atoms, `ShellMachinesProvider` keeps page machines alive across tab switches so pending save queues never cancel on navigation, `matchCached` keeps cached pages visible if a background refresh fails, and toggles/pickers update optimistically with coalesced background saves.
- The app is a pnpm workspace (vite-plus, React, TanStack Router, shadcn on Base UI, Effect, effect-machine) that `agent` embeds from `apps/web/dist`; `VITE_MOCK=1 pnpm dev` runs it against an in-memory mock (password `blob`)
- `agent serve --web <addr>` serves the app (default `127.0.0.1:7878`); `agent web password set` sets the password.
- `agent serve --public-url <url>` sets the absolute http(s) URL the app is publicly reached at (e.g. `https://silicon.tailcd7ccb.ts.net:7878` via Tailscale Serve proxying to `--web 127.0.0.1:7878`): the OAuth redirect URI and `mcp.login` URL use it instead of the Host header, the WebSocket Origin check accepts exactly that origin, and the session cookie gets `Secure` when it is https. `serve` persists it so `agent mcp login` prints the same URL; running `serve` without it clears the stored value.
- The session cookie is `HttpOnly; SameSite=Lax` so the cross-site OAuth callback's top-level GET still carries it.
- HTTP endpoints: `/api/session`, `/api/login`, `/api/logout`, `/api/mcp/login`, `/api/mcp/callback`, `/ws`, `/p/{id}`, `/p/{id}/{name}` (public shared workspace files, unauthenticated capability URLs, served with `Content-Security-Policy: sandbox allow-scripts` and `X-Content-Type-Options: nosniff`; Markdown served inline as `text/plain; charset=utf-8`).
- Tables: `web_password`, `web_sessions`, `public_url`, `public_files`.
- The Discord settings live on the **Chat platforms** page at `/settings/chat/discord`: token Replace/Remove, connection status (`off` / `connecting` / `online` with the bot name / `error` with a short message), the user allowlist, Discord-wide interaction and reply defaults, per-channel, per-person and per-server interaction-mode overrides and the home channel or DM. Discord is the only chat platform today.
- Model and provider changes apply to the running bot without a restart: each turn, spawned task, compaction pass and utility call resolves from current settings; a run already in progress keeps its model, a task keeps the model it was spawned with for its lifetime, and a role whose settings no longer resolve keeps its last working model. The `model`, `models`, `provider` and `reasoning` commands run while `serve` holds the data directory; `serve`, `chat` and `skill` still need it exclusively.
- The mock transport is enabled only with `VITE_MOCK=1`.

## Chat

The bot is one chat that never ends. It coordinates and talks; tasks do the work.

- Code: `crates/harness/src/chat.rs` (`ChatHarness`, `CHAT_PROMPT`, `MESSENGER_PROMPT`), `crates/harness/src/prompt.rs`
- Runs: a run ends when the agent is done. Task reports and reminders steer a live run or wake an idle agent (`resume`).
- Input: every outside input is one XML element with escaped text: `<message>`, `<report>`, `<reminder>`, `<scheduled>`, `<checkin>`
- Final reply `NO_UPDATES` ends a messenger turn silently.
- Tools: `zoom`, `date`, `spawn`, `models`, `tasks`, `tell`, `stop`, `share` (creates public capability links in `public_files` for workspace files or inline Markdown/HTML under `workspace/shares/`; also available to background tasks), plus messenger, calendar, watch and static MCP (`mcp_search`, `mcp_execute`, `mcp_resources`, `mcp_read`, `mcp_prompts`, `mcp_prompt`) tools
- Prompt caching: `ChatHarness` and `SessionHarness` pass a stable per-workspace/session `prompt_cache_key` on OpenAI Responses (`llm-rig`), and keeping MCP behind static `mcp_search` + `mcp_execute` avoids mutating the `tools` list mid-turn.
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
- `read`, `write` and `edit` resolve paths against the task's workspace root but are not restricted to it: `..` and absolute paths reach anywhere the process can
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
- Providers: `agent provider add [--no-key] | key set | key clear | list | remove`, `agent models refresh | list`
- Tables: `providers`, `models`, `model_reasoning`, `model_images`, `settings`, `utility_model`

## Utility model

One-shot generation: one request, no tools, no memory, no session.

- Code: `crates/harness/src/utility.rs` (`Utility::generate`, `decide_respond`, `parse_respond`, `parse_inline`)
- Used by the shared pre-messenger **Auto-respond** gate (`settings.auto_respond`, default ON when a utility model is configured; toggled on the **General** tab at `/settings/general` or via `agent auto-respond show | on | off`; conservative in channels, more responsive in threads, silently drops `IGNORE` before typing/reactions; caches recent history in memory for actively participating channels/threads with 30m/2h TTL to avoid repeated Discord REST calls; both the addressed fast-lane and the unaddressed triage lane deliver the first message immediately when idle, and while a turn is active or work is queued for that place debounce bursts per place with a 1.5s quiet period, 6s max wait, and 10-message batch cap, emitting each admitted message unmerged with its own `reply`/`to` tokens), the reply-mode gate, and thread titles; without a utility model, all three are off

## Messengers (Discord)

- Code: `crates/messenger/src/lib.rs`, `crates/messenger-discord/src/{lib,gateway,render}.rs`, `crates/harness/src/messenger.rs`, `crates/agent/src/run.rs` (`serve`, `admit`)
- Gate: access mode (`whitelist` default, `blacklist`, or `allow-all`) with per-user entries scoped to `global` (including DMs), `guild` (server), or `channel` (including threads); existing `allowed_users` migrate into the global whitelist. Interaction modes then apply per admitted user — a thread-level `channel` override wins, then the channel's, then the author's guild-scoped `user` override, then the `server` override for the guild (its target id IS the guild id; `user` overrides require `--guild`, `channel`/`server` forbid it), then the Discord-wide default (`auto` unless changed): `always` admits unaddressed regular/reply messages (marked `unaddressed="true"` on `<message>` so the model judges whether to reply or stay silent with `NO_UPDATES`), `mention-only` drops them, `auto` runs the Auto-respond check and falls back to `mention-only` when no utility model exists or Auto-respond is off. Changes apply live, and a store-backed pre-filter caches channels/threads and drops non-admissible messages before any Discord REST call
- Reply-mode gate: defaults to the Discord-wide `defaultReplyMode` (`auto` unless changed; with a utility model, a plain-channel message is answered inline or in a thread with a 5-second budget, fallback thread); overrides (`auto`, `thread`, `direct`) live in `reply_modes` scoped to `channel` or `server` (guild id as target) and apply live — a `channel` override wins, then the `server` override for the message's guild, then the default (`direct` forces inline replies and suppresses new thread creation on both `reply` and `to` sends while keeping posts to existing threads working; `thread` always starts a thread and skips the inline-versus-thread decision)
- Threads: when reply mode resolves to a thread, a reply to a plain-channel message starts a thread from it; bot-created threads get a generated title via the utility model when configured
- Pings: one ping per message per turn; follow-ups in a bot run post plainly
- Acknowledgment: 👀 on admit for addressed messages (skipped for unaddressed group-chat messages so silent `NO_UPDATES` turns stay quiet); ⏳ while a `for` task runs, then ✅ or ⚠️
- Typing: pulses every 8 seconds in the conversation while a message is unanswered (up to 10 minutes) and while the bot answers a task report
- References: replied-to message as `<quote>`; attachments as `<attachment>`
- Tools: `send` (chat only), `history`, `search`, `participants`, `message` (chat and tasks)
- The bot token lives in `messenger_tokens`, not the environment: `agent messenger token set discord` reads it from stdin and validates it against `GET /users/@me` before storing; `agent messenger token clear discord` removes it; the **Chat platforms** page does the same over `discord.setToken` / `discord.clearToken`. `serve` polls the row and connects, swaps or drops the gateway live — no restart; with no token sends fail with `Discord is not connected`; a token with no main model reports `Waiting for a main model`; a fatal gateway close (e.g. rejected token, missing intents) reports an error state and stays there until the token changes or is saved again from the web, instead of exiting the process; schedules and check-ins pause whenever there is no main model or the bot is offline (jobs due meanwhile fire late once it is back); shell commands and stdio MCP servers never inherit `BLOB_1_APP_KEY`
- Web protocol: `discord.get`, `discord.allow`, `discord.disallow`, `discord.setAccessMode`, `discord.addAccessEntry`, `discord.removeAccessEntry`, `discord.setHome`, `discord.directory` (`guilds`, `channels`, `people`; empty lists while disconnected), `discord.setToken`, `discord.clearToken`, `discord.setInteractionMode`, `discord.clearInteractionMode`, `discord.setDefaultInteractionMode`, `discord.setReplyMode`, `discord.clearReplyMode`, `discord.setDefaultReplyMode`; `Discord` replies carry `token: "set"|"missing"` and `connection` (`off`, `connecting`, `online` + bot name, `error` + message) alongside `allowed`, `accessMode`, `accessEntries`, `home`, `interactionModes` (`channel`, `user`, `server` scopes; `mention-only`, `always`, `auto`; `guildId` only for `user` overrides), `replyModes` (`channel`, `server` scopes; `auto`, `thread`, `direct`), `defaultInteractionMode` and `defaultReplyMode`
- CLI: `agent serve` (needs `BLOB_1_APP_KEY`), `agent messenger allow | disallow | list`, `agent messenger access show <messenger>`, `agent messenger access mode <messenger> <allow-all|whitelist|blacklist>`, `agent messenger access add | remove <messenger> <whitelist|blacklist> <user_id> [--guild <guild_id> | --channel <channel_id>]`, `agent messenger token set | clear <messenger>`, `agent messenger mode list <messenger>` (prints the default plus overrides), `agent messenger mode default <messenger> [<mention-only|always|auto>]`, `agent messenger mode set <messenger> <channel|user|server> <id> <mention-only|always|auto> [--guild <guild_id>]`, `agent messenger mode clear <messenger> <channel|user|server> <id> [--guild <guild_id>]`, `agent messenger reply list <messenger>` (prints the default plus overrides), `agent messenger reply default <messenger> [<auto|thread|direct>]`, `agent messenger reply set <messenger> <channel|server> <id> <auto|thread|direct>`, `agent messenger reply clear <messenger> <channel|server> <id>`
- Tables: `destinations`, `allowed_users`, `access_modes`, `access_entries`, `messenger_tokens`, `interaction_modes`, `messenger_defaults`, `reply_modes`, `pending_threads`, `seen_threads`

## Calendar

- Code: `crates/harness/src/schedule.rs` (`run_schedules`), `crates/storage-sqlite/src/schedule.rs`, `apps/web/src/features/schedules/`
- One-off (`at`) or recurring (`cron`) jobs with their own timezone and optional `to` or `reply`
- Fires as `<scheduled id due [late] [to] [for] [home]>` while `serve` runs; a missed job fires once, marked late
- Tools: `schedule`, `unschedule`, `schedules`
- Web protocol: `schedule.get`, `schedule.occurrences`, `schedule.setTimezone`, `schedule.add`, `schedule.remove` (`/settings/schedules`)
- CLI: `agent schedule add | list | remove`, `agent timezone show | set`, `agent home show | set | clear`
- Tables: `schedules`, `clock`, `home`

## Watches and check-ins

- Code: `crates/harness/src/watch.rs`, `crates/harness/src/schedule.rs` (`next_checkin`), `crates/storage-sqlite/src/{watch,checkin}.rs`, `apps/web/src/features/schedules/`
- Watches: things the bot keeps an eye on, with optional `to`, `reply` and `until`
- Check-ins: every interval within an hour window on chosen weekdays, only when watches exist; fire as `<checkin time [home]>` with a `<watch>` per item
- Results with no destination and no home go in the final reply, kept in memory and printed by `serve`
- Tools: `watch`, `unwatch`, `watches`
- Web protocol: `checkin.set`, `checkin.clear`, `watch.add`, `watch.remove` (`/settings/schedules`)
- CLI: `agent watch list | remove`, `agent checkin show | set --every --days --hours | off`
- Tables: `watches`, `checkin`

## Skills

- Code: `crates/environment-local/src/lib.rs` (sync), `crates/storage-sqlite/src/skill.rs`, `crates/agent/src/import.rs`
- Stored in the database, synced to the skills directory with a `.managed.json` manifest; tasks load them
- CLI: `agent skill add | list | remove`
- Tables: `skills`, `skill_files`

## MCP

Blob-1 is an MCP client: chat and background tasks can call tools, read resources and use prompts from configured servers.

- Code: `crates/mcp/src/{lib,oauth,tools}.rs`, `crates/storage-sqlite/src/mcp.rs`, `crates/agent/src/web.rs` (`/api/mcp/login`, `/api/mcp/callback`), `apps/web/src/features/mcp/`
- Servers are stdio (command, args, env) or Streamable HTTP (url, static headers whose values are env var names); no legacy SSE
- One shared `mcp::Mcp` per `serve` keeps one connection per server for chat and tasks; config changes apply without a restart
- Server ids are lowercase letters, digits and `-`
- Connect waits up to 30s; each MCP request times out at 60s (tool calls 5 min). A dead, timed-out or failed transport drops the connection so the next call reconnects; in-flight calls keep their own handle — disabling or removing a server lets running calls finish — and a dropped `tools/call` reports its outcome as unknown rather than retrying. The `mcp.list` status probe never drops a connection on timeout — it reports the last known tool count instead. Stdio servers run in their own process group, killed explicitly whenever the connection closes even if the leader already exited, so wrappers like `npx` can't strand grandchildren; `serve`/`chat`/`mcp tools` close and await them on exit
- OAuth 2.1: protected-resource and authorization-server metadata, dynamic client registration, PKCE, refresh; `agent mcp login <id>` or the settings page opens the flow — fetching the `WWW-Authenticate` challenge with an initialize POST when no prior connect captured one — `GET /api/mcp/callback` finishes it; tokens and client registration (including the discovery challenge, used to rebuild the client for refresh) persist. Enable/disable keeps them; replacing a server's transport config clears its OAuth data; the registration travels with the pending login state so concurrent logins can't clobber each other
- Tools: static discovery and execution — `mcp_search` returns matching servers, tool names, descriptions and JSON input schemas without mutating the `tools` list (preserving prompt-cache prefixes), `mcp_execute` runs `server` + `tool` with a JSON `arguments` object, plus `mcp_resources`, `mcp_read`, `mcp_prompts`, `mcp_prompt`; the client does not advertise sampling, roots or elicitation and ignores logging and progress notifications
- Servers connect on first use and are probed when added, enabled, or listed from Settings (`mcp.add`, `mcp.setEnabled`, `mcp.list`, obeying retry backoff); statuses are `connected`, `connecting` (a connect attempt is in flight), `idle` (Standby), `needs-login` (Needs sign-in), `error` (with text) and `disconnected` (disabled)
- Web protocol: `mcp.list`, `mcp.add`, `mcp.remove`, `mcp.setEnabled`, `mcp.login`
- CLI: `agent mcp list`, `agent mcp add <id> [--env K=V] -- <command> [args]`, `agent mcp add <id> --url <url> [--header 'Name: ENV_VAR']`, `agent mcp remove <id>`, `agent mcp enable | disable <id>`, `agent mcp login <id>` (prints the stored `--public-url` base when `serve` set one, else `--web`), `agent mcp tools <id>`
- Tables: `mcp_servers`, `mcp_tokens`, `mcp_oauth_states`

## Secrets at rest

- Code: `crates/storage-sqlite/src/crypto.rs`; `SqliteStorage::open`/`open_shared` take `Option<AppKey>`
- `BLOB_1_APP_KEY` is any non-empty passphrase; `serve` requires it. Argon2id derives the 256-bit key once per open over a random 16-byte salt stored in `app_key (id=1)`, whose `canary` decrypts to a fixed value — a wrong key fails with `BLOB_1_APP_KEY does not match this database`. Secret reads and writes without a key fail with `set BLOB_1_APP_KEY`
- Values are XChaCha20-Poly1305 with a random 24-byte nonce, stored as `enc:v1:<base64(nonce‖ciphertext)>`; encryption stays inside the storage layer so callers keep plain types. Plaintext rows seal idempotently on the first keyed open; key rotation is not supported yet
- Encrypted columns: `providers.api_key`, `mcp_servers.env`, `mcp_servers.headers`, `mcp_tokens.credentials`, `mcp_tokens.registration`, `mcp_oauth_states.state`, `messenger_tokens.token` (empty JSON maps stay plaintext). Web password/session hashes are not encrypted
- Tables: `app_key`, `secret_scrub` (set while freed plaintext may still sit in the WAL; every keyed open retries a truncating checkpoint until it succeeds)

## Releases and self-update

- Code: `crates/agent/src/runtime.rs`, `scripts/install.sh`, `.github/workflows/release.yml`
- The workspace version is `0.0.0`; release builds stamp `BLOB_1_VERSION` so `blob-1 --version` reports the tag. Channels: `latest` ships `X.Y.Z-latest.YYYYMMDD.HHMM` (tag `vX.Y.Z-latest.YYYYMMDD.HHMM`, published with `--prerelease --latest=false`), `stable` ships plain `X.Y.Z` (published `--latest`). The release workflow builds linux-x64 and darwin-arm64 on tag pushes (`v*`) or `workflow_dispatch` (channel `latest`/`stable`, optional explicit `version`)
- `scripts/install.sh` (`curl | sh`) installs into `$BLOB_1_HOME/runtime` (default `~/.blob-1/runtime`): `versions/<version>/blob-1` + `.install-complete`, a `~/.local/bin/blob-1` symlink, and the `runtime/launchers` list. Env: `BLOB_1_CHANNEL` (`latest` default, `stable`), `BLOB_1_VERSION`, `BLOB_1_HOME`, `BLOB_1_INSTALL_BIN_DIR`, `BLOB_1_RELEASE_BASE_URL`
- `blob-1 update [VERSION] [--channel latest|stable] [--allow-downgrade] [--no-restart]` downloads `SHA256SUMS` + `blob-1-<version>-<platform>.tar.gz` into `versions/.staging-*`, verifies the checksum, smoke-tests `--version` and renames into place. With a launcher on `runtime/launcher.sock` it sends `{"type":"apply-update","targetVersion","allowDowngrade"}` and reports the outcome; otherwise it repoints launcher symlinks and prunes old versions. `GH_TOKEN`/`GITHUB_TOKEN` authenticate the releases API
- `blob-1 launcher [--web --public-url]` (inherits `--data-dir`, `--workspace`, `skills-dir`) supervises `serve`: it runs `versions/<activeVersion>/blob-1 serve` (falling back to the current exe) with `BLOB_1_READY_FILE`; `serve` writes `ready\n` there once the database and web listener are up. On `apply-update` it persists a `pending` update in `runtime/service-state.json`, stops the child, snapshots `agent.db` (+ `-wal`/`-shm`) into `runtime/db-backup/<id>/`, trials the target, then commits (new `activeVersion`, repoint launchers, prune) or rolls back (restore db, restart the old version). A `pending` record at startup is recovered the same way. SIGTERM/SIGINT stop the child and remove `launcher.sock`
- Upgrades must move forward; a dev `0.0.0` build may adopt its first release without `--allow-downgrade`

## Dev server

- Files: `Dockerfile`, `compose.yaml`
- `docker compose up -d --build` runs `agent serve` with host networking and the `agent-data` volume; `CPA1_KEY` and `BLOB_1_APP_KEY` come from the shell
- The runtime stage is `ubuntu:24.04` with the utility packages plus Node.js LTS (`npm`, `npx`, `corepack`) and Python 3 with `uv`/`uvx`, so stdio MCP servers can run inside the container
