import { Api } from '@blob-1/client/api'
import { BotSkillsState, type RequestPayload } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

const SourceBot = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  generation: Schema.Finite,
})
export type ImportFromBotSource = typeof SourceBot.Type

const loadSourceSkills = Effect.fn('ImportFromBot.load')(function* (source: ImportFromBotSource) {
  const api = yield* Api
  return yield* api.request('bot.skills.list', {
    botId: source.id,
    botGeneration: source.generation,
  })
})

const importFromBot = Effect.fn('ImportFromBot.import')(function* (input: {
  readonly targetBotId: string
  readonly targetBotGeneration: number | null
  readonly fromBotId: string
  readonly fromBotGeneration: number
  readonly names: ReadonlyArray<string>
}) {
  const api = yield* Api
  const basePayload = {
    botId: input.targetBotId,
    fromBotId: input.fromBotId,
    fromBotGeneration: input.fromBotGeneration,
    names: input.names,
  }
  const payload: RequestPayload<'bot.skills.importFromBot'> =
    input.targetBotGeneration === null
      ? basePayload
      : { ...basePayload, botGeneration: input.targetBotGeneration }
  return yield* api.request('bot.skills.importFromBot', payload)
})

export const ImportFromBotInput = Schema.Struct({
  targetBotId: Schema.String,
  targetBotGeneration: Schema.NullOr(Schema.Finite),
})
export type ImportFromBotInput = typeof ImportFromBotInput.Type

export const ImportFromBotStates = Machine.state({
  fields: {
    targetBotId: Schema.String,
    targetBotGeneration: Schema.NullOr(Schema.Finite),
  },
  states: {
    Closed: {},
    Picking: { fields: { error: Schema.NullOr(Schema.String) } },
    Loading: { fields: { source: SourceBot } },
    Choosing: {
      fields: {
        source: SourceBot,
        skills: BotSkillsState,
        error: Schema.NullOr(Schema.String),
      },
    },
    Importing: {
      fields: {
        source: SourceBot,
        skills: BotSkillsState,
        names: Schema.Array(Schema.String),
      },
    },
    Imported: { fields: { skills: BotSkillsState } },
  },
})

export const ImportFromBotEvents = Machine.events({
  Open: {},
  Close: {},
  SelectSource: { id: Schema.String, name: Schema.String, generation: Schema.Finite },
  Import: { names: Schema.Array(Schema.String) },
  ClearError: {},
})

/** Loads another bot's skills, then imports the checked names into this bot. */
export const ImportFromBotMachine = Machine.make({
  id: 'ImportFromBot',
  root: ImportFromBotStates,
  events: ImportFromBotEvents,
  input: ImportFromBotInput,
  effects: { loadSourceSkills, importFromBot },
}).handle({
  root: ({ input }) => ({
    targetBotId: input.targetBotId,
    targetBotGeneration: input.targetBotGeneration,
  }),
  initial: { target: 'Closed' },
  states: {
    Closed: {
      on: {
        Open: { target: 'Picking', data: { error: null } },
      },
    },
    Picking: {
      on: {
        Close: { target: 'Closed' },
        SelectSource: {
          target: 'Loading',
          data: ({ event }) => ({
            source: { id: event.id, name: event.name, generation: event.generation },
          }),
        },
      },
    },
    Loading: {
      on: {
        Close: { target: 'Closed' },
      },
      invoke: {
        src: 'loadSourceSkills',
        input: ({ state }) => state.source,
        onDone: {
          target: 'Choosing',
          data: ({ state, output }) => ({ source: state.source, skills: output, error: null }),
        },
        onFailure: {
          target: 'Picking',
          data: ({ error }) => ({ error: error.message }),
        },
      },
    },
    Choosing: {
      on: {
        Close: { target: 'Closed' },
        ClearError: {
          update: 'Choosing',
          data: ({ state }) => ({ source: state.source, skills: state.skills, error: null }),
        },
        SelectSource: {
          target: 'Loading',
          guard: ({ state, event }) => event.id !== state.source.id,
          data: ({ event }) => ({
            source: { id: event.id, name: event.name, generation: event.generation },
          }),
        },
        Import: {
          target: 'Importing',
          guard: ({ event }) => event.names.length > 0,
          data: ({ state, event }) => ({
            source: state.source,
            skills: state.skills,
            names: event.names,
          }),
        },
      },
    },
    Importing: {
      on: {
        Close: { target: 'Closed' },
      },
      invoke: {
        src: 'importFromBot',
        input: ({ root, state }) => ({
          targetBotId: root.targetBotId,
          targetBotGeneration: root.targetBotGeneration,
          fromBotId: state.source.id,
          fromBotGeneration: state.source.generation,
          names: state.names,
        }),
        onDone: {
          target: 'Imported',
          data: ({ output }) => ({ skills: output }),
        },
        onFailure: {
          target: 'Choosing',
          data: ({ state, error }) => ({
            source: state.source,
            skills: state.skills,
            error: error.message,
          }),
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
