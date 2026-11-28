import { Api } from '@blob-1/client/api'
import { BotSkillsState, GithubSkillsPreview } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

const previewGithubSkills = Effect.fn('GithubImport.preview')(function* (input: {
  readonly url: string
  readonly path: string
}) {
  const api = yield* Api
  return yield* api.request('bot.skills.githubPreview', input)
})

const importGithubSkills = Effect.fn('GithubImport.import')(function* (input: {
  readonly repo: string
  readonly commit: string
  readonly folders: ReadonlyArray<string>
}) {
  const api = yield* Api
  return yield* api.request('bot.skills.githubImport', input)
})

export const GithubImportStates = Machine.state({
  states: {
    Closed: {},
    Editing: { fields: { error: Schema.NullOr(Schema.String) } },
    Previewing: { fields: { url: Schema.String, path: Schema.String } },
    Choosing: {
      fields: { preview: GithubSkillsPreview, error: Schema.NullOr(Schema.String) },
    },
    Importing: {
      fields: { preview: GithubSkillsPreview, folders: Schema.Array(Schema.String) },
    },
    Imported: { fields: { skills: BotSkillsState } },
  },
})

export const GithubImportEvents = Machine.events({
  Open: {},
  Close: {},
  Preview: { url: Schema.String, path: Schema.String },
  Back: {},
  Import: { folders: Schema.Array(Schema.String) },
  ClearError: {},
})

/** Two-step GitHub skill import: preview a repository path, then import selected skill folders. */
export const GithubImportMachine = Machine.make({
  id: 'GithubImport',
  root: GithubImportStates,
  events: GithubImportEvents,
  effects: { previewGithubSkills, importGithubSkills },
}).handle({
  initial: { target: 'Closed' },
  states: {
    Closed: {
      on: {
        Open: { target: 'Editing', data: { error: null } },
      },
    },
    Editing: {
      on: {
        Close: { target: 'Closed' },
        ClearError: { update: 'Editing', data: { error: null } },
        Preview: {
          target: 'Previewing',
          guard: ({ event }) => event.url.trim() !== '',
          data: ({ event }) => ({ url: event.url.trim(), path: event.path.trim() }),
        },
      },
    },
    Previewing: {
      on: {
        Close: { target: 'Closed' },
      },
      invoke: {
        src: 'previewGithubSkills',
        input: ({ state }) => ({ url: state.url, path: state.path }),
        onDone: {
          target: 'Choosing',
          data: ({ output }) => ({ preview: output, error: null }),
        },
        onFailure: {
          target: 'Editing',
          data: ({ error }) => ({ error: error.message }),
        },
      },
    },
    Choosing: {
      on: {
        Close: { target: 'Closed' },
        Back: { target: 'Editing', data: { error: null } },
        ClearError: {
          update: 'Choosing',
          data: ({ state }) => ({ preview: state.preview, error: null }),
        },
        Import: {
          target: 'Importing',
          guard: ({ event }) => event.folders.length > 0,
          data: ({ state, event }) => ({ preview: state.preview, folders: event.folders }),
        },
      },
    },
    Importing: {
      on: {
        Close: { target: 'Closed' },
      },
      invoke: {
        src: 'importGithubSkills',
        input: ({ state }) => ({
          repo: state.preview.repo,
          commit: state.preview.commit,
          folders: state.folders,
        }),
        onDone: {
          target: 'Imported',
          data: ({ output }) => ({ skills: output }),
        },
        onFailure: {
          target: 'Choosing',
          data: ({ state, error }) => ({ preview: state.preview, error: error.message }),
        },
      },
    },
    Imported: {
      on: {
        Close: { target: 'Closed' },
      },
    },
  },
})
