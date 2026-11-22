---
status: accepted
---

# Keep the always override and the Auto-respond toggle

Blob-1 keeps the per-channel and per-user `always` interaction override alongside the bot-wide `settings.auto_respond` toggle (on by default when a utility model is set).

Unaddressed messages without an override go through `harness::decide_respond` on the utility model, which avoids waking the main model in shared channels. Two real cases need exits from that default:

- **Dedicated bot channels.** In a private channel used only by one person and Blob-1, every message, including `"ok"` or a bare screenshot, is meant for the bot. Routing those messages through the utility model adds latency on every turn and risks false `IGNORE` drops. An explicit `always` override skips the utility gate and sends every message in that channel straight to the main model.
- **Mention-only servers with a utility model.** You can configure a utility model for thread titles and inline or thread reply decisions without wanting the bot to auto-respond across every channel. The bot-wide `settings.auto_respond` toggle disables unaddressed triage globally without removing the utility model or adding `mention-only` overrides to every channel.

## Considered options

- **Drop `always` and leave only `mention-only` and default auto-respond.** Simpler override list, because `always` wastes main-model tokens in multi-person channels. Rejected because dedicated single-user channels still need a zero-triage path that never drops short confirmations or bare attachments.
- **Make auto-respond always active whenever a utility model is configured.** Matches how thread titles and `ReplyMode::Decision` work, with one fewer setting in **General**. Rejected because turning off auto-respond would then require removing the utility model (losing titles and reply-mode decisions) or adding `mention-only` overrides channel by channel.
