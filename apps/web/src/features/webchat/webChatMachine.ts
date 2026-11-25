import { Api } from '@blob-1/client/api'
import { ChatMessage, ChatPage } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

/**
 * The newest page applied to the cached window. Ids are consecutive, so a page
 * that starts past the cached tail means messages were missed in between:
 * the window restarts from the page and older paging resumes from there.
 */
const applyNewest = <
  T extends { readonly messages: ReadonlyArray<ChatMessage>; readonly hasMore: boolean },
>(
  root: T,
  page: ChatPage,
): T => {
  const tail = root.messages.at(-1)?.id
  const head = page.messages[0]?.id
  return tail !== undefined && head !== undefined && head > tail + 1
    ? { ...root, messages: page.messages, hasMore: page.hasMore }
    : { ...root, messages: merge(root.messages, page.messages) }
}

/** Messages merged by id and kept in id order, so pages and refreshes can overlap. */
const merge = (
  current: ReadonlyArray<ChatMessage>,
  incoming: ReadonlyArray<ChatMessage>,
): ReadonlyArray<ChatMessage> => {
  const byId = new Map(current.map(message => [message.id, message]))
  for (const message of incoming) byId.set(message.id, message)
  return [...byId.values()].toSorted((a, b) => a.id - b.id)
}

const refreshChat = Effect.gen(function* () {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('chat.history', { before: null })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const loadOlder = Effect.fn('WebChat.loadOlder')(function* (before: number) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('chat.history', { before })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const sendMessage = Effect.fn('WebChat.send')(function* (text: string) {
  const api = yield* Api
  return yield* api.request('chat.send', { text })
})

export const WebChatInput = ChatPage
export type WebChatInput = typeof WebChatInput.Type

export const WebChatStates = Machine.state({
  type: 'parallel',
  fields: {
    messages: Schema.Array(ChatMessage),
    /** Older messages exist before the first loaded one. */
    hasMore: Schema.Boolean,
    working: Schema.Boolean,
    typingUntil: Schema.NullOr(Schema.Finite),
    refreshQueued: Schema.Boolean,
    /** Bumped after a send succeeds, so the composer can clear. */
    sentSeq: Schema.Finite,
    sendError: Schema.NullOr(Schema.String),
  },
  states: {
    Sync: { states: { Idle: {}, Working: {} } },
    Older: { states: { Idle: {}, Working: { fields: { before: Schema.Finite } } } },
    Send: { states: { Idle: {}, Working: { fields: { text: Schema.String } } } },
  },
})

export const WebChatEvents = Machine.events({
  Refresh: {},
  LoadOlder: {},
  Send: { text: Schema.String },
  ClearSendError: {},
})

/** The bot's web chat: newest page refreshes on pushes, older pages load on demand. */
export const WebChatMachine = Machine.make({
  id: 'WebChat',
  root: WebChatStates,
  events: WebChatEvents,
  input: WebChatInput,
  effects: { refreshChat, loadOlder, sendMessage },
}).handle({
  root: ({ input }) => ({
    messages: input.messages,
    hasMore: input.hasMore,
    working: input.working,
    typingUntil: input.typingUntil,
    refreshQueued: false,
    sentSeq: 0,
    sendError: null,
  }),
  on: {
    // Queued so a push that lands mid-fetch still gets one trailing refresh.
    Refresh: {
      update: 'root',
      data: ({ root }) => ({ ...root, refreshQueued: true }),
    },
    ClearSendError: {
      update: 'root',
      data: ({ root }) => ({ ...root, sendError: null }),
    },
  },
  states: {
    Sync: {
      initial: { target: 'Sync.Idle' },
      states: {
        Idle: {
          always: {
            guard: ({ root }) => root.refreshQueued,
            target: 'Sync.Working',
            update: 'root',
            data: ({ root }) => ({ target: undefined, update: { ...root, refreshQueued: false } }),
          },
        },
        Working: {
          invoke: {
            src: 'refreshChat',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update:
                  output === null
                    ? root
                    : {
                        ...applyNewest(root, output),
                        working: output.working,
                        typingUntil: output.typingUntil,
                      },
              }),
            },
          },
        },
      },
    },
    Older: {
      initial: { target: 'Older.Idle' },
      states: {
        Idle: {
          on: {
            LoadOlder: {
              guard: ({ root }) => root.hasMore && root.messages.length > 0,
              target: 'Older.Working',
              data: ({ root }) => ({ before: root.messages[0]?.id ?? 0 }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'loadOlder',
            input: ({ state }) => state.before,
            onDone: {
              target: 'Older.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                // A refresh may have reset the window while this page loaded;
                // a page for the old window would reopen a gap.
                update:
                  output === null || root.messages[0]?.id !== state.before
                    ? root
                    : {
                        ...root,
                        messages: merge(root.messages, output.messages),
                        hasMore: output.hasMore,
                      },
              }),
            },
          },
        },
      },
    },
    Send: {
      initial: { target: 'Send.Idle' },
      states: {
        Idle: {
          on: {
            Send: {
              target: 'Send.Working',
              update: 'root',
              data: ({ root, event }) => ({
                target: { text: event.text },
                update: { ...root, sendError: null },
              }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'sendMessage',
            input: ({ state }) => state.text,
            onDone: {
              target: 'Send.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: {
                  ...applyNewest(root, output),
                  working: output.working,
                  typingUntil: output.typingUntil,
                  sentSeq: root.sentSeq + 1,
                },
              }),
            },
            onFailure: {
              target: 'Send.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: undefined,
                update: { ...root, sendError: error.message },
              }),
            },
          },
        },
      },
    },
  },
})
