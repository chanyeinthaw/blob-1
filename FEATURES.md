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

- Code: `crates/agent/src/web.rs`, `crates/storage-sqlite/src/web_auth.rs`, `packages/contracts/src/protocol.ts` (wire schemas), `packages/client/src/{api,ws,session,mock}.ts` (API client), `apps/web/src/routes/*`, `apps/web/src/lib/{session.ts,shellMachines.tsx,useBackgroundRefresh.ts}` (cached session guard, shell-scoped persistent page machines, stale-while-revalidate rendering), and `apps/web/src/features/{session,general,schedules,models,providers,chat,mcp,webhooks}/` (pages)
- Settings navigation is instant: once signed in, `requireSession` returns from an in-memory cache (cleared on sign-out or session expiry), `Shell` prefetches all 6 `Atom.keepAlive` settings page atoms, `ShellMachinesProvider` keeps page machines alive across tab switches so pending save queues never cancel on navigation, `matchCached` keeps cached pages visible if a background refresh fails, and toggles/pickers update optimistically with coalesced background saves.
- The app is a pnpm workspace (vite-plus, React, TanStack Router, shadcn on Base UI, Effect, effect-machine) that `agent` embeds from `apps/web/dist`; `VITE_MOCK=1 pnpm dev` runs it against an in-memory mock (password `blob`)
- `agent serve --web <addr>` serves the app (default `127.0.0.1:7878`); `agent web password set` sets the password.
- `agent serve --public-url <url>` sets the absolute http(s) URL the app is publicly reached at (e.g. `https://silicon.tailcd7ccb.ts.net:7878` via Tailscale Serve proxying to `--web 127.0.0.1:7878`): the OAuth redirect URI and `mcp.login` URL use it instead of the Host header, the WebSocket Origin check accepts exactly that origin, and the session cookie gets `Secure` when it is https. `serve` persists it so `agent mcp login` prints the same URL; running `serve` without it clears the stored value.
- The session cookie is `HttpOnly; SameSite=Lax` so the cross-site OAuth callback's top-level GET still carries it.
- HTTP endpoints: `/api/session`, `/api/login`, `/api/logout`, `/api/mcp/login`, `/api/mcp/callback`, `/ws`, `/p/{id}`, `/p/{id}/{name}` (public shared workspace files, unauthenticated capability URLs, served with `Content-Security-Policy: sandbox allow-scripts` and `X-Content-Type-Options: nosniff`; Markdown served inline as `text/plain; charset=utf-8`), `POST /hooks/{token}` (inbound capability-token webhooks capped at 64 KiB; atomically updates `last_triggered_at` and enqueues `<event source="webhook" name="...">` into durable `pending_events` before returning `202 Accepted`, then wakes the owning bot's chat loop), `POST /hooks/cb/{token}` (ephemeral task-created callback webhooks capped at 64 KiB; atomically decrements `remaining_uses` and enqueues the wake event into durable `pending_events` in the same SQLite transaction before returning `202 Accepted`, then wakes the chat loop to deliver `<event source="callback" name="..."[ for="..."]>`).
- Tables: `web_password`, `web_sessions`, `public_url`, `public_files`, `usage_events`, `webhooks`, `callbacks`, `pending_events`.
- Usage telemetry: every completion in `run.rs` records `usage_events` (`chat`, `task`, `utility`, `compaction`, provider, model, input/cached/output tokens); surfaced via `usage.get` on each bot's **General** page (`/settings/bots/$botId/general`).
- Webhooks: `/settings/bots/$botId/webhooks` (`webhooks.list`, `webhooks.create`, `webhooks.setEnabled`, `webhooks.remove`) manages capability-token inbound webhooks; tokens are encrypted at rest in `webhooks.token` with SHA-256 `token_hash` lookup.
- Discord settings live under each bot's **Platforms → Discord** page at `/settings/bots/$botId/discord` (legacy `/settings/chat/discord` redirects to `default`): token Replace/Remove, connection status (`off` / `connecting` / `online` with the bot name / `error` with a short message), unified access rules, Discord-wide interaction and reply defaults, per-channel, per-person and per-server interaction-mode overrides and the home channel or DM. Discord is the only chat platform today.
- Model and provider changes apply to the running bot without a restart: each turn, spawned task, compaction pass and utility call resolves from current settings; a run already in progress keeps its model, a task keeps the model it was spawned with for its lifetime, and a role whose settings no longer resolve keeps its last working model. The `model`, `models`, `provider` and `reasoning` commands run while `serve` holds the data directory; `serve`, `chat` and `skill` still need it exclusively.
- The mock transport is enabled only with `VITE_MOCK=1`.

## Chat

The bot is one chat that never ends. It coordinates and talks; tasks do the work.

- Code: `crates/harness/src/chat.rs` (`ChatHarness`, `CHAT_PROMPT`, `MESSENGER_PROMPT`), `crates/harness/src/prompt.rs`
- Runs: a run ends when the agent is done. Task reports and reminders steer a live run or wake an idle agent (`resume`).
- Input: every outside input is one XML element with escaped text: `<message>` (carries `time="Day YYYY-MM-DD HH:MM <TZ>"` in `clock.timezone`, falling back to `UTC`, both on live arrival and `zoom(id, 1)`), `<report>`, `<reminder>`, `<scheduled>`, `<checkin>`, `<event>` (also carries `time="..."`)
- Final reply `NO_UPDATES` ends a messenger turn silently.
- Tools: `zoom`, `date`, `spawn`, `models`, `tasks`, `tell`, `stop`, `send` (when messengers are configured), plus calendar (`schedule`, `unschedule`, `schedules`) and watch (`watch`, `unwatch`, `watches`) tools; connected MCP servers are listed in the chat prompt so chat can delegate them to background tasks
- Prompt caching: `ChatHarness` and `SessionHarness` pass a stable per-workspace/session `prompt_cache_key` on OpenAI Responses (`llm-rig`), and keeping MCP behind static `mcp_search` + `mcp_execute` in tasks avoids mutating the `tools` list mid-turn.
- CLI: `agent chat <message>` (one message, then waits for its tasks to report)

## Instructions

- Code: `crates/harness/src/chat.rs` (`DEFAULT_INSTRUCTIONS`)
- Prompt: stored instructions text verbatim followed by `These instructions are current and override anything conflicting in your memory.` at the top of the chat system prompt, default `# Identity\n\nYou are Blob-1`, read every turn; also passed as `Assistant instructions:` snippet to the Auto-respond utility model
- CLI: `agent instructions show | set <file|-> | reset`
- Table: `instructions` (legacy `identity` rows migrate verbatim on startup and drop `identity`)

## Memory

- Code: `crates/memory/src/lib.rs`, `crates/storage-sqlite/src/memory.rs`
- Append-only rows with JSON `meta`; binary summary tree; chat view (128,000 bytes) and seen view (32,000 bytes); 512-byte nodes; `zoom`, `date`
- Compaction model summarizes in the background
- Tables: `memory_messages`, `memory_nodes`, `memory_views`, `memory_view_parts`

## Tasks

Background workers spawned by the chat.

- Code: `crates/harness/src/tasks.rs`, `crates/harness/src/session.rs`, `crates/harness/src/compaction.rs`, `crates/harness/src/share.rs`, `crates/harness/src/callback.rs`
- Tools: `read`, `write`, `edit`, `bash`, read-only `zoom` and `date`, read-only `history`, `search`, `participants`, `message` (never `send`), `share` (creates public capability links in `public_files` for workspace files or inline Markdown/HTML under `workspace/shares/`), `callback` (creates temporary expiring one-shot/multi-use `POST /hooks/cb/{token}` wake URLs in `callbacks` with optional `for` reply/destination token; requires `--public-url`), plus static MCP tools (`mcp_search`, `mcp_execute`, `mcp_resources`, `mcp_read`, `mcp_prompts`, `mcp_prompt`)
- `read`, `write` and `edit` resolve paths against the task's workspace root but are not restricted to it: `..` and absolute paths reach anywhere the process can
- Reports: `<report task status [for]>`; a 15-minute watch per run sends `<reminder task running last_activity [for]>`
- `spawn` with `for: "m…"` links a task to a messenger message
- Images: `read` and MCP tool results return png, jpg, gif and webp files as images on models with image input
- Tables: `sessions`, `messages`, `message_images`, `task_choices`

## Models

- Code: `crates/agent/src/run.rs` (`setup`, `chat_harness`, `ModelFactory`, `RecordedDecision`), `crates/llm/src/lib.rs` (`DecisionModel`), `crates/llm-rig/src/{lib,decision}.rs`
- Main model: `agent model set <provider>/<model>`, `agent reasoning set <level>` (LLM providers only)
- Compaction model: `agent model compaction <provider>/<model> [--reasoning]` (LLM providers only)
- Utility model: `agent model utility <provider>/<model> [--reasoning] | off` (LLM providers only)
- Decision model: `agent model decision <provider>/<model> | off` (`decision` providers only; optional fast classifier slot with automatic fallback to `utility`)
- Per model: `agent model map`, `agent model context`, `agent model input <provider/model|provider> <text|image>`
- Providers: `agent provider add --api <responses|completions|decision> [--no-key] [--model <id>...] | key set | key clear | list | remove`, `agent models refresh | list` (`decision` providers use the generic System One / Jev wire format, auto-seed `@cf/cloudflare/clef-flash` and `@cf/cloudflare/clef` for Cloudflare Workers AI `/ai/run` URLs, and require at least one explicit `--model <id>` / model ID for non-Cloudflare decision endpoints; refresh preserves explicitly registered decision model IDs)
- Tables: `providers`, `models`, `model_reasoning`, `model_images`, `settings`, `utility_model`, `decision_model`

## Utility & Decision models

One-shot generation and fast classification: one request, no tools, no memory, no session.

- Code: `crates/harness/src/utility.rs` (`Utility::from_parts`, `Utility::generate`, `Utility::choose`, `decide_respond`, `decide_inline`, `parse_respond`, `parse_inline`)
- Used by the shared pre-messenger **Auto-respond** gate (`settings.auto_respond`, default ON when a decision or utility model is configured; toggled on the **General** tab at `/settings/general` or via `agent auto-respond show | on | off`; conservative in channels with `P(RESPOND) >= 0.65` on decision models, more responsive in threads with `P(RESPOND) >= 0.50`, silently drops `IGNORE` before typing/reactions; caches recent history in memory for actively participating channels/threads with 30m/2h TTL to avoid repeated Discord REST calls; both the addressed fast-lane and the unaddressed triage lane deliver the first message immediately when idle, and while a turn is active or work is queued for that place debounce bursts per place with a 1.5s quiet period, 6s max wait, and 10-message batch cap, emitting each admitted message unmerged with its own `reply`/`to` tokens), the reply-mode gate (`decide_inline`), and thread titles (Utility LLM). Auto-respond and reply-mode prefer the **Decision** model when configured and fall back to **Utility** if unset or on error; without either model, Auto-respond and inline-vs-thread classification are off.

## Messengers (Discord)

- Code: `crates/messenger/src/lib.rs`, `crates/messenger-discord/src/{lib,gateway,render}.rs`, `crates/harness/src/messenger.rs`, `crates/agent/src/run.rs` (`serve`, `admit`)
- Gate: unified `Default + Allow/Block rules` access gate (`access_defaults`, `access_rules`), evaluated in the gateway pre-filter before any Discord REST call and re-checked at admission:
  - **Default**: `block` (default on fresh installs) or `allow`.
  - **Rules**: unique per `(user_id, scope, scope_id)` where `user_id` is a specific person or `None` (`everyone`), `scope` is `global` (all servers + DMs; person-only), `server` (guild channels/threads, never DMs), or `channel` (channel + its threads, never DMs), and `effect` is `allow` or `block`.
  - **Precedence** (first match wins — Person beats Everyone, narrower place beats broader): (1) user in thread → (2) user in parent channel → (3) user in server → (4) user global → (5) everyone in thread → (6) everyone in parent channel → (7) everyone in server → (8) default. DMs only check step 4 then step 8. Uncached threads trigger `NeedParent` only when a higher-precedence `channel`-scoped rule could flip the known decision. Legacy `allowed_users` / `access_modes` / `access_entries` / `channel_access_*` tables migrate fail-closed on startup and are dropped (never widening access; migrating a legacy channel whitelist switches default access to `block` so DMs require an explicit Global person `allow` rule, logged on startup).
  - **Interaction modes** then apply per admitted message — a thread-level `channel` override wins, then the channel's, then the author's guild-scoped `user` override, then the `server` override for the guild (its target id IS the guild id; `user` overrides require `--guild`, `channel`/`server` forbid it), then the Discord-wide default (`auto` unless changed): `always` admits unaddressed regular/reply messages (marked `unaddressed="true"` on `<message>` so the model judges whether to reply or stay silent with `NO_UPDATES`), `mention-only` drops them, `auto` runs the Auto-respond check and falls back to `mention-only` when no utility model exists or Auto-respond is off. Changes apply live.
- Reply-mode gate: defaults to the Discord-wide `defaultReplyMode` (`auto` unless changed; with a utility model, a plain-channel message is answered inline or in a thread with a 5-second budget, fallback thread); overrides (`auto`, `thread`, `direct`) live in `reply_modes` scoped to `channel` or `server` (guild id as target) and apply live — a `channel` override wins, then the `server` override for the message's guild, then the default (`direct` forces inline replies and suppresses new thread creation on both `reply` and `to` sends while keeping posts to existing threads working; `thread` always starts a thread and skips the inline-versus-thread decision)
- Threads: when reply mode resolves to a thread, a reply to a plain-channel message starts a thread from it; bot-created threads get a generated title via the utility model when configured
- Pings: one ping per message per turn; follow-ups in a bot run post plainly
- Acknowledgment: 👀 on admit for addressed messages (skipped for unaddressed group-chat messages so silent `NO_UPDATES` turns stay quiet); ⏳ while a `for` task runs, then ✅ or ⚠️
- Typing: pulses every 8 seconds in the conversation while a message is unanswered (up to 10 minutes) and while the bot answers a task report
- References: replied-to message as `<quote>`; attachments as `<attachment>`
- Tools: `send` (chat only; optional `files` attaches up to 10 workspace files via `PinnedWorkspace`, 10 MiB/file, 25 MiB total), `history`, `search`, `participants`, `message` (tasks only)
- The bot token lives in `messenger_tokens`, not the environment: `agent messenger token set discord` reads it from stdin and validates it against `GET /users/@me` before storing; `agent messenger token clear discord` removes it; the bot's **Platforms → Discord** page does the same over `discord.setToken` / `discord.clearToken`. `serve` polls the row and connects, swaps or drops the gateway live — no restart; with no token sends fail with `Discord is not connected`; a token with no main model reports `Waiting for a main model`; a fatal gateway close (e.g. rejected token, missing intents) reports an error state and stays there until the token changes or is saved again from the web, instead of exiting the process; schedules and check-ins pause whenever there is no main model or the bot is offline (jobs due meanwhile fire late once it is back); shell commands and stdio MCP servers never inherit `BLOB_1_APP_KEY`
- Web protocol: `discord.get`, `discord.allow`, `discord.disallow`, `discord.setDefaultAccess`, `discord.setAccessRule`, `discord.removeAccessRule`, `discord.setHome`, `discord.directory` (`guilds`, `channels`, `people`; empty lists while disconnected), `discord.setToken`, `discord.clearToken`, `discord.setInteractionMode`, `discord.clearInteractionMode`, `discord.setDefaultInteractionMode`, `discord.setReplyMode`, `discord.clearReplyMode`, `discord.setDefaultReplyMode`; `Discord` replies carry `token: "set"|"missing"` and `connection` (`off`, `connecting`, `online` + bot name, `error` + message) alongside `allowed`, `defaultAccess` (`block` | `allow`), `accessRules` (`userId`, `scope`, `scopeId`, `effect`), `home`, `interactionModes` (`channel`, `user`, `server` scopes; `mention-only`, `always`, `auto`; `guildId` only for `user` overrides), `replyModes` (`channel`, `server` scopes; `auto`, `thread`, `direct`), `defaultInteractionMode` and `defaultReplyMode`
- CLI: `agent serve` (needs `BLOB_1_APP_KEY`), `agent messenger allow | disallow | list`, `agent messenger access show <messenger>`, `agent messenger access default <messenger> <block|allow>`, `agent messenger access set <messenger> <allow|block> [--user <user_id>] [--server <server_id> | --channel <channel_id>]`, `agent messenger access remove <messenger> [--user <user_id>] [--server <server_id> | --channel <channel_id>]`, `agent messenger token set | clear <messenger>`, `agent messenger mode list <messenger>` (prints the default plus overrides), `agent messenger mode default <messenger> [<mention-only|always|auto>]`, `agent messenger mode set <messenger> <channel|user|server> <id> <mention-only|always|auto> [--guild <guild_id>]`, `agent messenger mode clear <messenger> <channel|user|server> <id> [--guild <guild_id>]`, `agent messenger reply list <messenger>` (prints the default plus overrides), `agent messenger reply default <messenger> [<auto|thread|direct>]`, `agent messenger reply set <messenger> <channel|server> <id> <auto|thread|direct>`, `agent messenger reply clear <messenger> <channel|server> <id>`
- Tables: `destinations`, `access_defaults`, `access_rules`, `messenger_tokens`, `interaction_modes`, `messenger_defaults`, `reply_modes`, `pending_threads`, `seen_threads`

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

Blob-1 is an MCP client: background tasks can call tools, read resources and use prompts from configured servers, while the chat prompt lists connected servers so chat can delegate work to tasks.

- Code: `crates/mcp/src/{lib,oauth,tools}.rs`, `crates/storage-sqlite/src/mcp.rs`, `crates/agent/src/web.rs` (`/api/mcp/login`, `/api/mcp/callback`), `apps/web/src/features/mcp/`
- Servers are stdio (command, args, env) or Streamable HTTP (url, static headers whose values are env var names); no legacy SSE
- One shared `mcp::Mcp` per `serve` keeps one connection per server across tasks; config changes apply without a restart
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

## Multi-bot

One `agent serve` process can host multiple mutually trusted bots sharing the same global provider credentials, model catalog, and MCP connection pool.

- Code: `crates/storage/src/lib.rs` (`BotStore`), `crates/storage-sqlite/src/bot.rs`, `crates/agent/src/run.rs` (`BotRegistry`), `crates/agent/src/web.rs`, `apps/web/src/features/bots/`, `apps/web/src/components/layout/Shell.tsx`
- The `default` bot (`Blob-1`) uses the root `agent.db` and `workspace/` in place; additional bots live under `<data-dir>/bots/<id>/{agent.db, workspace/}` with separate memory, instructions, schedules, Discord gateways, webhooks, callbacks, and usage telemetry
- Trust boundary: per-bot directories and SQLite files provide logical separation for mutually trusted bots, not OS-level security isolation — every bot's `LocalEnvironment` file and shell tools run with the `agent serve` process's filesystem permissions. Run separate instances or containers for untrusted or OS-isolated bots
- Creating, renaming, pausing/resuming, or deleting a bot applies live without restarting `serve`; the `default` bot cannot be removed. Deleting a bot moves its directory to a generation-tagged `.trash-<id>-<gen>-<nonce>` archive before freeing the slug — the roster-delete failure path restores it (a failed restore leaves the archive for `open_entry` to recover on next start, matched by generation so a recreated slug never picks it up) — so a recreated ID never inherits prior data. Stopping a bot aborts and joins its listener, admission, supervisor, and in-flight memory compaction workers before the bot can reopen
- Per-bot model slots (`Main`, `Compaction`, `Utility`, `Decision`) resolve against the global provider catalog (`bot_models` for non-default bots; `settings`/`utility_model`/`decision_model` for `default`); removing a provider checks every bot's model slots
- Per-bot MCP server toggles (`bot_mcp_disabled`) filter which globally enabled MCP servers each bot's chat prompt and background tasks can access
- Web UI: far-left rail lists each bot's tile (with live connection dot), `+` to create a bot, and global `Providers`, `Model catalog`, and `MCP` icons below a divider; selecting a bot opens a `SplitPage` sidebar with `General`, `Models`, `Schedules`, `Webhooks`, `MCP`, and `Platforms → Discord`
- Web protocol: `bots.list`, `bots.create`, `bots.update`, `bots.remove`, `bot.mcp.get`, `bot.mcp.setEnabled`; all bot-scoped RPCs accept optional `botId` (defaulting to `"default"`). Each bot has a monotonic `generation` (`bot_sequence`) that is never reused for a recreated slug; bot-scoped writes carry `botGeneration` and stale-generation writes are rejected, and the web UI keys retained atoms and persistent machines by `<botId>@<generation>`
- Tables: `bots`, `bot_sequence`, `bot_models`, `bot_mcp_disabled`

## Dev server

- Files: `Dockerfile`, `compose.yaml`
- `docker compose up -d --build` runs `agent serve` with host networking and the `agent-data` volume; `CPA1_KEY` and `BLOB_1_APP_KEY` come from the shell
- The runtime stage is `ubuntu:24.04` with the utility packages plus Node.js LTS (`npm`, `npx`, `corepack`) and Python 3 with `uv`/`uvx`, so stdio MCP servers can run inside the container
