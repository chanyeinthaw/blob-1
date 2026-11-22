import { Api } from '@blob-1/client/api'
import { McpAddPayload, McpServer, type RequestPayload } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const refreshServers = Effect.gen(function* () {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('mcp.list', {})
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const setEnabled = Effect.fn('Mcp.setEnabled')(function* (args: {
  readonly id: string
  readonly enabled: boolean
}) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('mcp.setEnabled', args)
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const removeServer = Effect.fn('Mcp.remove')(function* (id: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('mcp.remove', { id })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const login = Effect.fn('Mcp.login')(function* (id: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api.request('mcp.login', { id }).pipe(
    Effect.tap(({ url }) => Effect.sync(() => window.location.assign(url))),
    Effect.as(null),
    Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))),
  )
})

const addServer = Effect.fn('Mcp.add')(function* (payload: RequestPayload<'mcp.add'>) {
  const api = yield* Api
  return yield* api.request('mcp.add', payload)
})

export const McpInput = Schema.Struct({ servers: Schema.Array(McpServer) })
export type McpInput = typeof McpInput.Type

const ErrorMessage = Schema.NullOr(Schema.String)

export const McpStates = Machine.state({
  type: 'parallel',
  fields: { servers: Schema.Array(McpServer) },
  states: {
    Refresh: {
      states: {
        Idle: {},
        Working: {},
      },
    },
    Toggle: {
      states: {
        Idle: {},
        Working: { fields: { id: Schema.String, enabled: Schema.Boolean } },
      },
    },
    Remove: {
      states: {
        Idle: {},
        Working: { fields: { id: Schema.String } },
      },
    },
    Login: {
      states: {
        Idle: {},
        Working: { fields: { id: Schema.String } },
      },
    },
    Add: {
      states: {
        Closed: {},
        Editing: { fields: { error: ErrorMessage } },
        Submitting: { fields: { payload: McpAddPayload } },
      },
    },
  },
})

export const McpEvents = Machine.events({
  Refresh: {},
  OpenAdd: {},
  CloseAdd: {},
  ClearAddError: {},
  SubmitAdd: { payload: McpAddPayload },
  SetEnabled: { id: Schema.String, enabled: Schema.Boolean },
  RemoveServer: { id: Schema.String },
  Login: { id: Schema.String },
})

/** The MCP page: the server list plus refresh, toggle, remove, OAuth login and add workflows. */
export const McpMachine = Machine.make({
  id: 'Mcp',
  root: McpStates,
  events: McpEvents,
  input: McpInput,
  effects: { refreshServers, setEnabled, removeServer, login, addServer },
}).handle({
  root: ({ input }) => input,
  states: {
    Refresh: {
      initial: { target: 'Refresh.Idle' },
      states: {
        Idle: {
          on: {
            Refresh: { target: 'Refresh.Working' },
          },
        },
        Working: {
          invoke: {
            src: 'refreshServers',
            onDone: {
              target: 'Refresh.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, servers: output ?? root.servers },
              }),
            },
          },
        },
      },
    },
    Toggle: {
      initial: { target: 'Toggle.Idle' },
      states: {
        Idle: {
          on: {
            SetEnabled: {
              target: 'Toggle.Working',
              data: ({ event }) => ({ id: event.id, enabled: event.enabled }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'setEnabled',
            input: ({ state }) => state,
            onDone: {
              target: 'Toggle.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, servers: output ?? root.servers },
              }),
            },
          },
        },
      },
    },
    Remove: {
      initial: { target: 'Remove.Idle' },
      states: {
        Idle: {
          on: {
            RemoveServer: {
              target: 'Remove.Working',
              data: ({ event }) => ({ id: event.id }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'removeServer',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Remove.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, servers: output ?? root.servers },
              }),
            },
          },
        },
      },
    },
    Login: {
      initial: { target: 'Login.Idle' },
      states: {
        Idle: {
          on: {
            Login: {
              target: 'Login.Working',
              data: ({ event }) => ({ id: event.id }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'login',
            input: ({ state }) => state.id,
            onDone: { target: 'Login.Idle' },
          },
        },
      },
    },
    Add: {
      initial: { target: 'Add.Closed' },
      states: {
        Closed: {
          on: {
            OpenAdd: { target: 'Add.Editing', data: { error: null } },
          },
        },
        Editing: {
          on: {
            CloseAdd: { target: 'Add.Closed' },
            ClearAddError: { update: 'Add.Editing', data: { error: null } },
            SubmitAdd: {
              target: 'Add.Submitting',
              data: ({ event }) => ({ payload: event.payload }),
            },
          },
        },
        Submitting: {
          invoke: {
            src: 'addServer',
            input: ({ state }) => state.payload,
            onDone: {
              target: 'Add.Closed',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, servers: output },
              }),
            },
            onFailure: {
              target: 'Add.Editing',
              data: ({ error }) => ({ error: error.message }),
            },
          },
        },
      },
    },
  },
})
