# AGENTS.md

## Feature map

`FEATURES.md` maps every feature to its code, model tools, CLI commands and tables. Update it in the same commit whenever a change adds, removes, renames or moves any of these:

- a crate or source file that holds a feature
- a chat or task tool
- an `agent` CLI command or flag
- a database table
- a model input element (`<message>`, `<report>`, `<reminder>`, `<scheduled>`, `<checkin>`, and their children)
- a behavior described in the map

## Web workspace

The web settings app is a pnpm workspace beside the Rust crates. Tooling is vite-plus (`vp`): oxfmt, oxlint with the unslop plugins in `tools/oxlint`, and `effect-tsgo` (TypeScript 7) for type checks.

- `apps/web`: React 19, TanStack Router (file routes in `src/routes`), Tailwind 4, shadcn on Base UI (`src/components/ui`), Effect, and `@typeonce/effect-machine` for page workflows.
- `packages/contracts`: Effect Schemas for the WebSocket protocol. The Rust code in `crates/agent/src/web.rs` is the source of truth; keep both in sync by hand.
- `packages/client`: the API client (WebSocket transport, request/reply, session, mock). Import subpaths: `@blob-1/client/api`, `/ws`, `/session`, `/mock`.
- `crates/agent` embeds `apps/web/dist`; the Dockerfile builds it.

Commands, from the repo root: `pnpm install`, `pnpm dev` (proxies `/api` and `/ws` to `127.0.0.1:7878`; `VITE_MOCK=1 pnpm dev` uses the in-memory mock, password `blob`), `pnpm check` (format check, lint with warnings as errors, typecheck), `pnpm fmt`, `pnpm build`.

### Folder structure

- Keep `src/routes` thin: route options and one feature component. Features live in `src/features/<feature>`.
- A feature holds its machine (`<name>Machine.ts`, React-free), its data atoms (`<name>State.ts`), and its components.
- Shared layout lives in `src/components/layout` (`Page`, `Section`, `FieldRow`, `EmptyLine`, `SplitPage`, `Shell`). Shared widgets live in `src/components` (`ConfirmDialog`, `Picker`).
- Keep generated shadcn primitives in `src/components/ui` in their upstream style. Change radii there so every page follows: pills (`rounded-full`) for buttons, inputs and triggers; `rounded-xl` for menus and popovers with `rounded-lg` items; `rounded-2xl` for dialogs and multi-line text areas. Selection checkmarks sit at the end of a row.

### React and state

- Declare components as named functions with a named props interface; prefer named exports.
- Load page data with an atom from `AppRuntime` (`src/lib/runtime.ts`); it reloads each time the page mounts.
- Model workflows (save, remove, polling) with an effect-machine owned by `createMachineContext` and `MachineAtoms`. Pass loaded data as machine input. Render state with `MachineState`; send events with `useAtomSet(machine.send)`.
- Never mirror machine state into `useState` or sync it with `useEffect`. Plain form drafts and UI toggles stay in local state.
- Report background failures with the `Notify` service; show field errors inline.
- No native `title` tooltips; use the `Tooltip` component.
