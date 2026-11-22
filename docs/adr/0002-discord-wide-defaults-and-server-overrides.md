---
status: accepted
---

# Discord-wide defaults and server overrides

Blob-1 now stores a Discord-wide `messenger_defaults` row (interaction `auto`, reply `auto` out of the box) and a `server` scope on both interaction and reply overrides, so operators can shift the baseline without editing every channel.

Before this, unaddressed messages with no matching override were implicitly auto-triaged and replies always fell back to `auto` (then called `decision`); changing that baseline meant per-channel `mention-only` rows everywhere or no utility model at all. Two gaps pushed the change:

- **Servers that should mostly ignore the bot.** A whole guild can now be set to `mention-only` or `always` with one `server` override instead of a `channel` row per channel, and new channels inherit it automatically. Reply overrides follow the same shape: a `server` row sets `auto`, `thread` or `direct` for every channel in the guild.
- **Baselines that differ per deployment.** A quiet bot can flip the Discord-wide interaction default to `mention-only` and keep `auto` or `always` only where overrides say so; a chatty bot can flip the reply default to `thread` or `direct` once.

`auto` is now an explicit mode, not just the absence of an override: it names "run the Auto-respond check here" so it can sit anywhere in the precedence chain (`thread > channel > user > server > default` for interaction; `channel > server > default` for reply). When Auto-respond is disabled or no utility model is configured, `auto` behaves like `mention-only` — `settings.auto_respond` stays the triage gate only and never gates the utility model's other jobs (reply-mode decisions, thread titles). The server override's target id is the guild id itself, so it takes no `--guild`.

Reply mode `decision` was renamed to `auto` to match the interaction vocabulary: it means "the utility model decides" in both places, inline-versus-thread for replies and reply-versus-silent for interaction. The migration rewrites stored `decision` values, and nothing accepts the old name.

## Considered options

- **Keep `auto` implicit and add only the `server` scope.** Rejected: without a selectable `auto`, a `mention-only` or `always` default could not be punched through for a single channel except by copying the triage behavior into an `always`-style hack; an explicit mode keeps the chain uniform.
- **Server overrides keyed by `--guild` like user overrides.** Rejected: the override target already is the guild, so a separate guild column would either duplicate the id or sit unused; keying by `target_id` keeps `user` the only scope that needs `--guild`.
- **Reply default via a synthetic channel id in `reply_modes`.** Rejected: a magic channel id collides with real ids and hides in `mode list`; a dedicated `messenger_defaults` table makes the two defaults explicit and leaves room for future per-messenger settings.
- **Keep `decision` as an accepted alias for reply `auto`.** Rejected: no production data depends on the string beyond the migration, and two names for one mode invite drift between CLI, protocol and storage.
