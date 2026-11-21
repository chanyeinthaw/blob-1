import { clsx } from 'clsx'
import { Array, Effect, Match, Option, Schema } from 'effect'
import { Command, Submodel, Update } from 'foldkit'
import { type Html, type HtmlBuilder } from 'foldkit/html'
import { defineMessageUnion } from 'foldkit/message'
import { defineTaggedUnion } from 'foldkit/schema'
import { modifyFields } from 'foldkit/struct'

import { Combobox, Dialog, RadioGroup } from '@foldkit/ui'

import { type Api, request } from '../../api/client'
import {
  Discord,
  DiscordConnection,
  DiscordDirectory,
  Home,
} from '../../api/protocol'
import { chatDiscordRouter, chatRouter } from '../../route'
import * as Button from '../../ui/button'
import * as SharedCombobox from '../../ui/combobox'
import * as ConfirmDialog from '../../ui/confirmDialog'
import * as FieldRow from '../../ui/fieldRow'
import * as Icon from '../../ui/icon'
import * as Input from '../../ui/input'
import * as Layout from '../../ui/layout'
import * as Toast from '../../ui/toast'

const HomeKind = Schema.Literals(['None', 'Channel', 'DirectMessage'])
type HomeKind = typeof HomeKind.Type

const homeKinds: ReadonlyArray<HomeKind> = ['None', 'Channel', 'DirectMessage']

const homeKindLabels: Record<HomeKind, string> = {
  None: 'None',
  Channel: 'Channel',
  DirectMessage: 'Direct message',
}

const State = defineTaggedUnion({
  Loading: {},
  Failed: { error: Schema.String },
  Loaded: {
    token: Schema.Literals(['set', 'missing']),
    connection: DiscordConnection,
    isEditingToken: Schema.Boolean,
    tokenInput: Schema.String,
    maybeTokenError: Schema.Option(Schema.String),
    isSavingToken: Schema.Boolean,
    tokenGeneration: Schema.String,
    allowed: Schema.Array(Schema.String),
    savedHome: Home,
    directory: DiscordDirectory,
    maybeError: Schema.Option(Schema.String),
    homeStatus: Schema.Literals(['Idle', 'Saving', 'Saved']),
    maybeRemoving: Schema.Option(Schema.String),
    homeKind: HomeKind,
    channelInput: Schema.String,
    userInput: Schema.String,
    allowInput: Schema.String,
    maybeAllowError: Schema.Option(Schema.String),
    isAllowing: Schema.Boolean,
  },
})
type State = typeof State.Type
type Loaded = typeof State.Loaded.Type

const Confirming = Schema.Literals(['RemoveUser', 'RemoveToken'])

export const Model = Schema.Struct({
  isListView: Schema.Boolean,
  loadId: Schema.String,
  state: State,
  confirming: Confirming,
  confirm: Dialog.Model,
  homeKindGroup: RadioGroup.Model,
  channelDropdown: Combobox.Model,
  personDropdown: Combobox.Model,
})
export type Model = typeof Model.Type

export const Message = defineMessageUnion({
  SucceededLoadDiscord: {
    discord: Discord,
    directory: DiscordDirectory,
    tokenGeneration: Schema.String,
  },
  FailedLoadDiscord: { error: Schema.String, loadId: Schema.String },
  CreatedLoadId: { loadId: Schema.String },
  UpdatedAllowInput: { value: Schema.String },
  SubmittedAllowUser: {},
  SucceededAllowUser: { discord: Discord },
  FailedAllowUser: { error: Schema.String },
  ClickedRemove: { user: Schema.String },
  GotConfirmMessage: { message: Dialog.Message },
  ConfirmedRemove: {},
  SucceededRemoveUser: { discord: Discord },
  FailedRemoveUser: { error: Schema.String },
  GotHomeKindMessage: { message: RadioGroup.Message },
  GotChannelDropdownMessage: { message: Combobox.Message },
  GotPersonDropdownMessage: { message: Combobox.Message },
  SucceededSetHome: { discord: Discord, home: Home },
  FailedSetHome: { error: Schema.String, home: Home },
  ClickedReplaceToken: {},
  ClickedCancelToken: {},
  UpdatedTokenInput: { value: Schema.String },
  SubmittedToken: {},
  SucceededSaveToken: { discord: Discord, tokenGeneration: Schema.String },
  FailedSaveToken: { error: Schema.String },
  ClickedRemoveToken: {},
  ConfirmedRemoveToken: {},
  SucceededRemoveToken: { discord: Discord, tokenGeneration: Schema.String },
  FailedRemoveToken: { error: Schema.String },
  SucceededRefreshStatus: { discord: Discord },
  FailedRefreshStatus: {},
  SucceededLoadDirectory: {
    directory: DiscordDirectory,
    tokenGeneration: Schema.String,
  },
  FailedLoadDirectory: {},
})
export type Message = typeof Message.Type

export const OutMessage = defineMessageUnion({
  ShowedToast: { variant: Toast.ToastVariant, title: Schema.String },
})
export type OutMessage = typeof OutMessage.Type

type UpdateReturn = Update.ReturnWithOutMessage<Model, Message, OutMessage, Api>

const HomeKindRadioGroup = RadioGroup.create<HomeKind>()

const readLoaded = (model: Model): Option.Option<Loaded> =>
  Match.value(model.state).pipe(
    Match.tag('Loaded', loaded => Option.some(loaded)),
    Match.orElse(() => Option.none()),
  )

const updateLoaded = (
  model: Model,
  change: (loaded: Loaded) => Loaded,
): Model =>
  Option.match(readLoaded(model), {
    onNone: () => model,
    onSome: loaded => modifyFields(model, { state: () => change(loaded) }),
  })

const withLoaded = (
  model: Model,
  step: (loaded: Loaded) => UpdateReturn,
): UpdateReturn =>
  Option.match(readLoaded(model), {
    onNone: () => ({ model }),
    onSome: step,
  })

const homeKindOf = (home: Home): HomeKind =>
  Match.value(home).pipe(
    Match.withReturnType<HomeKind>(),
    Match.discriminatorsExhaustive('kind')({
      none: () => 'None',
      channel: () => 'Channel',
      dm: () => 'DirectMessage',
    }),
  )

const loadedFrom = (
  discord: Discord,
  directory: DiscordDirectory,
  tokenGeneration: string,
): Loaded =>
  State.Loaded({
    token: discord.token,
    connection: discord.connection,
    isEditingToken: discord.token === 'missing',
    tokenInput: '',
    maybeTokenError: Option.none(),
    isSavingToken: false,
    tokenGeneration,
    allowed: discord.allowed,
    savedHome: discord.home,
    directory,
    maybeError: Option.none(),
    homeStatus: 'Idle',
    maybeRemoving: Option.none(),
    homeKind: homeKindOf(discord.home),
    channelInput: discord.home.kind === 'channel' ? discord.home.id : '',
    userInput: discord.home.kind === 'dm' ? discord.home.user : '',
    allowInput: '',
    maybeAllowError: Option.none(),
    isAllowing: false,
  })

const draftHome = (loaded: Loaded): Home =>
  Match.value(loaded.homeKind).pipe(
    Match.withReturnType<Home>(),
    Match.when('None', () => ({ kind: 'none' })),
    Match.when('Channel', () => ({ kind: 'channel', id: loaded.channelInput })),
    Match.when('DirectMessage', () => ({ kind: 'dm', user: loaded.userInput })),
    Match.exhaustive,
  )

const isSameHome = (left: Home, right: Home): boolean => {
  if (left.kind === 'channel' && right.kind === 'channel') {
    return left.id === right.id
  }
  if (left.kind === 'dm' && right.kind === 'dm') {
    return left.user === right.user
  }
  return left.kind === 'none' && right.kind === 'none'
}

const newGeneration = Effect.sync(
  () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`,
)

const NewLoadId = Command.define('NewLoadId', {
  messages: [Message.CreatedLoadId],
  execute: Effect.map(newGeneration, loadId =>
    Message.CreatedLoadId({ loadId }),
  ),
})

const LoadDiscord = Command.define('LoadDiscord', {
  args: { loadId: Schema.String },
  messages: [Message.SucceededLoadDiscord, Message.FailedLoadDiscord],
  execute: ({ loadId }) =>
    Effect.all(
      {
        discord: request('discord.get', {}),
        directory: request('discord.directory', {}),
      },
      { concurrency: 'unbounded' },
    ).pipe(
      Effect.map(({ discord, directory }) =>
        Message.SucceededLoadDiscord({
          discord,
          directory,
          tokenGeneration: loadId,
        }),
      ),
      Effect.catch(error =>
        Effect.succeed(
          Message.FailedLoadDiscord({ error: error.message, loadId }),
        ),
      ),
    ),
})

const AllowUser = Command.define('AllowUser', {
  args: { user: Schema.String },
  messages: [Message.SucceededAllowUser, Message.FailedAllowUser],
  execute: ({ user }) =>
    request('discord.allow', { user }).pipe(
      Effect.map(discord => Message.SucceededAllowUser({ discord })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedAllowUser({ error: error.message })),
      ),
    ),
})

const RemoveUser = Command.define('RemoveUser', {
  args: { user: Schema.String },
  messages: [Message.SucceededRemoveUser, Message.FailedRemoveUser],
  execute: ({ user }) =>
    request('discord.disallow', { user }).pipe(
      Effect.map(discord => Message.SucceededRemoveUser({ discord })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedRemoveUser({ error: error.message })),
      ),
    ),
})

const SetHome = Command.define('SetHome', {
  args: { home: Home },
  messages: [Message.SucceededSetHome, Message.FailedSetHome],
  execute: ({ home }) =>
    request('discord.setHome', { home }).pipe(
      Effect.map(discord => Message.SucceededSetHome({ discord, home })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedSetHome({ error: error.message, home })),
      ),
    ),
})

const SaveToken = Command.define('SaveToken', {
  args: { token: Schema.String },
  messages: [Message.SucceededSaveToken, Message.FailedSaveToken],
  execute: ({ token }) =>
    request('discord.setToken', { token }).pipe(
      Effect.flatMap(discord =>
        Effect.map(newGeneration, tokenGeneration =>
          Message.SucceededSaveToken({ discord, tokenGeneration }),
        ),
      ),
      Effect.catch(error =>
        Effect.succeed(Message.FailedSaveToken({ error: error.message })),
      ),
    ),
})

const RemoveToken = Command.define('RemoveToken', {
  messages: [Message.SucceededRemoveToken, Message.FailedRemoveToken],
  execute: request('discord.clearToken', {}).pipe(
    Effect.flatMap(discord =>
      Effect.map(newGeneration, tokenGeneration =>
        Message.SucceededRemoveToken({ discord, tokenGeneration }),
      ),
    ),
    Effect.catch(error =>
      Effect.succeed(Message.FailedRemoveToken({ error: error.message })),
    ),
  ),
})

const RefreshStatus = Command.define('RefreshStatus', {
  messages: [Message.SucceededRefreshStatus, Message.FailedRefreshStatus],
  execute: Effect.sleep('2 seconds').pipe(
    Effect.andThen(request('discord.get', {})),
    Effect.map(discord => Message.SucceededRefreshStatus({ discord })),
    Effect.catch(() => Effect.succeed(Message.FailedRefreshStatus())),
  ),
})

const LoadDirectory = Command.define('LoadDirectory', {
  args: { tokenGeneration: Schema.String },
  messages: [Message.SucceededLoadDirectory, Message.FailedLoadDirectory],
  execute: ({ tokenGeneration }) =>
    request('discord.directory', {}).pipe(
      Effect.map(directory =>
        Message.SucceededLoadDirectory({ directory, tokenGeneration }),
      ),
      Effect.catch(() => Effect.succeed(Message.FailedLoadDirectory())),
    ),
})

const emptyDirectory: DiscordDirectory = { channels: [], people: [] }

const refreshWhileConnecting = (discord: Discord) =>
  discord.connection.state === 'connecting' ? [RefreshStatus()] : []

const afterStatus = (model: Model, discord: Discord) => [
  ...refreshWhileConnecting(discord),
  ...Option.match(readLoaded(model), {
    onNone: () => [],
    onSome: loaded =>
      discord.connection.state === 'online' &&
      loaded.connection.state !== 'online'
        ? [LoadDirectory({ tokenGeneration: loaded.tokenGeneration })]
        : [],
  }),
]

const withDirectory = (model: Model, directory: DiscordDirectory): Model =>
  Option.match(readLoaded(model), {
    onNone: () => model,
    onSome: loaded => {
      const home = loaded.savedHome
      return modifyFields(model, {
        state: () => modifyFields(loaded, { directory: () => directory }),
        channelDropdown: dropdown =>
          SharedCombobox.setSelection(
            dropdown,
            home.kind === 'channel'
              ? `#${directory.channels.find(channel => channel.id === home.id)?.name ?? home.id}`
              : '',
          ),
        personDropdown: dropdown =>
          SharedCombobox.setSelection(
            dropdown,
            home.kind === 'dm'
              ? (directory.people.find(person => person.id === home.user)
                  ?.name ?? home.user)
              : '',
          ),
      })
    },
  })

const withDiscord = (loaded: Loaded, discord: Discord): Loaded =>
  modifyFields(loaded, {
    token: () => discord.token,
    connection: () => discord.connection,
  })

const foldConfirmOutMessage = Dialog.OutMessage.match<
  Update.Step<Model, Message, Api>
>({
  Opened: () => model => ({ model }),
  Closed: () => model => ({ model }),
})

const foldConfirm = Update.foldChild({
  update: Dialog.update,
  read: (model: Model) => Option.some(model.confirm),
  write: (model: Model, nextConfirm: Dialog.Model) =>
    modifyFields(model, { confirm: () => nextConfirm }),
  toParentMessage: (message: Dialog.Message) =>
    Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const foldConfirmOpen = Update.foldChildStep({
  update: Dialog.open,
  read: (model: Model) => Option.some(model.confirm),
  write: (model: Model, nextConfirm: Dialog.Model) =>
    modifyFields(model, { confirm: () => nextConfirm }),
  toParentMessage: (message: Dialog.Message) =>
    Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const foldConfirmClose = Update.foldChildStep({
  update: Dialog.close,
  read: (model: Model) => Option.some(model.confirm),
  write: (model: Model, nextConfirm: Dialog.Model) =>
    modifyFields(model, { confirm: () => nextConfirm }),
  toParentMessage: (message: Dialog.Message) =>
    Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const saveHome = (model: Model): Update.Return<Model, Message, Api> =>
  Option.match(readLoaded(model), {
    onNone: () => ({ model }),
    onSome: loaded => {
      const home = draftHome(loaded)
      if (loaded.homeStatus === 'Saving') {
        return { model }
      }
      if (
        (home.kind === 'channel' && home.id === '') ||
        (home.kind === 'dm' && home.user === '')
      ) {
        return {
          model: updateLoaded(model, current =>
            modifyFields(current, {
              homeStatus: () => 'Idle',
              maybeError: () => Option.none(),
            }),
          ),
        }
      }
      if (isSameHome(home, loaded.savedHome)) {
        return {
          model: updateLoaded(model, current =>
            modifyFields(current, {
              homeStatus: () => 'Saved',
              maybeError: () => Option.none(),
            }),
          ),
        }
      }
      return {
        model: updateLoaded(model, current =>
          modifyFields(current, {
            homeStatus: () => 'Saving',
            maybeError: () => Option.none(),
          }),
        ),
        commands: [SetHome({ home })],
      }
    },
  })

const foldHomeKindOutMessage = RadioGroup.OutMessage.match<
  Update.Step<Model, Message, Api>,
  RadioGroup.OutMessage<HomeKind>
>({
  Selected:
    ({ value }) =>
    model =>
      Option.match(readLoaded(model), {
        onNone: () => ({ model }),
        onSome: loaded => {
          if (loaded.homeKind === value) {
            return { model }
          }
          const channelInput =
            value === 'Channel' && loaded.savedHome.kind === 'channel'
              ? loaded.savedHome.id
              : loaded.channelInput
          const userInput =
            value === 'DirectMessage' && loaded.savedHome.kind === 'dm'
              ? loaded.savedHome.user
              : loaded.userInput
          const channel = loaded.directory.channels.find(
            item => item.id === channelInput,
          )
          const person = loaded.directory.people.find(
            item => item.id === userInput,
          )
          return saveHome(
            modifyFields(model, {
              state: () =>
                modifyFields(loaded, {
                  homeKind: () => value,
                  channelInput: () => channelInput,
                  userInput: () => userInput,
                }),
              channelDropdown: dropdown =>
                SharedCombobox.setSelection(
                  dropdown,
                  channel === undefined ? channelInput : `#${channel.name}`,
                ),
              personDropdown: dropdown =>
                SharedCombobox.setSelection(
                  dropdown,
                  person?.name ?? userInput,
                ),
            }),
          )
        },
      }),
})

const foldHomeKind = Update.foldChild({
  update: HomeKindRadioGroup.update,
  read: (model: Model) => Option.some(model.homeKindGroup),
  write: (model: Model, nextHomeKindGroup: RadioGroup.Model) =>
    modifyFields(model, { homeKindGroup: () => nextHomeKindGroup }),
  toParentMessage: (message: RadioGroup.Message) =>
    Message.GotHomeKindMessage({ message }),
  foldOutMessage: foldHomeKindOutMessage,
})

const foldChannel = Update.foldChild({
  update: SharedCombobox.Combobox.update,
  read: (model: Model) => Option.some(model.channelDropdown),
  write: (model: Model, next: Combobox.Model) =>
    modifyFields(model, { channelDropdown: () => next }),
  toParentMessage: (message: Combobox.Message) =>
    Message.GotChannelDropdownMessage({ message }),
  foldOutMessage: Combobox.OutMessage.match<
    Update.Step<Model, Message, Api>,
    Combobox.OutMessage<string>
  >({
    Selected:
      ({ value }) =>
      model =>
        saveHome(
          updateLoaded(model, loaded =>
            modifyFields(loaded, { channelInput: () => value }),
          ),
        ),
    ClearedSelection: () => model => ({ model }),
  }),
})

const foldPerson = Update.foldChild({
  update: SharedCombobox.Combobox.update,
  read: (model: Model) => Option.some(model.personDropdown),
  write: (model: Model, next: Combobox.Model) =>
    modifyFields(model, { personDropdown: () => next }),
  toParentMessage: (message: Combobox.Message) =>
    Message.GotPersonDropdownMessage({ message }),
  foldOutMessage: Combobox.OutMessage.match<
    Update.Step<Model, Message, Api>,
    Combobox.OutMessage<string>
  >({
    Selected:
      ({ value }) =>
      model =>
        saveHome(
          updateLoaded(model, loaded =>
            modifyFields(loaded, { userInput: () => value }),
          ),
        ),
    ClearedSelection: () => model => ({ model }),
  }),
})

export const init = (
  isListView: boolean,
): Update.Return<Model, Message, Api> => ({
  model: {
    isListView,
    loadId: '',
    state: State.Loading(),
    confirming: 'RemoveUser',
    confirm: Dialog.init({ id: 'remove-discord-user' }),
    homeKindGroup: RadioGroup.init({ id: 'discord-home-kind' }),
    channelDropdown: SharedCombobox.init('discord-channel'),
    personDropdown: SharedCombobox.init('discord-person'),
  },
  commands: [NewLoadId()],
})

export const showList = (model: Model, isListView: boolean): Model =>
  modifyFields(model, { isListView: () => isListView })

export const update = (model: Model, message: Message) =>
  Message.match<UpdateReturn>(message, {
    CreatedLoadId: ({ loadId }) => ({
      model: modifyFields(model, { loadId: () => loadId }),
      commands: [LoadDiscord({ loadId })],
    }),

    SucceededLoadDiscord: ({ discord, directory, tokenGeneration }) => {
      if (tokenGeneration !== model.loadId) {
        return { model }
      }
      const home = discord.home
      return {
        commands: refreshWhileConnecting(discord),
        model: modifyFields(model, {
          state: () => loadedFrom(discord, directory, tokenGeneration),
          channelDropdown: dropdown =>
            SharedCombobox.setSelection(
              dropdown,
              home.kind === 'channel'
                ? `#${directory.channels.find(channel => channel.id === home.id)?.name ?? home.id}`
                : '',
            ),
          personDropdown: dropdown =>
            SharedCombobox.setSelection(
              dropdown,
              home.kind === 'dm'
                ? (directory.people.find(person => person.id === home.user)
                    ?.name ?? home.user)
                : '',
            ),
        }),
      }
    },

    FailedLoadDiscord: ({ error, loadId }) => ({
      model:
        loadId === model.loadId
          ? modifyFields(model, { state: () => State.Failed({ error }) })
          : model,
    }),

    UpdatedAllowInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          allowInput: () => value,
          maybeAllowError: () => Option.none(),
        }),
      ),
    }),
    SubmittedAllowUser: () =>
      withLoaded(model, loaded => {
        if (loaded.isAllowing || loaded.allowInput === '') {
          return { model }
        }
        const error = !/^\d{17,20}$/.test(loaded.allowInput)
          ? 'Enter a Discord user ID with 17–20 digits.'
          : loaded.allowed.includes(loaded.allowInput)
            ? 'This user is already allowed.'
            : ''
        if (error !== '') {
          return {
            model: updateLoaded(model, current =>
              modifyFields(current, {
                maybeAllowError: () => Option.some(error),
              }),
            ),
          }
        }
        return {
          model: updateLoaded(model, current =>
            modifyFields(current, {
              isAllowing: () => true,
              maybeAllowError: () => Option.none(),
            }),
          ),
          commands: [AllowUser({ user: loaded.allowInput })],
        }
      }),

    SucceededAllowUser: ({ discord }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          allowed: () => discord.allowed,
          allowInput: () => '',
          isAllowing: () => false,
          maybeAllowError: () => Option.none(),
        }),
      ),
    }),

    FailedAllowUser: ({ error }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          isAllowing: () => false,
          maybeAllowError: () => Option.some(error),
        }),
      ),
    }),

    ClickedRemove: ({ user }) =>
      foldConfirmOpen(
        updateLoaded(
          modifyFields(model, { confirming: () => 'RemoveUser' as const }),
          loaded =>
            modifyFields(loaded, { maybeRemoving: () => Option.some(user) }),
        ),
      ),

    GotConfirmMessage: ({ message }) => foldConfirm(model, message),

    ConfirmedRemove: () =>
      withLoaded(model, loaded =>
        Option.match(loaded.maybeRemoving, {
          onNone: () => ({ model }),
          onSome: user => {
            const dialogClose = foldConfirmClose(model)

            return {
              model: dialogClose.model,
              commands: [...(dialogClose.commands ?? []), RemoveUser({ user })],
            }
          },
        }),
      ),

    SucceededRemoveUser: ({ discord }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          allowed: () => discord.allowed,
          maybeRemoving: () => Option.none(),
        }),
      ),
    }),

    FailedRemoveUser: ({ error }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { maybeRemoving: () => Option.none() }),
      ),
      outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
    }),

    GotHomeKindMessage: ({ message }) => foldHomeKind(model, message),

    GotChannelDropdownMessage: ({ message }) => foldChannel(model, message),
    GotPersonDropdownMessage: ({ message }) => foldPerson(model, message),

    SucceededSetHome: ({ discord, home }) => {
      const next = updateLoaded(model, loaded =>
        modifyFields(loaded, {
          savedHome: () => discord.home,
          homeStatus: () => 'Saved',
        }),
      )
      return withLoaded(next, loaded =>
        isSameHome(draftHome(loaded), home) ? { model: next } : saveHome(next),
      )
    },

    FailedSetHome: ({ error }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          homeStatus: () => 'Idle',
          maybeError: () => Option.some(error),
        }),
      ),
    }),

    ClickedReplaceToken: () => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          isEditingToken: () => true,
          tokenInput: () => '',
          maybeTokenError: () => Option.none(),
        }),
      ),
    }),

    ClickedCancelToken: () => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          isEditingToken: () => loaded.token === 'missing',
          tokenInput: () => '',
          maybeTokenError: () => Option.none(),
        }),
      ),
    }),

    UpdatedTokenInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          tokenInput: () => value,
          maybeTokenError: () => Option.none(),
        }),
      ),
    }),

    SubmittedToken: () =>
      withLoaded(model, loaded =>
        loaded.isSavingToken || loaded.tokenInput.trim() === ''
          ? { model }
          : {
              model: updateLoaded(model, current =>
                modifyFields(current, {
                  isSavingToken: () => true,
                  maybeTokenError: () => Option.none(),
                }),
              ),
              commands: [SaveToken({ token: loaded.tokenInput.trim() })],
            },
      ),

    SucceededSaveToken: ({ discord, tokenGeneration }) => ({
      model: updateLoaded(
        discord.connection.state === 'online'
          ? model
          : withDirectory(model, emptyDirectory),
        loaded =>
          modifyFields(withDiscord(loaded, discord), {
            isEditingToken: () => false,
            tokenInput: () => '',
            isSavingToken: () => false,
            tokenGeneration: current =>
              discord.connection.state === 'online' ? current : tokenGeneration,
          }),
      ),
      commands: refreshWhileConnecting(discord),
    }),

    FailedSaveToken: ({ error }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          isSavingToken: () => false,
          maybeTokenError: () => Option.some(error),
        }),
      ),
    }),

    ClickedRemoveToken: () =>
      foldConfirmOpen(
        modifyFields(model, { confirming: () => 'RemoveToken' as const }),
      ),

    ConfirmedRemoveToken: () => {
      const dialogClose = foldConfirmClose(model)
      return {
        model: dialogClose.model,
        commands: [...(dialogClose.commands ?? []), RemoveToken()],
      }
    },

    SucceededRemoveToken: ({ discord, tokenGeneration }) => ({
      model: updateLoaded(withDirectory(model, emptyDirectory), loaded =>
        modifyFields(withDiscord(loaded, discord), {
          isEditingToken: () => true,
          tokenInput: () => '',
          maybeTokenError: () => Option.none(),
          tokenGeneration: () => tokenGeneration,
        }),
      ),
    }),

    FailedRemoveToken: ({ error }) => ({
      model,
      outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
    }),

    SucceededRefreshStatus: ({ discord }) => ({
      model: updateLoaded(model, loaded => withDiscord(loaded, discord)),
      commands: afterStatus(model, discord),
    }),

    FailedRefreshStatus: () => ({
      model,
      commands: Option.exists(
        readLoaded(model),
        loaded => loaded.connection.state === 'connecting',
      )
        ? [RefreshStatus()]
        : [],
    }),

    SucceededLoadDirectory: ({ directory, tokenGeneration }) => ({
      model: Option.exists(
        readLoaded(model),
        loaded =>
          loaded.connection.state === 'online' &&
          loaded.tokenGeneration === tokenGeneration,
      )
        ? withDirectory(model, directory)
        : model,
    }),

    FailedLoadDirectory: () => ({ model }),
  })

const segmentedGroupClassName =
  'inline-flex h-8 items-center rounded-full bg-chip p-[3px]'

const segmentClassName =
  'h-full cursor-pointer rounded-full px-3 text-body font-medium text-slate hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring data-[checked]:bg-white data-[checked]:text-ink data-[checked]:shadow-sm'

const homeKindGroupView = (
  render: RadioGroup.RenderInfo<HomeKind>,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [...render.group, h.Class(segmentedGroupClassName)],
    Array.map(render.options, option =>
      h.keyed('button')(
        option.value,
        [...option.option, h.Class(segmentClassName)],
        [h.span([...option.label], [homeKindLabels[option.value]])],
      ),
    ),
  )

const personName = (loaded: Loaded, user: string): string | undefined =>
  loaded.directory.people.find(person => person.id === user)?.name

const initials = (name: string): string =>
  name
    .split(/\s+/)
    .filter(word => word !== '')
    .slice(0, 2)
    .map(word => word.charAt(0).toUpperCase())
    .join('')

const allowedRowView = (
  loaded: Loaded,
  user: string,
  h: HtmlBuilder<Message>,
): Html => {
  const name = personName(loaded, user)
  return h.keyed('li')(
    user,
    [h.Class('flex min-h-12 items-center gap-3 py-2')],
    [
      h.span(
        [
          h.AriaHidden(true),
          h.Class(
            'grid size-7 shrink-0 place-items-center rounded-full bg-chip text-[11px] font-semibold text-slate',
          ),
        ],
        [name === undefined ? Icon.user('size-3.5') : initials(name)],
      ),
      h.div(
        [h.Class('flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2')],
        [
          name === undefined
            ? h.span([h.Class('break-all font-mono text-body')], [user])
            : h.span([h.Class('text-body text-ink')], [name]),
          name === undefined
            ? h.empty
            : h.span([h.Class('font-mono text-caption text-mute')], [user]),
        ],
      ),
      Button.iconView(
        {
          label: `Remove ${user}`,
          icon: Icon.trash('size-4'),
          tone: 'danger',
          onClick: Message.ClickedRemove({ user }),
        },
        h,
      ),
    ],
  )
}

const allowSectionView = (loaded: Loaded, h: HtmlBuilder<Message>): Html =>
  Layout.section(
    'Who can talk to Blob-1',
    [
      Array.match(loaded.allowed, {
        onEmpty: () => Layout.emptyLine('Nobody can talk to Blob-1 yet.', h),
        onNonEmpty: users =>
          Layout.rowList(
            Array.map(users, user => allowedRowView(loaded, user, h)),
            h,
          ),
      }),
      h.form(
        [h.Class('mt-2'), h.OnSubmit(Message.SubmittedAllowUser())],
        [
          h.label(
            [h.For('discord-allow'), h.Class('sr-only')],
            ['Discord user ID'],
          ),
          h.div(
            [h.Class('flex items-start gap-2')],
            [
              h.div(
                [h.Class('min-w-0 flex-1')],
                [
                  Input.view(
                    {
                      id: 'discord-allow',
                      value: loaded.allowInput,
                      onInput: value => Message.UpdatedAllowInput({ value }),
                      maybeError: loaded.maybeAllowError,
                      isMono: true,
                      inputMode: 'numeric',
                      placeholder: 'Discord user ID',
                    },
                    h,
                  ),
                ],
              ),
              Button.iconView(
                {
                  label: 'Allow user',
                  icon: Icon.plus('size-4'),
                  onClick: Message.SubmittedAllowUser(),
                  isDisabled: loaded.allowInput === '' || loaded.isAllowing,
                },
                h,
              ),
            ],
          ),
        ],
      ),
    ],
    h,
  )

const homeIdView = (
  model: Model,
  loaded: Loaded,
  h: HtmlBuilder<Message>,
): Html => {
  const multipleServers =
    new Set(loaded.directory.channels.map(channel => channel.server)).size > 1
  return Match.value(loaded.homeKind).pipe(
    Match.withReturnType<Html>(),
    Match.when('None', () => h.empty),
    Match.when('Channel', () =>
      SharedCombobox.view(
        {
          model: model.channelDropdown,
          options: loaded.directory.channels.map(channel => ({
            value: channel.id,
            label: `#${channel.name}`,
            maybeHint: multipleServers ? channel.server : '',
            search: `${channel.name} ${channel.server}`,
          })),
          selected: loaded.channelInput,
          placeholder: 'Choose a channel',
          className: 'w-full',
          toParentMessage: message =>
            Message.GotChannelDropdownMessage({ message }),
        },
        h,
      ),
    ),
    Match.when('DirectMessage', () =>
      SharedCombobox.view(
        {
          model: model.personDropdown,
          options: loaded.directory.people.map(person => ({
            value: person.id,
            label: person.name,
            maybeHint: person.id,
          })),
          selected: loaded.userInput,
          placeholder: 'Choose a person',
          className: 'w-full',
          toParentMessage: message =>
            Message.GotPersonDropdownMessage({ message }),
        },
        h,
      ),
    ),
    Match.exhaustive,
  )
}

const savedHomeLabel = (loaded: Loaded): string => {
  const home = loaded.savedHome
  if (home.kind === 'channel') {
    const channel = loaded.directory.channels.find(item => item.id === home.id)
    return channel === undefined ? home.id : `#${channel.name} (${home.id})`
  }
  if (home.kind === 'dm') {
    const person = loaded.directory.people.find(item => item.id === home.user)
    return person === undefined ? home.user : `${person.name} (${home.user})`
  }
  return 'None'
}

const homeFeedback = (loaded: Loaded): string => {
  if (loaded.homeStatus === 'Saving') {
    return 'Saving…'
  }
  if (!isSameHome(draftHome(loaded), loaded.savedHome)) {
    return `Active: ${savedHomeLabel(loaded)}`
  }
  return loaded.homeStatus === 'Saved' ? 'Saved' : ''
}

const homeSectionView = (
  model: Model,
  loaded: Loaded,
  h: HtmlBuilder<Message>,
): Html =>
  Layout.section(
    'Home channel',
    [
      FieldRow.view(
        {
          id: model.homeKindGroup.id,
          label: 'Type',
          control: h.submodel({
            slotId: model.homeKindGroup.id,
            model: model.homeKindGroup,
            view: HomeKindRadioGroup.view,
            viewInputs: {
              options: homeKinds,
              selectedValue: Option.some(loaded.homeKind),
              ariaLabel: 'Home channel',
              toView: render => homeKindGroupView(render, h),
            },
            toParentMessage: message => Message.GotHomeKindMessage({ message }),
          }),
        },
        h,
      ),
      loaded.homeKind === 'None'
        ? h.empty
        : FieldRow.view(
            {
              id: Combobox.inputId(
                loaded.homeKind === 'Channel'
                  ? model.channelDropdown.id
                  : model.personDropdown.id,
              ),
              label: 'Destination',
              control: homeIdView(model, loaded, h),
            },
            h,
          ),
      h.div(
        [
          h.Class(
            'md:grid md:grid-cols-[96px_minmax(0,1fr)] md:items-start md:gap-4',
          ),
        ],
        [
          h.span([h.Class('hidden md:block')]),
          h.div(
            [h.Class('min-w-0')],
            [
              h.p(
                [
                  h.Class('text-caption text-mute empty:sr-only'),
                  h.AriaLive('polite'),
                ],
                [homeFeedback(loaded)],
              ),
              Option.match(loaded.maybeError, {
                onNone: () => h.empty,
                onSome: error =>
                  h.p(
                    [
                      h.Class('text-caption text-alarm'),
                      h.Attribute('role', 'alert'),
                    ],
                    [error],
                  ),
              }),
            ],
          ),
        ],
      ),
    ],
    h,
  )

const removeDialogView = (
  model: Model,
  loaded: Loaded,
  h: HtmlBuilder<Message>,
): Html =>
  model.confirming === 'RemoveToken'
    ? ConfirmDialog.view(
        {
          model: model.confirm,
          title: 'Remove the Discord token?',
          maybeDetail: 'Blob-1 disconnects from Discord.',
          confirmLabel: 'Remove',
          onConfirm: Message.ConfirmedRemoveToken(),
          toParentMessage: message => Message.GotConfirmMessage({ message }),
        },
        h,
      )
    : ConfirmDialog.view(
        {
          model: model.confirm,
          title: 'Remove from allowlist?',
          ...Option.match(loaded.maybeRemoving, {
            onNone: () => ({}),
            onSome: user => ({
              maybeDetail:
                personName(loaded, user) === undefined
                  ? user
                  : `${personName(loaded, user)} (${user})`,
            }),
          }),
          confirmLabel: 'Remove',
          onConfirm: Message.ConfirmedRemove(),
          toParentMessage: message => Message.GotConfirmMessage({ message }),
        },
        h,
      )

type Status = Readonly<{ label: string; dotClassName: string }>

const statusOf = (connection: DiscordConnection): Status =>
  Match.value(connection).pipe(
    Match.withReturnType<Status>(),
    Match.discriminatorsExhaustive('state')({
      off: () => ({ label: 'Not connected', dotClassName: 'bg-ring' }),
      connecting: () => ({ label: 'Connecting…', dotClassName: 'bg-ring' }),
      online: ({ bot }) => ({
        label: `Online as ${bot}`,
        dotClassName: 'bg-ok',
      }),
      error: ({ message }) => ({ label: message, dotClassName: 'bg-alarm' }),
    }),
  )

const statusDot = (className: string, h: HtmlBuilder<Message>): Html =>
  h.span([
    h.AriaHidden(true),
    h.Class(clsx('size-2 shrink-0 rounded-full', className)),
  ])

const statusBadgeView = (loaded: Loaded, h: HtmlBuilder<Message>): Html => {
  const status = statusOf(loaded.connection)
  return h.span(
    [
      h.Class(
        clsx(
          'inline-flex h-7 max-w-full items-center gap-2 rounded-full bg-chip px-3 text-caption font-medium',
          loaded.connection.state === 'error' ? 'text-alarm' : 'text-slate',
        ),
      ),
      h.AriaLive('polite'),
    ],
    [
      statusDot(status.dotClassName, h),
      h.span([h.Class('truncate')], [status.label]),
    ],
  )
}

const tokenEditView = (loaded: Loaded, h: HtmlBuilder<Message>): Html =>
  h.form(
    [h.OnSubmit(Message.SubmittedToken())],
    [
      h.label([h.For('discord-token'), h.Class('sr-only')], ['Bot token']),
      h.div(
        [h.Class('flex flex-wrap items-start gap-2')],
        [
          h.div(
            [h.Class('min-w-0 flex-1 basis-64')],
            [
              Input.view(
                {
                  id: 'discord-token',
                  type: 'password',
                  value: loaded.tokenInput,
                  onInput: value => Message.UpdatedTokenInput({ value }),
                  maybeError: loaded.maybeTokenError,
                  isMono: true,
                  isAutofocus: loaded.token === 'set',
                  autoComplete: 'off',
                  placeholder: 'Bot token',
                },
                h,
              ),
            ],
          ),
          h.div(
            [h.Class('flex items-center gap-2')],
            [
              ...(loaded.token === 'set'
                ? [
                    Button.view(
                      {
                        label: 'Cancel',
                        variant: 'secondary',
                        isDisabled: loaded.isSavingToken,
                        maybeOnClick: Option.some(Message.ClickedCancelToken()),
                      },
                      h,
                    ),
                  ]
                : []),
              Button.view(
                {
                  label: loaded.isSavingToken ? 'Connecting…' : 'Save',
                  variant: 'primary',
                  type: 'submit',
                  isDisabled:
                    loaded.isSavingToken || loaded.tokenInput.trim() === '',
                },
                h,
              ),
            ],
          ),
        ],
      ),
    ],
  )

const tokenSavedView = (h: HtmlBuilder<Message>): Html =>
  h.div(
    [h.Class('flex min-h-10 flex-wrap items-center gap-x-3 gap-y-2')],
    [
      Icon.key('size-4 shrink-0 text-mute'),
      h.span(
        [h.Class('mr-auto font-mono text-body tracking-widest text-slate')],
        ['••••••••••••••••'],
      ),
      h.div(
        [h.Class('flex items-center gap-1')],
        [
          Button.view(
            {
              label: 'Replace',
              variant: 'outline',
              maybeOnClick: Option.some(Message.ClickedReplaceToken()),
            },
            h,
          ),
          Button.iconView(
            {
              label: 'Remove token',
              icon: Icon.trash('size-4'),
              tone: 'danger',
              onClick: Message.ClickedRemoveToken(),
            },
            h,
          ),
        ],
      ),
    ],
  )

const tokenSectionView = (loaded: Loaded, h: HtmlBuilder<Message>): Html =>
  Layout.section(
    'Bot token',
    [loaded.isEditingToken ? tokenEditView(loaded, h) : tokenSavedView(h)],
    h,
  )

const loadedView = (
  model: Model,
  loaded: Loaded,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [],
    [
      tokenSectionView(loaded, h),
      allowSectionView(loaded, h),
      homeSectionView(model, loaded, h),
      removeDialogView(model, loaded, h),
    ],
  )

const contentView = (model: Model, h: HtmlBuilder<Message>): Html =>
  State.match(model.state, {
    Loading: () => h.p([h.Class('text-slate')], ['Loading…']),
    Failed: ({ error }) =>
      h.p([h.Class('text-alarm'), h.Attribute('role', 'alert')], [error]),
    Loaded: loaded => loadedView(model, loaded, h),
  })

const discordItemView = (model: Model, h: HtmlBuilder<Message>): Html =>
  Layout.splitItem(
    {
      href: chatDiscordRouter(),
      label: 'Discord',
      icon: Icon.discord('size-4 shrink-0 text-[#5865F2]'),
      isActive: true,
      ...Option.match(readLoaded(model), {
        onNone: () => ({}),
        onSome: loaded => {
          const status = statusOf(loaded.connection)
          return {
            trailing: h.span(
              [h.Title(status.label), h.Class('flex items-center')],
              [
                statusDot(status.dotClassName, h),
                h.span([h.Class('sr-only')], [status.label]),
              ],
            ),
          }
        },
      }),
    },
    h,
  )

export const view = Submodel.defineView<Model, Message>((model, h) =>
  Layout.splitPage(
    {
      title: 'Chat platforms',
      isListView: model.isListView,
      items: [discordItemView(model, h)],
      detail: {
        title: 'Discord',
        backHref: chatRouter(),
        actions: Option.match(readLoaded(model), {
          onNone: () => [],
          onSome: loaded => [statusBadgeView(loaded, h)],
        }),
        children: [contentView(model, h)],
      },
    },
    h,
  ),
)
