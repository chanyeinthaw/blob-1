import { Array, Effect, Option, Schema } from 'effect'
import { Command, Submodel, Update } from 'foldkit'
import { Html, HtmlBuilder } from 'foldkit/html'
import { defineMessageUnion } from 'foldkit/message'
import { defineTaggedUnion } from 'foldkit/schema'
import { modifyFields } from 'foldkit/struct'

import { Dialog, Listbox } from '@foldkit/ui'

import { type Api, request } from '../../api/client'
import { Provider, ProviderApi } from '../../api/protocol'
import * as Button from '../../ui/button'
import * as ConfirmDialog from '../../ui/confirmDialog'
import * as Dropdown from '../../ui/dropdown'
import * as FieldRow from '../../ui/fieldRow'
import * as Icon from '../../ui/icon'
import * as Input from '../../ui/input'
import * as Layout from '../../ui/layout'
import * as Toast from '../../ui/toast'

const apiDropdownId = 'provider-api'

const Status = defineTaggedUnion({
  Loading: {},
  Loaded: { providers: Schema.Array(Provider) },
  Failed: { error: Schema.String },
})
type Status = typeof Status.Type

const AddStatus = defineTaggedUnion({
  Idle: {},
  Submitting: {},
  Failed: { error: Schema.String },
})
type AddStatus = typeof AddStatus.Type

const DraftField = Schema.Struct({
  value: Schema.String,
  maybeError: Schema.Option(Schema.String),
})
type DraftField = typeof DraftField.Type

const Draft = Schema.Struct({
  id: DraftField,
  api: ProviderApi,
  baseUrl: DraftField,
  keyEnv: DraftField,
})
type Draft = typeof Draft.Type

export const Model = Schema.Struct({
  status: Status,
  maybeRefreshingId: Schema.Option(Schema.String),
  maybeRemoveId: Schema.Option(Schema.String),
  confirm: Dialog.Model,
  apiDropdown: Listbox.Model,
  draft: Draft,
  add: AddStatus,
  isAddOpen: Schema.Boolean,
})
export type Model = typeof Model.Type

export const Message = defineMessageUnion({
  SucceededLoadProviders: { providers: Schema.Array(Provider) },
  FailedLoadProviders: { error: Schema.String },
  ClickedRefreshProvider: { id: Schema.String },
  SucceededRefreshProvider: {
    id: Schema.String,
    providers: Schema.Array(Provider),
  },
  FailedRefreshProvider: { error: Schema.String },
  ClickedRemoveProvider: { id: Schema.String },
  ConfirmedRemoveProvider: {},
  SucceededRemoveProvider: { providers: Schema.Array(Provider) },
  FailedRemoveProvider: { error: Schema.String },
  GotConfirmMessage: { message: Dialog.Message },
  GotApiDropdownMessage: { message: Listbox.Message },
  OpenedAddForm: {},
  ClosedAddForm: {},
  UpdatedDraftId: { value: Schema.String },
  UpdatedDraftBaseUrl: { value: Schema.String },
  UpdatedDraftKeyEnv: { value: Schema.String },
  SubmittedAddProvider: {},
  SucceededAddProvider: { providers: Schema.Array(Provider) },
  FailedAddProvider: { error: Schema.String },
})
export type Message = typeof Message.Type

export const OutMessage = defineMessageUnion({
  ShowedToast: { variant: Toast.ToastVariant, title: Schema.String },
})
export type OutMessage = typeof OutMessage.Type

const LoadProviders = Command.define('LoadProviders', {
  messages: [Message.SucceededLoadProviders, Message.FailedLoadProviders],
  execute: request('providers.list', {}).pipe(
    Effect.map(providers => Message.SucceededLoadProviders({ providers })),
    Effect.catch(error =>
      Effect.succeed(Message.FailedLoadProviders({ error: error.message })),
    ),
  ),
})

const RefreshProvider = Command.define('RefreshProvider', {
  args: { id: Schema.String },
  messages: [Message.SucceededRefreshProvider, Message.FailedRefreshProvider],
  execute: ({ id }) =>
    request('providers.refresh', { id }).pipe(
      Effect.map(providers =>
        Message.SucceededRefreshProvider({ id, providers }),
      ),
      Effect.catch(error =>
        Effect.succeed(Message.FailedRefreshProvider({ error: error.message })),
      ),
    ),
})

const RemoveProvider = Command.define('RemoveProvider', {
  args: { id: Schema.String },
  messages: [Message.SucceededRemoveProvider, Message.FailedRemoveProvider],
  execute: ({ id }) =>
    request('providers.remove', { id }).pipe(
      Effect.map(providers => Message.SucceededRemoveProvider({ providers })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedRemoveProvider({ error: error.message })),
      ),
    ),
})

const AddProvider = Command.define('AddProvider', {
  args: {
    id: Schema.String,
    api: ProviderApi,
    baseUrl: Schema.String,
    keyEnv: Schema.String,
  },
  messages: [Message.SucceededAddProvider, Message.FailedAddProvider],
  execute: args =>
    request('providers.add', args).pipe(
      Effect.map(providers => Message.SucceededAddProvider({ providers })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedAddProvider({ error: error.message })),
      ),
    ),
})

const draftFieldWith = (value: string): DraftField => ({
  value,
  maybeError: Option.none(),
})

const initialDraft: Draft = {
  id: draftFieldWith(''),
  api: 'responses',
  baseUrl: draftFieldWith(''),
  keyEnv: draftFieldWith(''),
}

export const init = (): Update.Return<Model, Message, Api> => ({
  model: {
    status: Status.Loading(),
    maybeRefreshingId: Option.none(),
    maybeRemoveId: Option.none(),
    confirm: Dialog.init({ id: 'remove-provider' }),
    apiDropdown: Dropdown.init(apiDropdownId),
    draft: initialDraft,
    add: AddStatus.Idle(),
    isAddOpen: false,
  },
  commands: [LoadProviders()],
})

const foldConfirmOutMessage = Dialog.OutMessage.match<
  Update.Step<Model, Message, Api>
>({
  Opened: () => model => ({ model }),
  Closed: () => model => ({
    model: modifyFields(model, { maybeRemoveId: () => Option.none() }),
  }),
})

const foldConfirmOpen = Update.foldChildStep({
  update: Dialog.open,
  read: (model: Model) => Option.some(model.confirm),
  write: (model: Model, nextConfirm: Dialog.Model) =>
    modifyFields(model, { confirm: () => nextConfirm }),
  toParentMessage: message => Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const foldConfirmClose = Update.foldChildStep({
  update: Dialog.close,
  read: (model: Model) => Option.some(model.confirm),
  write: (model: Model, nextConfirm: Dialog.Model) =>
    modifyFields(model, { confirm: () => nextConfirm }),
  toParentMessage: message => Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const foldConfirm = Update.foldChild({
  update: Dialog.update,
  read: (model: Model) => Option.some(model.confirm),
  write: (model: Model, nextConfirm: Dialog.Model) =>
    modifyFields(model, { confirm: () => nextConfirm }),
  toParentMessage: message => Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const draftWithApi = (model: Model, value: string): Model =>
  Option.match(Schema.decodeUnknownOption(ProviderApi)(value), {
    onNone: () => model,
    onSome: api =>
      modifyFields(model, {
        draft: draft => modifyFields(draft, { api: () => api }),
      }),
  })

const foldApiDropdownOutMessage = Listbox.OutMessage.match<
  Update.Step<Model, Message, Api>
>({
  Selected:
    ({ value }) =>
    model => ({ model: draftWithApi(model, value) }),
})

const foldApiDropdown = Update.foldChild({
  update: Dropdown.Dropdown.update,
  read: (model: Model) => Option.some(model.apiDropdown),
  write: (model: Model, nextApiDropdown: Listbox.Model) =>
    modifyFields(model, { apiDropdown: () => nextApiDropdown }),
  toParentMessage: message => Message.GotApiDropdownMessage({ message }),
  foldOutMessage: foldApiDropdownOutMessage,
})

const modelCountLabel = (count: number): string =>
  `${count} ${count === 1 ? 'model' : 'models'}`

const requiredField = (field: DraftField): DraftField =>
  modifyFields(field, {
    maybeError: () =>
      field.value.trim() === '' ? Option.some('Required') : Option.none(),
  })

export const update = (model: Model, message: Message) =>
  Message.match<Update.ReturnWithOutMessage<Model, Message, OutMessage, Api>>(
    message,
    {
      SucceededLoadProviders: ({ providers }) => ({
        model: modifyFields(model, {
          status: () => Status.Loaded({ providers }),
        }),
      }),

      FailedLoadProviders: ({ error }) => ({
        model: modifyFields(model, { status: () => Status.Failed({ error }) }),
        outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
      }),

      ClickedRefreshProvider: ({ id }) =>
        Option.isSome(model.maybeRefreshingId)
          ? { model }
          : {
              model: modifyFields(model, {
                maybeRefreshingId: () => Option.some(id),
              }),
              commands: [RefreshProvider({ id })],
            },

      SucceededRefreshProvider: ({ providers }) => ({
        model: modifyFields(model, {
          status: () => Status.Loaded({ providers }),
          maybeRefreshingId: () => Option.none(),
        }),
      }),

      FailedRefreshProvider: ({ error }) => ({
        model: modifyFields(model, { maybeRefreshingId: () => Option.none() }),
        outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
      }),

      ClickedRemoveProvider: ({ id }) =>
        foldConfirmOpen(
          modifyFields(model, { maybeRemoveId: () => Option.some(id) }),
        ),

      ConfirmedRemoveProvider: () =>
        Option.match(model.maybeRemoveId, {
          onNone: () => ({ model }),
          onSome: id => {
            const confirmClosed = foldConfirmClose(model)

            return {
              model: confirmClosed.model,
              commands: [
                RemoveProvider({ id }),
                ...(confirmClosed.commands ?? []),
              ],
            }
          },
        }),

      SucceededRemoveProvider: ({ providers }) => ({
        model: modifyFields(model, {
          status: () => Status.Loaded({ providers }),
        }),
      }),

      FailedRemoveProvider: ({ error }) => ({
        model,
        outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
      }),

      GotConfirmMessage: ({ message }) => foldConfirm(model, message),

      GotApiDropdownMessage: ({ message }) => foldApiDropdown(model, message),

      OpenedAddForm: () => ({
        model: modifyFields(model, { isAddOpen: () => true }),
      }),

      ClosedAddForm: () => ({
        model: modifyFields(model, {
          isAddOpen: () => false,
          draft: () => initialDraft,
          add: () => AddStatus.Idle(),
        }),
      }),

      UpdatedDraftId: ({ value }) => ({
        model: modifyFields(model, {
          draft: draft =>
            modifyFields(draft, { id: () => draftFieldWith(value) }),
        }),
      }),

      UpdatedDraftBaseUrl: ({ value }) => ({
        model: modifyFields(model, {
          draft: draft =>
            modifyFields(draft, { baseUrl: () => draftFieldWith(value) }),
        }),
      }),

      UpdatedDraftKeyEnv: ({ value }) => ({
        model: modifyFields(model, {
          draft: draft =>
            modifyFields(draft, { keyEnv: () => draftFieldWith(value) }),
        }),
      }),

      SubmittedAddProvider: () => {
        if (model.add._tag === 'Submitting') {
          return { model }
        }

        const checked = modifyFields(model, {
          draft: draft =>
            modifyFields(draft, {
              id: requiredField,
              baseUrl: requiredField,
              keyEnv: requiredField,
            }),
        })

        const isComplete = Array.every(
          [checked.draft.id, checked.draft.baseUrl, checked.draft.keyEnv],
          field => Option.isNone(field.maybeError),
        )

        if (!isComplete) {
          return { model: checked }
        }

        return {
          model: modifyFields(checked, { add: () => AddStatus.Submitting() }),
          commands: [
            AddProvider({
              id: checked.draft.id.value.trim(),
              api: checked.draft.api,
              baseUrl: checked.draft.baseUrl.value.trim(),
              keyEnv: checked.draft.keyEnv.value.trim(),
            }),
          ],
        }
      },

      SucceededAddProvider: ({ providers }) => ({
        model: modifyFields(model, {
          status: () => Status.Loaded({ providers }),
          draft: () => initialDraft,
          add: () => AddStatus.Idle(),
          isAddOpen: () => false,
        }),
      }),

      FailedAddProvider: ({ error }) => ({
        model: modifyFields(model, { add: () => AddStatus.Failed({ error }) }),
      }),
    },
  )

const apiLabels: Record<ProviderApi, string> = {
  responses: 'Responses',
  completions: 'Chat Completions',
}

const apiOptions: ReadonlyArray<Dropdown.DropdownOption> = Array.map(
  ProviderApi.literals,
  api => ({ value: api, label: apiLabels[api] }),
)

const providerRowView = (
  model: Model,
  provider: Provider,
  h: HtmlBuilder<Message>,
): Html => {
  const isAnyRefreshing = Option.isSome(model.maybeRefreshingId)
  const isRefreshing = Option.contains(model.maybeRefreshingId, provider.id)
  const detailView = (icon: Html, text: string): Html =>
    h.span(
      [h.Class('flex min-w-0 items-center gap-1.5')],
      [
        h.span([h.Class('shrink-0 text-mute')], [icon]),
        h.span([h.Class('truncate font-mono text-caption')], [text]),
      ],
    )

  return h.keyed('li')(
    provider.id,
    [h.Class('flex items-center gap-4 p-4 hover:bg-hover/50')],
    [
      h.span(
        [
          h.AriaHidden(true),
          h.Class(
            'grid size-10 shrink-0 place-items-center rounded-full border border-hair bg-shelf text-slate',
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
              h.p([h.Class('text-body font-medium text-ink')], [provider.id]),
              h.span(
                [
                  h.Class(
                    'rounded-full bg-chip px-2 py-px text-caption font-medium text-ink',
                  ),
                ],
                [apiLabels[provider.api]],
              ),
            ],
          ),
          h.div(
            [
              h.Class(
                'mt-1 flex min-w-0 flex-col gap-1 text-slate sm:flex-row sm:items-center sm:gap-4',
              ),
            ],
            [
              detailView(Icon.link('size-3.5'), provider.baseUrl),
              detailView(Icon.key('size-3.5'), provider.keyEnv),
            ],
          ),
        ],
      ),
      h.span(
        [
          h.AriaLive('polite'),
          h.Class(
            'hidden shrink-0 text-caption text-mute tabular-nums sm:block',
          ),
        ],
        [isRefreshing ? 'Refreshing…' : modelCountLabel(provider.modelCount)],
      ),
      h.div(
        [h.Class('flex shrink-0 items-center gap-2')],
        [
          Button.iconView(
            {
              label: `Refresh ${provider.id} models`,
              icon: Icon.refresh('size-4'),
              onClick: Message.ClickedRefreshProvider({ id: provider.id }),
              isDisabled: isAnyRefreshing,
              isOutlined: true,
            },
            h,
          ),
          Button.iconView(
            {
              label: `Remove ${provider.id}`,
              icon: Icon.trash('size-4'),
              tone: 'danger',
              onClick: Message.ClickedRemoveProvider({ id: provider.id }),
              isOutlined: true,
            },
            h,
          ),
        ],
      ),
    ],
  )
}

const providersListView = (
  model: Model,
  providers: ReadonlyArray<Provider>,
  h: HtmlBuilder<Message>,
): Html =>
  Array.match(providers, {
    onEmpty: () => Layout.emptyLine('No providers yet.', h),
    onNonEmpty: nonEmptyProviders =>
      h.ul(
        [
          h.Class(
            'divide-y divide-hair overflow-hidden rounded-lg border border-hair bg-white shadow-xs',
          ),
        ],
        Array.map(nonEmptyProviders, provider =>
          providerRowView(model, provider, h),
        ),
      ),
  })

const listView = (model: Model, h: HtmlBuilder<Message>): Html =>
  Status.match(model.status, {
    Loading: () => h.p([h.Class('text-slate')], ['Loading…']),
    Failed: ({ error }) => h.p([h.Class('text-alarm')], [error]),
    Loaded: ({ providers }) => providersListView(model, providers, h),
  })

const addErrorView = (add: AddStatus, h: HtmlBuilder<Message>): Html =>
  AddStatus.match(add, {
    Idle: () => h.empty,
    Submitting: () => h.empty,
    Failed: ({ error }) =>
      h.p(
        [h.Class('text-body text-alarm'), h.Attribute('role', 'alert')],
        [error],
      ),
  })

const addFormView = (model: Model, h: HtmlBuilder<Message>): Html => {
  const isSubmitting = model.add._tag === 'Submitting'

  return h.form(
    [h.OnSubmit(Message.SubmittedAddProvider())],
    [
      FieldRow.view(
        {
          id: 'provider-id',
          label: 'ID',
          control: Input.view(
            {
              id: 'provider-id',
              value: model.draft.id.value,
              onInput: value => Message.UpdatedDraftId({ value }),
              maybeError: model.draft.id.maybeError,
              isMono: true,
            },
            h,
          ),
        },
        h,
      ),
      FieldRow.view(
        {
          id: Dropdown.buttonId(apiDropdownId),
          label: 'API',
          control: Dropdown.view(
            {
              model: model.apiDropdown,
              options: apiOptions,
              selected: model.draft.api,
              label: 'Provider API',
              toParentMessage: message =>
                Message.GotApiDropdownMessage({ message }),
            },
            h,
          ),
        },
        h,
      ),
      FieldRow.view(
        {
          id: 'provider-base-url',
          label: 'Base URL',
          control: Input.view(
            {
              id: 'provider-base-url',
              value: model.draft.baseUrl.value,
              onInput: value => Message.UpdatedDraftBaseUrl({ value }),
              maybeError: model.draft.baseUrl.maybeError,
              isMono: true,
              placeholder: 'https://…/v1',
            },
            h,
          ),
        },
        h,
      ),
      FieldRow.view(
        {
          id: 'provider-key-env',
          label: 'Key env var',
          control: Input.view(
            {
              id: 'provider-key-env',
              value: model.draft.keyEnv.value,
              onInput: value => Message.UpdatedDraftKeyEnv({ value }),
              maybeError: model.draft.keyEnv.maybeError,
              isMono: true,
              placeholder: 'OPENAI_API_KEY',
            },
            h,
          ),
        },
        h,
      ),
      h.div(
        [h.Class('mt-3 flex flex-col items-start gap-2 md:pl-[112px]')],
        [
          h.div(
            [h.Class('flex items-center gap-2')],
            [
              Button.view(
                {
                  label: isSubmitting ? 'Adding provider…' : 'Add provider',
                  variant: 'primary',
                  type: 'submit',
                  isDisabled: isSubmitting,
                },
                h,
              ),
              Button.view(
                {
                  label: 'Cancel',
                  variant: 'secondary',
                  maybeOnClick: Option.some(Message.ClosedAddForm()),
                  isDisabled: isSubmitting,
                },
                h,
              ),
            ],
          ),
          addErrorView(model.add, h),
        ],
      ),
    ],
  )
}

export const view = Submodel.defineView<Model, Message>((model, h) =>
  Layout.page(
    {
      title: 'Providers',
      wide: true,
      actions: model.isAddOpen
        ? []
        : [
            Button.view(
              {
                label: 'Add provider',
                variant: 'primary',
                icon: Icon.plus('size-4'),
                maybeOnClick: Option.some(Message.OpenedAddForm()),
              },
              h,
            ),
          ],
      children: [
        ...(model.isAddOpen
          ? [Layout.section('New provider', [addFormView(model, h)], h)]
          : []),
        h.div([h.Class(model.isAddOpen ? 'mt-8' : '')], [listView(model, h)]),
        ConfirmDialog.view(
          {
            model: model.confirm,
            title: 'Remove provider?',
            ...Option.match(model.maybeRemoveId, {
              onNone: () => ({}),
              onSome: id => ({ maybeDetail: id }),
            }),
            confirmLabel: 'Remove',
            onConfirm: Message.ConfirmedRemoveProvider(),
            toParentMessage: message => Message.GotConfirmMessage({ message }),
          },
          h,
        ),
      ],
    },
    h,
  ),
)
