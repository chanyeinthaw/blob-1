import { clsx } from 'clsx'
import { Array, Effect, Match, Option, Schema } from 'effect'
import { Command, Submodel, Update } from 'foldkit'
import { type Html, type HtmlBuilder } from 'foldkit/html'
import { defineMessageUnion } from 'foldkit/message'
import { defineTaggedUnion } from 'foldkit/schema'
import { modifyFields } from 'foldkit/struct'

import { Dialog, RadioGroup, Switch } from '@foldkit/ui'

import { type Api, request } from '../../api/client'
import {
  McpAddPayload,
  McpServer,
  type RequestPayload,
} from '../../api/protocol'
import * as Button from '../../ui/button'
import * as ConfirmDialog from '../../ui/confirmDialog'
import * as FieldRow from '../../ui/fieldRow'
import * as Icon from '../../ui/icon'
import * as Input from '../../ui/input'
import * as Layout from '../../ui/layout'
import * as Toast from '../../ui/toast'

const AddKind = Schema.Literals(['stdio', 'http'])
type AddKind = typeof AddKind.Type

const addKinds: ReadonlyArray<AddKind> = ['stdio', 'http']

const addKindLabels: Record<AddKind, string> = {
  stdio: 'Stdio',
  http: 'HTTP',
}

const State = defineTaggedUnion({
  Loading: {},
  Failed: { error: Schema.String },
  Loaded: {
    servers: Schema.Array(McpServer),
    addKind: AddKind,
    idInput: Schema.String,
    commandInput: Schema.String,
    argsInput: Schema.String,
    envInput: Schema.String,
    urlInput: Schema.String,
    headersInput: Schema.String,
    maybeAddError: Schema.Option(Schema.String),
    isAdding: Schema.Boolean,
    isAddOpen: Schema.Boolean,
    maybeRemoving: Schema.Option(Schema.String),
  },
})
type State = typeof State.Type
type Loaded = typeof State.Loaded.Type

export const Model = Schema.Struct({
  state: State,
  confirm: Dialog.Model,
  addKindGroup: RadioGroup.Model,
})
export type Model = typeof Model.Type

export const Message = defineMessageUnion({
  SucceededLoadMcp: { servers: Schema.Array(McpServer) },
  FailedLoadMcp: { error: Schema.String },
  GotAddKindMessage: { message: RadioGroup.Message },
  UpdatedIdInput: { value: Schema.String },
  UpdatedCommandInput: { value: Schema.String },
  UpdatedArgsInput: { value: Schema.String },
  UpdatedEnvInput: { value: Schema.String },
  UpdatedUrlInput: { value: Schema.String },
  UpdatedHeadersInput: { value: Schema.String },
  OpenedAddForm: {},
  ClosedAddForm: {},
  SubmittedAdd: {},
  SucceededAdd: { servers: Schema.Array(McpServer) },
  FailedAdd: { error: Schema.String },
  ClickedRemove: { id: Schema.String },
  GotConfirmMessage: { message: Dialog.Message },
  ConfirmedRemove: {},
  SucceededRemove: { servers: Schema.Array(McpServer) },
  FailedRemove: { error: Schema.String },
  ClickedToggle: { id: Schema.String, enabled: Schema.Boolean },
  SucceededToggle: { servers: Schema.Array(McpServer) },
  FailedToggle: { error: Schema.String },
  ClickedLogin: { id: Schema.String },
  SucceededLogin: { url: Schema.String },
  FailedLogin: { error: Schema.String },
})
export type Message = typeof Message.Type

export const OutMessage = defineMessageUnion({
  ShowedToast: { variant: Toast.ToastVariant, title: Schema.String },
  RequestedLogin: { url: Schema.String },
})
export type OutMessage = typeof OutMessage.Type

type UpdateReturn = Update.ReturnWithOutMessage<Model, Message, OutMessage, Api>

const AddKindRadioGroup = RadioGroup.create<AddKind>()

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

const loadedFrom = (servers: ReadonlyArray<McpServer>): Loaded =>
  State.Loaded({
    servers,
    addKind: 'stdio',
    idInput: '',
    commandInput: '',
    argsInput: '',
    envInput: '',
    urlInput: '',
    headersInput: '',
    maybeAddError: Option.none(),
    isAdding: false,
    isAddOpen: false,
    maybeRemoving: Option.none(),
  })

// env entries are whitespace-separated `KEY=VALUE`; header entries are
// `;`-separated `Name: ENV_VAR`, matching the CLI flag shapes.
const parseEnv = (input: string): Option.Option<Record<string, string>> => {
  const entries = input
    .trim()
    .split(/\s+/)
    .filter(entry => entry !== '')
    .map(entry => {
      const index = entry.indexOf('=')
      return index < 1
        ? null
        : ([entry.slice(0, index), entry.slice(index + 1)] as const)
    })
  return entries.every(entry => entry !== null)
    ? Option.some(Object.fromEntries(entries))
    : Option.none()
}

const parseHeaders = (input: string): Option.Option<Record<string, string>> => {
  const entries = input
    .split(';')
    .map(entry => entry.trim())
    .filter(entry => entry !== '')
    .map(entry => {
      const index = entry.indexOf(':')
      return index < 1
        ? null
        : ([
            entry.slice(0, index).trim(),
            entry.slice(index + 1).trim(),
          ] as const)
    })
  return entries.every(entry => entry !== null)
    ? Option.some(Object.fromEntries(entries))
    : Option.none()
}

const MCP_ID = /^[a-z0-9-]+$/

const draftServer = (
  loaded: Loaded,
): Option.Option<RequestPayload<'mcp.add'>> => {
  if (!MCP_ID.test(loaded.idInput)) {
    return Option.none()
  }
  return Match.value(loaded.addKind).pipe(
    Match.withReturnType<Option.Option<RequestPayload<'mcp.add'>>>(),
    Match.when('stdio', () =>
      loaded.commandInput.trim() === ''
        ? Option.none()
        : Option.some({
            id: loaded.idInput,
            transport: 'stdio' as const,
            command: loaded.commandInput,
            args: loaded.argsInput
              .trim()
              .split(/\s+/)
              .filter(arg => arg !== ''),
            env: Option.getOrElse(parseEnv(loaded.envInput), () => ({})),
          }),
    ),
    Match.when('http', () =>
      !URL.canParse(loaded.urlInput)
        ? Option.none()
        : Option.some({
            id: loaded.idInput,
            transport: 'http' as const,
            url: loaded.urlInput,
            headers: Option.getOrElse(
              parseHeaders(loaded.headersInput),
              () => ({}),
            ),
          }),
    ),
    Match.exhaustive,
  )
}

const addError = (loaded: Loaded): Option.Option<string> => {
  if (!MCP_ID.test(loaded.idInput)) {
    return Option.some('Id may use lowercase letters, digits and -.')
  }
  if (Option.isNone(parseEnv(loaded.envInput))) {
    return Option.some('Environment entries must look like KEY=VALUE.')
  }
  if (Option.isNone(parseHeaders(loaded.headersInput))) {
    return Option.some('Header entries must look like Name: ENV_VAR.')
  }
  return Match.value(loaded.addKind).pipe(
    Match.when('stdio', () =>
      loaded.commandInput.trim() === ''
        ? Option.some('Command is required.')
        : Option.none(),
    ),
    Match.when('http', () =>
      !URL.canParse(loaded.urlInput)
        ? Option.some('URL is not a valid URL.')
        : Option.none(),
    ),
    Match.exhaustive,
  )
}

const LoadMcp = Command.define('LoadMcp', {
  messages: [Message.SucceededLoadMcp, Message.FailedLoadMcp],
  execute: request('mcp.list', {}).pipe(
    Effect.map(servers => Message.SucceededLoadMcp({ servers })),
    Effect.catch(error =>
      Effect.succeed(Message.FailedLoadMcp({ error: error.message })),
    ),
  ),
})

const AddServer = Command.define('AddServer', {
  args: { payload: McpAddPayload },
  messages: [Message.SucceededAdd, Message.FailedAdd],
  execute: ({ payload }) =>
    request('mcp.add', payload).pipe(
      Effect.map(servers => Message.SucceededAdd({ servers })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedAdd({ error: error.message })),
      ),
    ),
})

const RemoveServer = Command.define('RemoveServer', {
  args: { id: Schema.String },
  messages: [Message.SucceededRemove, Message.FailedRemove],
  execute: ({ id }) =>
    request('mcp.remove', { id }).pipe(
      Effect.map(servers => Message.SucceededRemove({ servers })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedRemove({ error: error.message })),
      ),
    ),
})

const ToggleServer = Command.define('ToggleServer', {
  args: { id: Schema.String, enabled: Schema.Boolean },
  messages: [Message.SucceededToggle, Message.FailedToggle],
  execute: ({ id, enabled }) =>
    request('mcp.setEnabled', { id, enabled }).pipe(
      Effect.map(servers => Message.SucceededToggle({ servers })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedToggle({ error: error.message })),
      ),
    ),
})

const LoginServer = Command.define('LoginServer', {
  args: { id: Schema.String },
  messages: [Message.SucceededLogin, Message.FailedLogin],
  execute: ({ id }) =>
    request('mcp.login', { id }).pipe(
      Effect.map(({ url }) => Message.SucceededLogin({ url })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedLogin({ error: error.message })),
      ),
    ),
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

const foldAddKind = Update.foldChild({
  update: AddKindRadioGroup.update,
  read: (model: Model) => Option.some(model.addKindGroup),
  write: (model: Model, nextAddKindGroup: RadioGroup.Model) =>
    modifyFields(model, { addKindGroup: () => nextAddKindGroup }),
  toParentMessage: (message: RadioGroup.Message) =>
    Message.GotAddKindMessage({ message }),
  foldOutMessage: RadioGroup.OutMessage.match<
    Update.Step<Model, Message, Api>,
    RadioGroup.OutMessage<AddKind>
  >({
    Selected:
      ({ value }) =>
      model =>
        Option.match(readLoaded(model), {
          onNone: () => ({ model }),
          onSome: () => ({
            model: updateLoaded(model, current =>
              modifyFields(current, { addKind: () => value }),
            ),
          }),
        }),
  }),
})

export const init = (): Update.Return<Model, Message, Api> => ({
  model: {
    state: State.Loading(),
    confirm: Dialog.init({ id: 'remove-mcp-server' }),
    addKindGroup: RadioGroup.init({ id: 'mcp-add-kind' }),
  },
  commands: [LoadMcp()],
})

export const update = (model: Model, message: Message) =>
  Message.match<UpdateReturn>(message, {
    SucceededLoadMcp: ({ servers }) => ({
      model: modifyFields(model, { state: () => loadedFrom(servers) }),
    }),

    FailedLoadMcp: ({ error }) => ({
      model: modifyFields(model, { state: () => State.Failed({ error }) }),
    }),

    GotAddKindMessage: ({ message }) => foldAddKind(model, message),

    UpdatedIdInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          idInput: () => value,
          maybeAddError: () => Option.none(),
        }),
      ),
    }),
    UpdatedCommandInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { commandInput: () => value }),
      ),
    }),
    UpdatedArgsInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { argsInput: () => value }),
      ),
    }),
    UpdatedEnvInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { envInput: () => value }),
      ),
    }),
    UpdatedUrlInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { urlInput: () => value }),
      ),
    }),
    UpdatedHeadersInput: ({ value }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { headersInput: () => value }),
      ),
    }),

    OpenedAddForm: () => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { isAddOpen: () => true }),
      ),
    }),

    ClosedAddForm: () => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          isAddOpen: () => false,
          idInput: () => '',
          commandInput: () => '',
          argsInput: () => '',
          envInput: () => '',
          urlInput: () => '',
          headersInput: () => '',
          maybeAddError: () => Option.none(),
        }),
      ),
    }),

    SubmittedAdd: () =>
      withLoaded(model, loaded => {
        if (loaded.isAdding) {
          return { model }
        }
        const error = addError(loaded)
        if (Option.isSome(error)) {
          return {
            model: updateLoaded(model, current =>
              modifyFields(current, { maybeAddError: () => error }),
            ),
          }
        }
        const payload = draftServer(loaded)
        if (Option.isNone(payload)) {
          return { model }
        }
        return {
          model: updateLoaded(model, current =>
            modifyFields(current, {
              isAdding: () => true,
              maybeAddError: () => Option.none(),
            }),
          ),
          commands: [AddServer({ payload: payload.value })],
        }
      }),

    SucceededAdd: ({ servers }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          servers: () => servers,
          idInput: () => '',
          commandInput: () => '',
          argsInput: () => '',
          envInput: () => '',
          urlInput: () => '',
          headersInput: () => '',
          isAdding: () => false,
          isAddOpen: () => false,
          maybeAddError: () => Option.none(),
        }),
      ),
    }),

    FailedAdd: ({ error }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          isAdding: () => false,
          maybeAddError: () => Option.some(error),
        }),
      ),
    }),

    ClickedRemove: ({ id }) =>
      foldConfirmOpen(
        updateLoaded(model, loaded =>
          modifyFields(loaded, { maybeRemoving: () => Option.some(id) }),
        ),
      ),

    GotConfirmMessage: ({ message }) => foldConfirm(model, message),

    ConfirmedRemove: () =>
      withLoaded(model, loaded =>
        Option.match(loaded.maybeRemoving, {
          onNone: () => ({ model }),
          onSome: id => {
            const dialogClose = foldConfirmClose(model)

            return {
              model: dialogClose.model,
              commands: [...(dialogClose.commands ?? []), RemoveServer({ id })],
            }
          },
        }),
      ),

    SucceededRemove: ({ servers }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, {
          servers: () => servers,
          maybeRemoving: () => Option.none(),
        }),
      ),
    }),

    FailedRemove: ({ error }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { maybeRemoving: () => Option.none() }),
      ),
      outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
    }),

    ClickedToggle: ({ id, enabled }) => ({
      model,
      commands: [ToggleServer({ id, enabled })],
    }),

    SucceededToggle: ({ servers }) => ({
      model: updateLoaded(model, loaded =>
        modifyFields(loaded, { servers: () => servers }),
      ),
    }),

    FailedToggle: ({ error }) => ({
      model,
      outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
    }),

    ClickedLogin: ({ id }) => ({ model, commands: [LoginServer({ id })] }),

    SucceededLogin: ({ url }) => ({
      model,
      outMessage: OutMessage.RequestedLogin({ url }),
    }),

    FailedLogin: ({ error }) => ({
      model,
      outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
    }),
  })

const segmentedGroupClassName =
  'inline-flex h-8 items-center rounded-lg bg-chip p-[3px]'

const segmentClassName =
  'h-full cursor-pointer rounded-md px-3 text-body font-medium text-slate hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring data-[checked]:bg-white data-[checked]:text-ink data-[checked]:shadow-sm'

const addKindGroupView = (
  render: RadioGroup.RenderInfo<AddKind>,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [...render.group, h.Class(segmentedGroupClassName)],
    Array.map(render.options, option =>
      h.keyed('button')(
        option.value,
        [...option.option, h.Class(segmentClassName)],
        [h.span([...option.label], [addKindLabels[option.value]])],
      ),
    ),
  )

const statusBadge = (server: McpServer, h: HtmlBuilder<Message>): Html => {
  const chip = (
    className: string,
    children: ReadonlyArray<Html | string>,
  ): Html =>
    h.span(
      [
        h.Class(
          clsx(
            'inline-flex items-center gap-1.5 rounded-md bg-chip px-1.5 py-px text-caption font-medium',
            className,
          ),
        ),
      ],
      [...children],
    )
  if (!server.enabled || server.status === 'disconnected') {
    return chip('text-mute', ['Disabled'])
  }
  return Match.value(server.status).pipe(
    Match.withReturnType<Html>(),
    Match.when('connected', () =>
      chip('text-ink', [
        h.span([h.Class('size-1.5 rounded-full bg-ok')]),
        server.toolCount === undefined
          ? 'Connected'
          : `Connected · ${server.toolCount} tools`,
      ]),
    ),
    Match.when('connecting', () => chip('text-mute', ['Connecting…'])),
    Match.when('idle', () => chip('text-mute', ['Not connected yet'])),
    Match.when('needs-login', () => chip('text-alarm', ['Needs login'])),
    Match.when('error', () => chip('text-alarm', ['Error'])),
    Match.exhaustive,
  )
}

const serverDetail = (server: McpServer): string =>
  server.transport === 'stdio'
    ? [server.command, ...(server.args ?? [])].join(' ')
    : (server.url ?? '')

const switchTrackClassName =
  'group relative inline-flex h-[18px] w-[30px] shrink-0 cursor-pointer items-center rounded-full bg-track shadow-xs motion-safe:transition-colors data-[checked]:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring'

const switchThumbClassName =
  'size-3.5 translate-x-0.5 rounded-full bg-white motion-safe:transition-transform group-data-[checked]:translate-x-[14px]'

const enabledSwitch = (server: McpServer, h: HtmlBuilder<Message>): Html =>
  Switch.view(
    {
      id: `mcp-enabled-${server.id}`,
      isChecked: server.enabled,
      onToggle: enabled => Message.ClickedToggle({ id: server.id, enabled }),
      toView: attributes =>
        h.div(
          [h.Class('flex shrink-0 items-center justify-end')],
          [
            h.span(
              [...attributes.label, h.Class('sr-only')],
              [`Enable ${server.id}`],
            ),
            h.button(
              [...attributes.button, h.Class(switchTrackClassName)],
              [h.span([h.Class(switchThumbClassName)])],
            ),
          ],
        ),
    },
    h,
  )

const needsOauth = (server: McpServer): boolean =>
  server.enabled &&
  server.transport === 'http' &&
  server.auth !== 'headers' &&
  server.status !== 'connected'

const serverRowView = (server: McpServer, h: HtmlBuilder<Message>): Html =>
  h.keyed('li')(
    server.id,
    [h.Class('flex items-center gap-4 p-4 hover:bg-hover/50')],
    [
      h.span(
        [
          h.AriaHidden(true),
          h.Class(
            'grid size-10 shrink-0 place-items-center rounded-md border border-hair bg-shelf text-slate',
          ),
        ],
        [Icon.server('size-5')],
      ),
      h.div(
        [h.Class('min-w-0 flex-1')],
        [
          h.div(
            [h.Class('flex flex-wrap items-center gap-2')],
            [
              h.p([h.Class('text-body font-medium text-ink')], [server.id]),
              statusBadge(server, h),
            ],
          ),
          h.div(
            [h.Class('mt-1 flex min-w-0 items-center gap-1.5 text-slate')],
            [
              h.span(
                [h.Class('shrink-0 text-mute')],
                [
                  server.transport === 'stdio'
                    ? Icon.terminal('size-3.5')
                    : Icon.link('size-3.5'),
                ],
              ),
              h.span(
                [h.Class('truncate font-mono text-caption')],
                [serverDetail(server)],
              ),
            ],
          ),
          ...(server.error === undefined
            ? []
            : [
                h.p(
                  [
                    h.Class('mt-1 truncate text-caption text-alarm'),
                    h.Title(server.error),
                  ],
                  [server.error],
                ),
              ]),
        ],
      ),
      h.div(
        [h.Class('flex shrink-0 items-center gap-2')],
        [
          ...(needsOauth(server)
            ? [
                Button.view(
                  {
                    label: 'Connect',
                    variant: 'outline',
                    maybeOnClick: Option.some(
                      Message.ClickedLogin({ id: server.id }),
                    ),
                  },
                  h,
                ),
              ]
            : []),
          enabledSwitch(server, h),
          Button.iconView(
            {
              label: `Remove ${server.id}`,
              icon: Icon.trash('size-4'),
              tone: 'danger',
              onClick: Message.ClickedRemove({ id: server.id }),
              isOutlined: true,
            },
            h,
          ),
        ],
      ),
    ],
  )

const serversSectionView = (loaded: Loaded, h: HtmlBuilder<Message>): Html =>
  Layout.section(
    'Servers',
    [
      Array.match(loaded.servers, {
        onEmpty: () => Layout.emptyLine('No MCP servers configured yet.', h),
        onNonEmpty: servers =>
          h.ul(
            [
              h.Class(
                'divide-y divide-hair overflow-hidden rounded-lg border border-hair bg-white shadow-xs',
              ),
            ],
            Array.map(servers, server => serverRowView(server, h)),
          ),
      }),
    ],
    h,
  )

const addSectionView = (
  model: Model,
  loaded: Loaded,
  h: HtmlBuilder<Message>,
): Html =>
  Layout.section(
    'New server',
    [
      h.form(
        [h.OnSubmit(Message.SubmittedAdd())],
        [
          FieldRow.view(
            {
              id: model.addKindGroup.id,
              label: 'Transport',
              control: h.submodel({
                slotId: model.addKindGroup.id,
                model: model.addKindGroup,
                view: AddKindRadioGroup.view,
                viewInputs: {
                  options: addKinds,
                  selectedValue: Option.some(loaded.addKind),
                  ariaLabel: 'Transport',
                  toView: render => addKindGroupView(render, h),
                },
                toParentMessage: message =>
                  Message.GotAddKindMessage({ message }),
              }),
            },
            h,
          ),
          FieldRow.view(
            {
              id: 'mcp-add-id',
              label: 'Id',
              control: Input.view(
                {
                  id: 'mcp-add-id',
                  value: loaded.idInput,
                  onInput: value => Message.UpdatedIdInput({ value }),
                  maybeError: Option.none(),
                  isMono: true,
                  placeholder: 'filesystem',
                },
                h,
              ),
            },
            h,
          ),
          ...Match.value(loaded.addKind).pipe(
            Match.withReturnType<ReadonlyArray<Html>>(),
            Match.when('stdio', () => [
              FieldRow.view(
                {
                  id: 'mcp-add-command',
                  label: 'Command',
                  control: Input.view(
                    {
                      id: 'mcp-add-command',
                      value: loaded.commandInput,
                      onInput: value => Message.UpdatedCommandInput({ value }),
                      maybeError: Option.none(),
                      isMono: true,
                      placeholder: 'npx',
                    },
                    h,
                  ),
                },
                h,
              ),
              FieldRow.view(
                {
                  id: 'mcp-add-args',
                  label: 'Arguments',
                  control: Input.view(
                    {
                      id: 'mcp-add-args',
                      value: loaded.argsInput,
                      onInput: value => Message.UpdatedArgsInput({ value }),
                      maybeError: Option.none(),
                      isMono: true,
                      placeholder:
                        '-y @modelcontextprotocol/server-filesystem /tmp',
                    },
                    h,
                  ),
                },
                h,
              ),
              FieldRow.view(
                {
                  id: 'mcp-add-env',
                  label: 'Environment',
                  control: Input.view(
                    {
                      id: 'mcp-add-env',
                      value: loaded.envInput,
                      onInput: value => Message.UpdatedEnvInput({ value }),
                      maybeError: Option.none(),
                      isMono: true,
                      placeholder: 'KEY=VALUE OTHER=VALUE',
                    },
                    h,
                  ),
                },
                h,
              ),
            ]),
            Match.when('http', () => [
              FieldRow.view(
                {
                  id: 'mcp-add-url',
                  label: 'URL',
                  control: Input.view(
                    {
                      id: 'mcp-add-url',
                      value: loaded.urlInput,
                      onInput: value => Message.UpdatedUrlInput({ value }),
                      maybeError: Option.none(),
                      isMono: true,
                      placeholder: 'https://mcp.example.com/mcp',
                    },
                    h,
                  ),
                },
                h,
              ),
              FieldRow.view(
                {
                  id: 'mcp-add-headers',
                  label: 'Headers',
                  control: Input.view(
                    {
                      id: 'mcp-add-headers',
                      value: loaded.headersInput,
                      onInput: value => Message.UpdatedHeadersInput({ value }),
                      maybeError: Option.none(),
                      isMono: true,
                      placeholder: 'Authorization: API_TOKEN; X-Team: TEAM_ID',
                    },
                    h,
                  ),
                },
                h,
              ),
            ]),
            Match.exhaustive,
          ),
          h.div(
            [h.Class('mt-3 flex flex-col items-start gap-2 md:pl-[112px]')],
            [
              h.div(
                [h.Class('flex items-center gap-2')],
                [
                  Button.view(
                    {
                      label: loaded.isAdding ? 'Adding server…' : 'Add server',
                      variant: 'primary',
                      type: 'submit',
                      isDisabled: loaded.isAdding,
                    },
                    h,
                  ),
                  Button.view(
                    {
                      label: 'Cancel',
                      variant: 'secondary',
                      maybeOnClick: Option.some(Message.ClosedAddForm()),
                      isDisabled: loaded.isAdding,
                    },
                    h,
                  ),
                ],
              ),
              Option.match(loaded.maybeAddError, {
                onNone: () => h.empty,
                onSome: error =>
                  h.p(
                    [
                      h.Class('text-body text-alarm'),
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
  ConfirmDialog.view(
    {
      model: model.confirm,
      title: 'Remove MCP server?',
      ...Option.match(loaded.maybeRemoving, {
        onNone: () => ({}),
        onSome: id => ({ maybeDetail: id }),
      }),
      confirmLabel: 'Remove',
      onConfirm: Message.ConfirmedRemove(),
      toParentMessage: message => Message.GotConfirmMessage({ message }),
    },
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
      ...(loaded.isAddOpen ? [addSectionView(model, loaded, h)] : []),
      serversSectionView(loaded, h),
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

export const view = Submodel.defineView<Model, Message>((model, h) =>
  Layout.page(
    {
      title: 'MCP',
      actions: Option.match(readLoaded(model), {
        onNone: () => [],
        onSome: loaded =>
          loaded.isAddOpen
            ? []
            : [
                Button.view(
                  {
                    label: 'Add server',
                    variant: 'primary',
                    icon: Icon.plus('size-4'),
                    maybeOnClick: Option.some(Message.OpenedAddForm()),
                  },
                  h,
                ),
              ],
      }),
      children: [contentView(model, h)],
    },
    h,
  ),
)
