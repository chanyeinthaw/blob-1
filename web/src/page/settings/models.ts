import { Array, Effect, Match, Option, Record, Schema } from 'effect'
import { Command, Submodel, Update } from 'foldkit'
import { Html, HtmlBuilder } from 'foldkit/html'
import { defineMessageUnion } from 'foldkit/message'
import { defineTaggedUnion } from 'foldkit/schema'
import { modifyFields } from 'foldkit/struct'

import { Listbox, Switch, Input as UiInput } from '@foldkit/ui'

import { type Api, request } from '../../api/client'
import { type ModelInfo, Models, Reasoning, Slot } from '../../api/protocol'
import * as Button from '../../ui/button'
import * as Icon from '../../ui/icon'
import * as Layout from '../../ui/layout'
import * as Menu from '../../ui/menu'
import * as Toast from '../../ui/toast'

const ALL = 'all'
const REMOVE = 'remove'
const PROVIDER_MENU = 'provider-filter'
const PAGE_SIZE = 25

const OptionalRole = Schema.Literals(['compaction', 'utility'])
type OptionalRole = typeof OptionalRole.Type

const Role = Schema.Literals(['main', 'compaction', 'utility'])
type Role = typeof Role.Type

const Statuses = Schema.Record(Schema.String, Schema.String)
const ContextDraft = Schema.Struct({
  value: Schema.String,
  version: Schema.Number,
  status: Schema.String,
  saving: Schema.Boolean,
  savingVersion: Schema.Number,
  queued: Schema.Option(
    Schema.Struct({
      limit: Schema.NullOr(Schema.Number),
      version: Schema.Number,
    }),
  ),
})
const ContextDrafts = Schema.Record(Schema.String, ContextDraft)

export const Model = defineTaggedUnion({
  Loading: {},
  Loaded: {
    server: Models,
    shown: Models,
    roleStatuses: Statuses,
    contextDrafts: ContextDrafts,
    query: Schema.String,
    providerFilter: Schema.String,
    isInUseOnly: Schema.Boolean,
    visibleCount: Schema.Number,
    menus: Schema.Record(Schema.String, Listbox.Model),
  },
  Failed: { error: Schema.String },
})
export type Model = typeof Model.Type

type LoadedModel = typeof Model.Loaded.Type

export const Message = defineMessageUnion({
  SucceededLoadModels: { models: Models },
  FailedLoadModels: { error: Schema.String },
  GotProviderMenuMessage: { message: Listbox.Message },
  GotAssignMenuMessage: {
    providerId: Schema.String,
    modelId: Schema.String,
    message: Listbox.Message,
  },
  GotRoleMenuMessage: { roleId: Role, message: Listbox.Message },
  SucceededSaveMain: { models: Models },
  FailedSaveMain: { error: Schema.String },
  SucceededSaveOptional: { models: Models, role: OptionalRole },
  FailedSaveOptional: { error: Schema.String, role: OptionalRole },
  SucceededSaveImages: { models: Models },
  FailedSaveImages: { error: Schema.String },
  ToggledImages: {
    provider: Schema.String,
    modelId: Schema.String,
    images: Schema.Boolean,
  },
  UpdatedContext: {
    provider: Schema.String,
    modelId: Schema.String,
    value: Schema.String,
  },
  BlurredContext: { provider: Schema.String, modelId: Schema.String },
  SucceededSaveContext: {
    provider: Schema.String,
    modelId: Schema.String,
    models: Models,
  },
  FailedSaveContext: {
    provider: Schema.String,
    modelId: Schema.String,
    error: Schema.String,
  },
  UpdatedQuery: { query: Schema.String },
  ToggledInUseOnly: {},
  ClickedShowMore: {},
})
export type Message = typeof Message.Type

export const OutMessage = defineMessageUnion({
  ShowedToast: { variant: Toast.ToastVariant, title: Schema.String },
})
export type OutMessage = typeof OutMessage.Type

const LoadModels = Command.define('LoadModels', {
  messages: [Message.SucceededLoadModels, Message.FailedLoadModels],
  execute: request('models.get', {}).pipe(
    Effect.map(models => Message.SucceededLoadModels({ models })),
    Effect.catch(error =>
      Effect.succeed(Message.FailedLoadModels({ error: error.message })),
    ),
  ),
})

const SaveMain = Command.define('SaveMain', {
  args: { slot: Slot },
  messages: [Message.SucceededSaveMain, Message.FailedSaveMain],
  execute: ({ slot }) =>
    request('models.setMain', slot).pipe(
      Effect.map(models => Message.SucceededSaveMain({ models })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedSaveMain({ error: error.message })),
      ),
    ),
})

const SaveOptional = Command.define('SaveOptional', {
  args: { role: OptionalRole, slot: Schema.NullOr(Slot) },
  messages: [Message.SucceededSaveOptional, Message.FailedSaveOptional],
  execute: ({ role, slot }) =>
    request(
      role === 'compaction' ? 'models.setCompaction' : 'models.setUtility',
      { slot },
    ).pipe(
      Effect.map(models => Message.SucceededSaveOptional({ models, role })),
      Effect.catch(error =>
        Effect.succeed(
          Message.FailedSaveOptional({ error: error.message, role }),
        ),
      ),
    ),
})

const SaveImages = Command.define('SaveImages', {
  args: {
    provider: Schema.String,
    modelId: Schema.String,
    images: Schema.Boolean,
  },
  messages: [Message.SucceededSaveImages, Message.FailedSaveImages],
  execute: ({ provider, modelId, images }) =>
    request('models.setImages', { provider, model: modelId, images }).pipe(
      Effect.map(models => Message.SucceededSaveImages({ models })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedSaveImages({ error: error.message })),
      ),
    ),
})

const SaveContext = Command.define('SaveContext', {
  args: {
    provider: Schema.String,
    modelId: Schema.String,
    limit: Schema.NullOr(Schema.Number),
  },
  messages: [Message.SucceededSaveContext, Message.FailedSaveContext],
  execute: ({ provider, modelId, limit }) =>
    request('models.setContextLimit', { provider, model: modelId, limit }).pipe(
      Effect.map(models =>
        Message.SucceededSaveContext({ provider, modelId, models }),
      ),
      Effect.catch(error =>
        Effect.succeed(
          Message.FailedSaveContext({
            provider,
            modelId,
            error: error.message,
          }),
        ),
      ),
    ),
})

export const init = (): Update.Return<Model, Message, Api> => ({
  model: Model.Loading(),
  commands: [LoadModels()],
})

type UpdateReturn = Update.ReturnWithOutMessage<Model, Message, OutMessage, Api>
const withUpdateReturn = Match.withReturnType<UpdateReturn>()

const decodeReasoning = Schema.decodeUnknownOption(Reasoning)

const optionalSlot = (shown: Models, role: OptionalRole): Slot | null =>
  role === 'compaction' ? shown.compaction : shown.utility

const onLoaded = (
  model: Model,
  step: (loaded: LoadedModel) => UpdateReturn,
): UpdateReturn =>
  Match.value(model).pipe(
    withUpdateReturn,
    Match.tag('Loaded', step),
    Match.orElse(() => ({ model })),
  )

const withServerModels = (model: Model, models: Models): Model =>
  Match.value(model).pipe(
    Match.withReturnType<Model>(),
    Match.tag('Loaded', loaded =>
      modifyFields(loaded, {
        server: () => models,
        shown: shown =>
          modifyFields(models, {
            main: () =>
              JSON.stringify(shown.main) === JSON.stringify(loaded.server.main)
                ? models.main
                : shown.main,
            compaction: () =>
              JSON.stringify(shown.compaction) ===
              JSON.stringify(loaded.server.compaction)
                ? models.compaction
                : shown.compaction,
            utility: () =>
              JSON.stringify(shown.utility) ===
              JSON.stringify(loaded.server.utility)
                ? models.utility
                : shown.utility,
            providers: () =>
              models.providers.map(provider =>
                modifyFields(provider, {
                  models: entries =>
                    entries.map(info => {
                      const old = loaded.server.providers
                        .find(item => item.id === provider.id)
                        ?.models.find(item => item.id === info.id)
                      const current = shown.providers
                        .find(item => item.id === provider.id)
                        ?.models.find(item => item.id === info.id)
                      return old !== undefined &&
                        current !== undefined &&
                        old.images !== current.images
                        ? modifyFields(info, { images: () => current.images })
                        : info
                    }),
                }),
              ),
          }),
      }),
    ),
    Match.orElse(() => model),
  )

const revertToServer = (model: Model): Model =>
  Match.value(model).pipe(
    Match.withReturnType<Model>(),
    Match.tag('Loaded', loaded =>
      modifyFields(loaded, { shown: () => loaded.server }),
    ),
    Match.orElse(() => model),
  )

const errorToast = (title: string) =>
  OutMessage.ShowedToast({ variant: 'Error', title })

const savedRole = (model: Model, models: Models, role: string): UpdateReturn =>
  onLoaded(withServerModels(model, models), loaded => ({
    model: modifyFields(loaded, {
      roleStatuses: statuses => Record.set(statuses, role, 'Saved'),
    }),
  }))

const failedRole = (
  model: Model,
  error: string,
  role: 'main' | OptionalRole,
): UpdateReturn =>
  onLoaded(model, loaded => ({
    model: modifyFields(loaded, {
      shown: shown =>
        modifyFields(shown, {
          main: current => (role === 'main' ? loaded.server.main : current),
          compaction: current =>
            role === 'compaction' ? loaded.server.compaction : current,
          utility: current =>
            role === 'utility' ? loaded.server.utility : current,
        }),
      roleStatuses: statuses => Record.set(statuses, role, error),
    }),
    outMessage: errorToast(error),
  }))

const failedSave = (model: Model, error: string): UpdateReturn => ({
  model: revertToServer(model),
  outMessage: errorToast(error),
})

const selectMain = (loaded: LoadedModel, slot: Slot): UpdateReturn =>
  loaded.roleStatuses.main === 'Saving…'
    ? { model: loaded }
    : {
        model: modifyFields(loaded, {
          shown: shown => modifyFields(shown, { main: () => slot }),
          roleStatuses: statuses => Record.set(statuses, 'main', 'Saving…'),
        }),
        commands: [SaveMain({ slot })],
      }

const selectOptional = (
  loaded: LoadedModel,
  role: OptionalRole,
  slot: Slot | null,
): UpdateReturn =>
  loaded.roleStatuses[role] === 'Saving…'
    ? { model: loaded }
    : {
        model: modifyFields(loaded, {
          shown: shown =>
            role === 'compaction'
              ? modifyFields(shown, { compaction: () => slot })
              : modifyFields(shown, { utility: () => slot }),
          roleStatuses: statuses => Record.set(statuses, role, 'Saving…'),
        }),
        commands: [SaveOptional({ role, slot })],
      }

const withImages = (
  shown: Models,
  provider: string,
  modelId: string,
  images: boolean,
): Models =>
  modifyFields(shown, {
    providers: providers =>
      Array.map(providers, entry =>
        entry.id === provider
          ? modifyFields(entry, {
              models: models =>
                Array.map(models, info =>
                  info.id === modelId
                    ? modifyFields(info, { images: () => images })
                    : info,
                ),
            })
          : entry,
      ),
  })

const toggleImages = (
  model: Model,
  provider: string,
  modelId: string,
  images: boolean,
): UpdateReturn =>
  onLoaded(model, loaded => ({
    model: modifyFields(loaded, {
      shown: shown => withImages(shown, provider, modelId, images),
    }),
    commands: [SaveImages({ provider, modelId, images })],
  }))

const contextKey = (provider: string, modelId: string): string =>
  JSON.stringify([provider, modelId])

const parseContextLimit = (input: string): Option.Option<number | null> => {
  const value = input.trim()
  if (value === '') {
    return Option.some(null)
  }
  if (!/^(?:\d+|\d{1,3}(?:,\d{3})+)$/.test(value)) {
    return Option.none()
  }
  const limit = Number(value.replace(/,/g, ''))
  return Number.isSafeInteger(limit) && limit > 0
    ? Option.some(limit)
    : Option.none()
}

const saveContext = (
  model: Model,
  provider: string,
  modelId: string,
): UpdateReturn =>
  onLoaded(model, loaded => {
    const key = contextKey(provider, modelId)
    return Option.match(Record.get(loaded.contextDrafts, key), {
      onNone: () => ({ model }),
      onSome: draft => {
        const parsed = parseContextLimit(draft.value)
        if (Option.isNone(parsed)) {
          return {
            model: modifyFields(loaded, {
              contextDrafts: drafts =>
                Record.set(
                  drafts,
                  key,
                  modifyFields(draft, {
                    status: () => 'Enter a positive integer',
                  }),
                ),
            }),
          }
        }
        const limit = parsed.value
        if (draft.saving) {
          return {
            model: modifyFields(loaded, {
              contextDrafts: drafts =>
                Record.set(
                  drafts,
                  key,
                  modifyFields(draft, {
                    queued: () =>
                      Option.some({ limit, version: draft.version }),
                    status: () => 'Saving…',
                  }),
                ),
            }),
          }
        }
        const info = loaded.server.providers
          .find(entry => entry.id === provider)
          ?.models.find(entry => entry.id === modelId)
        if (info === undefined) {
          return { model }
        }
        if (info.limit === limit) {
          return {
            model: modifyFields(loaded, {
              contextDrafts: drafts =>
                Record.set(
                  drafts,
                  key,
                  modifyFields(draft, {
                    status: () => '',
                  }),
                ),
            }),
          }
        }
        return {
          model: modifyFields(loaded, {
            contextDrafts: drafts =>
              Record.set(
                drafts,
                key,
                modifyFields(draft, {
                  status: () => 'Saving…',
                  saving: () => true,
                  savingVersion: () => draft.version,
                  queued: () => Option.none(),
                }),
              ),
          }),
          commands: [SaveContext({ provider, modelId, limit })],
        }
      },
    })
  })

const finishContext = (
  loaded: LoadedModel,
  provider: string,
  modelId: string,
  draft: typeof ContextDraft.Type,
  status: string,
): UpdateReturn => {
  const key = contextKey(provider, modelId)
  return Option.match(draft.queued, {
    onNone: () => ({
      model: modifyFields(loaded, {
        contextDrafts: drafts =>
          Record.set(
            drafts,
            key,
            modifyFields(draft, {
              saving: () => false,
              status: () =>
                draft.version === draft.savingVersion
                  ? status
                  : draft.status === 'Saving…'
                    ? ''
                    : draft.status,
            }),
          ),
      }),
    }),
    onSome: queued => ({
      model: modifyFields(loaded, {
        contextDrafts: drafts =>
          Record.set(
            drafts,
            key,
            modifyFields(draft, {
              savingVersion: () => queued.version,
              queued: () => Option.none(),
              status: () =>
                draft.version === queued.version
                  ? 'Saving…'
                  : draft.status === 'Saving…'
                    ? ''
                    : draft.status,
            }),
          ),
      }),
      commands: [SaveContext({ provider, modelId, limit: queued.limit })],
    }),
  })
}

const decodeRole = Schema.decodeUnknownOption(Role)

const roles: ReadonlyArray<Role> = ['main', 'compaction', 'utility']

const roleLabels: Record<Role, string> = {
  main: 'Main',
  compaction: 'Compaction',
  utility: 'Utility',
}

const roleSlot = (shown: Models, role: Role): Slot | null =>
  role === 'main' ? shown.main : optionalSlot(shown, role)

const saveRole = (loaded: LoadedModel, role: Role, slot: Slot): UpdateReturn =>
  role === 'main'
    ? selectMain(loaded, slot)
    : selectOptional(loaded, role, slot)

const assignRole = (
  model: Model,
  role: Role,
  provider: string,
  modelId: string,
): UpdateReturn =>
  onLoaded(model, loaded =>
    saveRole(loaded, role, {
      provider,
      model: modelId,
      reasoning: roleSlot(loaded.shown, role)?.reasoning ?? 'medium',
    }),
  )

const selectRoleMenuItem = (
  model: Model,
  role: Role,
  value: string,
): UpdateReturn =>
  onLoaded(model, loaded => {
    if (value === REMOVE) {
      return role === 'main' ? { model } : selectOptional(loaded, role, null)
    }
    const slot = roleSlot(loaded.shown, role)
    return Option.match(decodeReasoning(value), {
      onNone: () => ({ model }),
      onSome: reasoning =>
        slot === null
          ? { model }
          : saveRole(
              loaded,
              role,
              modifyFields(slot, { reasoning: () => reasoning }),
            ),
    })
  })

const menuFor = (loaded: LoadedModel, key: string): Listbox.Model =>
  Option.getOrElse(Record.get(loaded.menus, key), () => Menu.init(key))

const foldMenu = (
  key: string,
  toParentMessage: (message: Listbox.Message) => Message,
  onSelected: (model: Model, value: string) => UpdateReturn,
) =>
  Update.foldChild({
    update: Menu.Menu.update,
    read: (model: Model) =>
      Model.match(model, {
        Loaded: loaded => Option.some(menuFor(loaded, key)),
        Loading: () => Option.none(),
        Failed: () => Option.none(),
      }),
    write: (model: Model, next: Listbox.Model) =>
      Model.match(model, {
        Loaded: loaded =>
          modifyFields(loaded, {
            menus: menus => Record.set(menus, key, next),
          }),
        Loading: () => model,
        Failed: () => model,
      }),
    toParentMessage,
    foldOutMessage: Listbox.OutMessage.match<
      Update.StepWithOutMessage<Model, Message, OutMessage, Api>
    >({
      Selected:
        ({ value }) =>
        model =>
          onSelected(model, value),
    }),
  })

const assignMenuKey = (provider: string, modelId: string): string =>
  `assign:${contextKey(provider, modelId)}`

const roleMenuKey = (role: Role): string => `role:${role}`

const withFilter = (
  model: Model,
  change: (loaded: LoadedModel) => LoadedModel,
): UpdateReturn =>
  onLoaded(model, loaded => ({
    model: modifyFields(change(loaded), { visibleCount: () => PAGE_SIZE }),
  }))

export const update = (model: Model, message: Message) =>
  Message.match<UpdateReturn>(message, {
    SucceededLoadModels: ({ models }) => ({
      model: Model.Loaded({
        server: models,
        shown: models,
        roleStatuses: {},
        contextDrafts: {},
        query: '',
        providerFilter: '',
        isInUseOnly: false,
        visibleCount: PAGE_SIZE,
        menus: {},
      }),
    }),
    FailedLoadModels: ({ error }) => ({
      model: Model.Failed({ error }),
      outMessage: errorToast(error),
    }),
    GotProviderMenuMessage: ({ message }) =>
      foldMenu(
        PROVIDER_MENU,
        next => Message.GotProviderMenuMessage({ message: next }),
        (current, value) =>
          withFilter(current, loaded =>
            modifyFields(loaded, {
              providerFilter: () => (value === ALL ? '' : value),
            }),
          ),
      )(model, message),
    GotAssignMenuMessage: ({ providerId, modelId, message }) =>
      foldMenu(
        assignMenuKey(providerId, modelId),
        next =>
          Message.GotAssignMenuMessage({
            providerId,
            modelId,
            message: next,
          }),
        (current, value) =>
          Option.match(decodeRole(value), {
            onNone: () => ({ model: current }),
            onSome: role => assignRole(current, role, providerId, modelId),
          }),
      )(model, message),
    GotRoleMenuMessage: ({ roleId, message }) =>
      foldMenu(
        roleMenuKey(roleId),
        next => Message.GotRoleMenuMessage({ roleId, message: next }),
        (current, value) => selectRoleMenuItem(current, roleId, value),
      )(model, message),
    SucceededSaveMain: ({ models }) => savedRole(model, models, 'main'),
    FailedSaveMain: ({ error }) => failedRole(model, error, 'main'),
    SucceededSaveOptional: ({ models, role }) => savedRole(model, models, role),
    FailedSaveOptional: ({ error, role }) => failedRole(model, error, role),
    SucceededSaveImages: ({ models }) => ({
      model: withServerModels(model, models),
    }),
    FailedSaveImages: ({ error }) => failedSave(model, error),
    ToggledImages: ({ provider, modelId, images }) =>
      toggleImages(model, provider, modelId, images),
    UpdatedContext: ({ provider, modelId, value }) =>
      onLoaded(model, loaded => {
        const key = contextKey(provider, modelId)
        const previous = Option.getOrElse(
          Record.get(loaded.contextDrafts, key),
          () => ({
            value: '',
            version: 0,
            status: '',
            saving: false,
            savingVersion: 0,
            queued: Option.none(),
          }),
        )
        const version = previous.version + 1
        return {
          model: modifyFields(loaded, {
            contextDrafts: drafts =>
              Record.set(
                drafts,
                key,
                modifyFields(previous, {
                  value: () => value,
                  version: () => version,
                  status: () => '',
                }),
              ),
          }),
        }
      }),
    BlurredContext: ({ provider, modelId }) =>
      saveContext(model, provider, modelId),
    SucceededSaveContext: ({ provider, modelId, models }) =>
      onLoaded(withServerModels(model, models), loaded => {
        const key = contextKey(provider, modelId)
        return Option.match(Record.get(loaded.contextDrafts, key), {
          onNone: () => ({ model: loaded }),
          onSome: draft =>
            finishContext(loaded, provider, modelId, draft, 'Saved'),
        })
      }),
    FailedSaveContext: ({ provider, modelId, error }) =>
      onLoaded(model, loaded => {
        const key = contextKey(provider, modelId)
        return Option.match(Record.get(loaded.contextDrafts, key), {
          onNone: () => ({ model }),
          onSome: draft =>
            finishContext(loaded, provider, modelId, draft, error),
        })
      }),
    UpdatedQuery: ({ query }) =>
      withFilter(model, loaded => modifyFields(loaded, { query: () => query })),
    ToggledInUseOnly: () =>
      withFilter(model, loaded =>
        modifyFields(loaded, { isInUseOnly: current => !current }),
      ),
    ClickedShowMore: () =>
      onLoaded(model, loaded => ({
        model: modifyFields(loaded, {
          visibleCount: count => count + PAGE_SIZE,
        }),
      })),
  })

const switchTrackClassName =
  'group relative inline-flex h-[18px] w-[30px] shrink-0 cursor-pointer items-center rounded-full bg-track shadow-xs motion-safe:transition-colors data-[checked]:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring'

const switchThumbClassName =
  'size-3.5 translate-x-0.5 rounded-full bg-white motion-safe:transition-transform group-data-[checked]:translate-x-[14px]'

type Entry = Readonly<{ provider: string; info: ModelInfo }>

const entriesOf = (shown: Models): ReadonlyArray<Entry> =>
  Array.flatMap(shown.providers, provider =>
    Array.map(provider.models, info => ({ provider: provider.id, info })),
  )

const rolesHeldBy = (shown: Models, entry: Entry): ReadonlyArray<Role> =>
  Array.filter(roles, role => {
    const slot = roleSlot(shown, role)
    return (
      slot !== null &&
      slot.provider === entry.provider &&
      slot.model === entry.info.id
    )
  })

const isMatch = (loaded: LoadedModel, entry: Entry): boolean =>
  (loaded.providerFilter === '' || entry.provider === loaded.providerFilter) &&
  (!loaded.isInUseOnly || rolesHeldBy(loaded.shown, entry).length > 0) &&
  `${entry.info.id} ${entry.provider}`
    .toLowerCase()
    .includes(loaded.query.trim().toLowerCase())

const contextText = (context: number | null): string =>
  Option.match(Option.fromNullishOr(context), {
    onNone: () => '—',
    onSome: value => value.toLocaleString('en-US'),
  })

const imagesSwitch = (
  provider: string,
  info: ModelInfo,
  h: HtmlBuilder<Message>,
): Html =>
  Switch.view(
    {
      id: `models-images-${provider}/${info.id}`,
      isChecked: info.images,
      onToggle: images =>
        Message.ToggledImages({ provider, modelId: info.id, images }),
      toView: attributes =>
        h.div(
          [h.Class('flex shrink-0 items-center justify-end')],
          [
            h.span(
              [...attributes.label, h.Class('sr-only')],
              [`Images for ${info.id}`],
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

const isSaving = (loaded: LoadedModel, role: Role): boolean =>
  loaded.roleStatuses[role] === 'Saving…'

const roleChipView = (
  loaded: LoadedModel,
  role: Role,
  slot: Slot,
  h: HtmlBuilder<Message>,
): Html =>
  h.keyed('span')(
    role,
    [h.Class('inline-flex')],
    [
      Menu.view(
        {
          model: menuFor(loaded, roleMenuKey(role)),
          items: [
            ...Array.map(Reasoning.literals, value => ({
              value,
              label: value,
            })),
            ...(role === 'main'
              ? []
              : [
                  {
                    value: REMOVE,
                    label: 'Remove role',
                    tone: 'danger' as const,
                  },
                ]),
          ],
          selected: slot.reasoning,
          label: `${roleLabels[role]} reasoning: ${slot.reasoning}`,
          triggerClassName:
            'inline-flex h-[22px] cursor-pointer items-center gap-1 rounded-full bg-chip pl-2.5 pr-1.5 text-caption font-medium text-ink hover:bg-hair focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring data-[disabled]:cursor-not-allowed data-[disabled]:opacity-60',
          panelClassName: 'w-32',
          isCompact: true,
          isDisabled: isSaving(loaded, role),
          triggerContent: h.span(
            [h.Class('inline-flex items-center gap-1.5')],
            [
              h.span([], [roleLabels[role]]),
              h.span(
                [h.Class('font-normal text-mute')],
                [isSaving(loaded, role) ? 'Saving…' : slot.reasoning],
              ),
              Icon.chevronDown('size-3 shrink-0'),
            ],
          ),
          toParentMessage: message =>
            Message.GotRoleMenuMessage({ roleId: role, message }),
        },
        h,
      ),
    ],
  )

const assignMenuView = (
  loaded: LoadedModel,
  entry: Entry,
  h: HtmlBuilder<Message>,
): Html => {
  const held = rolesHeldBy(loaded.shown, entry)
  const isBusy = Array.some(roles, role => isSaving(loaded, role))

  return Menu.view(
    {
      model: menuFor(loaded, assignMenuKey(entry.provider, entry.info.id)),
      items: Array.map(roles, role => {
        const holder = roleSlot(loaded.shown, role)
        const isHere = held.includes(role)
        return {
          value: role,
          label: roleLabels[role],
          hint: isHere
            ? 'Current'
            : holder === null
              ? 'Not set'
              : `Replaces ${holder.model}`,
          isDisabled: isHere,
        }
      }),
      label: `Use ${entry.info.id} as a role`,
      triggerClassName:
        'flex size-8 cursor-pointer items-center justify-center rounded-full text-mute hover:bg-hover hover:text-ink focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-expanded:opacity-100 data-[disabled]:cursor-not-allowed data-[disabled]:opacity-40 sm:opacity-0 sm:group-hover:opacity-100',
      panelClassName: 'w-[300px] max-w-[calc(100vw-16px)]',
      placement: 'bottom-end',
      isDisabled: isBusy,
      triggerContent: Icon.plus('size-4'),
      toParentMessage: message =>
        Message.GotAssignMenuMessage({
          providerId: entry.provider,
          modelId: entry.info.id,
          message,
        }),
    },
    h,
  )
}

const contextInputView = (
  loaded: LoadedModel,
  entry: Entry,
  h: HtmlBuilder<Message>,
): Html => {
  const { provider, info } = entry
  const draft = Option.getOrElse(
    Record.get(loaded.contextDrafts, contextKey(provider, info.id)),
    () => ({
      value: info.limit === null ? '' : contextText(info.limit),
      version: 0,
      status: '',
      saving: false,
      savingVersion: 0,
      queued: Option.none(),
    }),
  )
  const descriptionId = `context-${encodeURIComponent(JSON.stringify([provider, info.id]))}`
  const isInvalid =
    draft.status !== '' &&
    draft.status !== 'Saved' &&
    draft.status !== 'Saving…'
  const isCapped = Option.exists(
    parseContextLimit(draft.value),
    limit => limit !== null && info.context !== null && limit > info.context,
  )

  return h.div(
    [h.Class('flex shrink-0 flex-col items-end')],
    [
      h.input([
        h.Class(
          `h-7 w-24 rounded-full border px-3 text-right font-mono tabular-nums placeholder:text-mute focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 ${isInvalid ? 'border-alarm' : 'focus-visible:border-ring'} ${info.limit === null ? 'border-transparent bg-transparent text-ink hover:border-control' : 'border-control bg-white text-ink shadow-xs'}`,
        ),
        h.AriaLabel(`Context override for ${info.id}`),
        h.Attribute('aria-invalid', String(isInvalid)),
        h.Attribute('aria-describedby', descriptionId),
        h.Attribute('inputmode', 'numeric'),
        h.Attribute('placeholder', contextText(info.context)),
        h.Value(draft.value),
        h.OnInput(value =>
          Message.UpdatedContext({ provider, modelId: info.id, value }),
        ),
        h.OnKeyDownPreventDefault(key =>
          key === 'Enter'
            ? Option.some(
                Message.BlurredContext({ provider, modelId: info.id }),
              )
            : Option.none(),
        ),
        h.OnBlur(Message.BlurredContext({ provider, modelId: info.id })),
      ]),
      h.span(
        [
          h.Id(descriptionId),
          h.AriaLive('polite'),
          h.Class(
            `text-right text-caption empty:sr-only ${isInvalid ? 'text-alarm' : 'text-mute'}`,
          ),
        ],
        [
          isCapped
            ? `Effective: ${contextText(info.context)}${draft.status === '' ? '' : `. ${draft.status}`}`
            : draft.status,
        ],
      ),
    ],
  )
}

const gridClassName = (showProvider: boolean): string =>
  showProvider
    ? 'sm:grid-cols-[minmax(0,1fr)_96px_104px_40px_32px]'
    : 'sm:grid-cols-[minmax(0,1fr)_104px_40px_32px]'

const columnHeaderView = (
  showProvider: boolean,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [
      h.Class(
        `hidden h-10 items-center gap-x-3 border-b border-hair px-2 text-body font-medium text-mute sm:grid ${gridClassName(showProvider)}`,
      ),
    ],
    [
      h.span([], ['Model']),
      ...(showProvider ? [h.span([], ['Provider'])] : []),
      h.span([h.Class('text-right')], ['Context override']),
      h.span([h.Class('text-right')], ['Images']),
      h.span([]),
    ],
  )

const entryRowView = (
  loaded: LoadedModel,
  entry: Entry,
  showProvider: boolean,
  h: HtmlBuilder<Message>,
): Html => {
  const held = rolesHeldBy(loaded.shown, entry)

  return h.keyed('li')(
    `${entry.provider}/${entry.info.id}`,
    [
      h.Class(
        `group grid grid-cols-1 items-center gap-x-3 gap-y-1.5 px-2 py-2 hover:bg-hover/60 sm:min-h-12 ${gridClassName(showProvider)}`,
      ),
    ],
    [
      h.div(
        [h.Class('flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1')],
        [
          h.span(
            [h.Class('min-w-0 break-all font-mono text-ink sm:truncate')],
            [entry.info.id],
          ),
          ...Array.flatMap(held, role => {
            const slot = roleSlot(loaded.shown, role)
            return slot === null ? [] : [roleChipView(loaded, role, slot, h)]
          }),
        ],
      ),
      h.div(
        [h.Class('flex items-center justify-end gap-3 sm:contents')],
        [
          ...(showProvider
            ? [
                h.span(
                  [h.Class('mr-auto text-body text-slate sm:mr-0')],
                  [entry.provider],
                ),
              ]
            : []),
          contextInputView(loaded, entry, h),
          imagesSwitch(entry.provider, entry.info, h),
          assignMenuView(loaded, entry, h),
        ],
      ),
    ],
  )
}

const catalogView = (loaded: LoadedModel, h: HtmlBuilder<Message>): Html => {
  const all = entriesOf(loaded.shown)
  const matching = Array.filter(all, entry => isMatch(loaded, entry))
  const visible = Array.take(matching, loaded.visibleCount)
  const remaining = matching.length - visible.length
  const showProvider = loaded.providerFilter === ''
  const offRoles = Array.filter(
    ['compaction', 'utility'] as const,
    role => optionalSlot(loaded.shown, role) === null,
  )

  return h.div(
    [],
    [
      Array.match(matching, {
        onEmpty: () =>
          Layout.emptyLine(
            all.length === 0 ? 'No models available.' : 'No models match.',
            h,
          ),
        onNonEmpty: () =>
          h.div(
            [],
            [
              columnHeaderView(showProvider, h),
              Layout.rowList(
                Array.map(visible, entry =>
                  entryRowView(loaded, entry, showProvider, h),
                ),
                h,
              ),
            ],
          ),
      }),
      remaining > 0
        ? h.div(
            [h.Class('mt-2')],
            [
              Button.view(
                {
                  label: `Show ${Math.min(PAGE_SIZE, remaining)} more`,
                  variant: 'secondary',
                  maybeOnClick: Option.some(Message.ClickedShowMore()),
                },
                h,
              ),
            ],
          )
        : h.empty,
      ...(loaded.isInUseOnly
        ? Array.map(offRoles, role =>
            h.p(
              [h.Class('mt-3 text-body text-mute')],
              [`${roleLabels[role]}: Off`],
            ),
          )
        : []),
    ],
  )
}

const searchInputClassName =
  'h-8 w-full rounded-full border border-control bg-white pl-9 pr-3.5 shadow-xs text-body text-ink placeholder:text-mute focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

const filtersView = (loaded: LoadedModel, h: HtmlBuilder<Message>): Html => {
  const providers = loaded.shown.providers
  const total = Array.reduce(
    providers,
    0,
    (count, provider) => count + provider.models.length,
  )
  const inUseCount = Array.filter(
    entriesOf(loaded.shown),
    entry => rolesHeldBy(loaded.shown, entry).length > 0,
  ).length

  return h.div(
    [h.Class('flex w-full items-center gap-2 md:w-auto')],
    [
      h.div(
        [h.Class('min-w-0 flex-1 md:w-[220px] md:flex-none')],
        [
          UiInput.view(
            {
              id: 'models-search',
              value: loaded.query,
              placeholder: 'Search models',
              onInput: query => Message.UpdatedQuery({ query }),
              toView: attributes =>
                h.div(
                  [h.Class('relative')],
                  [
                    h.span(
                      [
                        h.Class(
                          'pointer-events-none absolute inset-y-0 left-3 flex items-center text-mute',
                        ),
                      ],
                      [Icon.search('size-4')],
                    ),
                    h.input([
                      ...attributes.input,
                      h.AriaLabel('Search models'),
                      h.Class(searchInputClassName),
                    ]),
                  ],
                ),
            },
            h,
          ),
        ],
      ),
      Menu.view(
        {
          model: menuFor(loaded, PROVIDER_MENU),
          items: [
            { value: ALL, label: 'All providers', hint: String(total) },
            ...Array.map(providers, provider => ({
              value: provider.id,
              label: provider.id,
              hint: String(provider.models.length),
            })),
          ],
          selected: loaded.providerFilter === '' ? ALL : loaded.providerFilter,
          label: 'Provider',
          triggerClassName:
            'flex h-8 w-full cursor-pointer items-center justify-between gap-2 rounded-full border border-control bg-white px-3.5 text-left text-body font-medium text-ink shadow-xs hover:bg-hover focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50',
          panelClassName: 'w-44',
          placement: 'bottom-end',
          className: 'w-36 shrink-0 md:w-44',
          triggerContent: h.span(
            [h.Class('flex min-w-0 flex-1 items-center justify-between gap-2')],
            [
              h.span(
                [h.Class('truncate')],
                [
                  loaded.providerFilter === ''
                    ? 'All providers'
                    : loaded.providerFilter,
                ],
              ),
              Icon.chevronDown('size-3.5 shrink-0 text-mute'),
            ],
          ),
          toParentMessage: message =>
            Message.GotProviderMenuMessage({ message }),
        },
        h,
      ),
      h.button(
        [
          h.Type('button'),
          h.AriaPressed(String(loaded.isInUseOnly)),
          h.OnClick(Message.ToggledInUseOnly()),
          h.Class(
            `inline-flex h-8 shrink-0 cursor-pointer items-center gap-1.5 rounded-full px-3.5 text-body font-medium focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring border shadow-xs ${loaded.isInUseOnly ? 'border-accent bg-accent text-white hover:bg-accent/90' : 'border-control bg-white text-ink hover:bg-hover'}`,
          ),
        ],
        [
          'In use',
          h.span(
            [
              h.Class(
                `rounded-full px-1.5 text-[11px] font-medium ${loaded.isInUseOnly ? 'bg-white/20 text-white' : 'bg-chip text-mute'}`,
              ),
            ],
            [String(inUseCount)],
          ),
        ],
      ),
    ],
  )
}

export const view = Submodel.defineView<Model, Message>((model, h) =>
  Layout.page(
    {
      title: 'Models',
      wide: true,
      wideActions: true,
      actions: Model.match<ReadonlyArray<Html>>(model, {
        Loading: () => [],
        Failed: () => [],
        Loaded: loaded => [filtersView(loaded, h)],
      }),
      children: [
        Model.match(model, {
          Loading: () => h.p([h.Class('text-slate')], ['Loading…']),
          Failed: ({ error }) => h.p([h.Class('text-alarm')], [error]),
          Loaded: loaded => catalogView(loaded, h),
        }),
      ],
    },
    h,
  ),
)
