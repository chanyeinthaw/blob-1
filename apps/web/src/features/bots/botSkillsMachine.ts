import { Api } from '@blob-1/client/api'
import { BotSkillMode, BotSkillsState, type BotSkill } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const SkillMutation = Schema.Union([
  Schema.Struct({ kind: Schema.Literal('setMode'), name: Schema.String, mode: BotSkillMode }),
  Schema.Struct({ kind: Schema.Literal('remove'), name: Schema.String }),
  Schema.Struct({ kind: Schema.Literal('adopt'), name: Schema.String }),
])
type SkillMutation = typeof SkillMutation.Type

const applyMutation = (
  skills: ReadonlyArray<BotSkill>,
  operation: SkillMutation,
): ReadonlyArray<BotSkill> => {
  if (operation.kind === 'setMode') {
    return skills.map(skill =>
      skill.name === operation.name ? { ...skill, mode: operation.mode } : skill,
    )
  }
  if (operation.kind === 'adopt') {
    return skills.map(skill =>
      skill.name === operation.name ? { ...skill, source: 'managed' as const } : skill,
    )
  }
  return skills.filter(skill => skill.name !== operation.name)
}

const refreshBotSkills = Effect.gen(function* () {
  const api = yield* Api
  return yield* api.request('bot.skills.list', {}).pipe(Effect.orElseSucceed(() => null))
})

const mutateBotSkill = Effect.fn('BotSkills.mutate')(function* (operation: SkillMutation) {
  const api = yield* Api
  const notify = yield* Notify
  const write =
    operation.kind === 'setMode'
      ? api.request('bot.skills.setMode', { name: operation.name, mode: operation.mode })
      : operation.kind === 'adopt'
        ? api.request('bot.skills.adopt', { name: operation.name })
        : api.request('bot.skills.remove', { name: operation.name })
  return yield* write.pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

export const BotSkillsInput = BotSkillsState
export type BotSkillsInput = typeof BotSkillsInput.Type

export const BotSkillsStates = Machine.state({
  type: 'parallel',
  fields: {
    serverData: BotSkillsState,
    data: BotSkillsState,
    inFlightMutation: Schema.Boolean,
    mutationSeq: Schema.Finite,
  },
  states: {
    Mutate: {
      states: {
        Idle: {},
        Working: { fields: { operation: SkillMutation } },
      },
    },
    Sync: {
      states: {
        Idle: {},
        Refreshing: { fields: { mutationSeq: Schema.Finite } },
      },
    },
  },
})

export const BotSkillsEvents = Machine.events({
  SetMode: { name: Schema.String, mode: BotSkillMode },
  Remove: { name: Schema.String },
  Adopt: { name: Schema.String },
  ReplaceData: { state: BotSkillsState },
  Refresh: {},
})

/** Keeps skill mutations optimistic, with refreshes ordered around them. */
export const BotSkillsMachine = Machine.make({
  id: 'BotSkills',
  root: BotSkillsStates,
  events: BotSkillsEvents,
  input: BotSkillsInput,
  effects: { refreshBotSkills, mutateBotSkill },
}).handle({
  root: ({ input }) => ({
    serverData: input,
    data: input,
    inFlightMutation: false,
    mutationSeq: 0,
  }),
  on: {
    ReplaceData: {
      update: 'root',
      data: ({ root, event }) => ({
        ...root,
        serverData: event.state,
        data: event.state,
        mutationSeq: root.mutationSeq + 1,
      }),
    },
  },
  states: {
    Mutate: {
      initial: { target: 'Mutate.Idle' },
      states: {
        Idle: {
          on: {
            SetMode: {
              target: 'Mutate.Working',
              update: 'root',
              data: ({ root, event }) => {
                const operation: SkillMutation = {
                  kind: 'setMode',
                  name: event.name,
                  mode: event.mode,
                }
                return {
                  target: { operation },
                  update: {
                    ...root,
                    data: { skills: applyMutation(root.data.skills, operation) },
                    inFlightMutation: true,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
            },
            Remove: {
              target: 'Mutate.Working',
              update: 'root',
              data: ({ root, event }) => {
                const operation: SkillMutation = { kind: 'remove', name: event.name }
                return {
                  target: { operation },
                  update: {
                    ...root,
                    data: { skills: applyMutation(root.data.skills, operation) },
                    inFlightMutation: true,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
            },
            Adopt: {
              target: 'Mutate.Working',
              update: 'root',
              data: ({ root, event }) => {
                const operation: SkillMutation = { kind: 'adopt', name: event.name }
                return {
                  target: { operation },
                  update: {
                    ...root,
                    data: { skills: applyMutation(root.data.skills, operation) },
                    inFlightMutation: true,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
            },
          },
        },
        Working: {
          invoke: {
            src: 'mutateBotSkill',
            input: ({ state }) => state.operation,
            onDone: {
              target: 'Mutate.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update:
                  output === null
                    ? { ...root, data: root.serverData, inFlightMutation: false }
                    : {
                        ...root,
                        serverData: output,
                        data: output,
                        inFlightMutation: false,
                        mutationSeq: root.mutationSeq + 1,
                      },
              }),
            },
          },
        },
      },
    },
    Sync: {
      initial: { target: 'Sync.Idle' },
      states: {
        Idle: {
          on: {
            Refresh: {
              target: 'Sync.Refreshing',
              data: ({ root }) => ({ mutationSeq: root.mutationSeq }),
            },
          },
        },
        Refreshing: {
          invoke: {
            src: 'refreshBotSkills',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output !== null && state.mutationSeq === root.mutationSeq
                    ? {
                        ...root,
                        serverData: output,
                        // Never clobber an optimistic write while it is still in flight.
                        data: root.inFlightMutation ? root.data : output,
                      }
                    : root,
              }),
            },
          },
        },
      },
    },
  },
})
