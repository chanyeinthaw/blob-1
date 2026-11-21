import { Effect, Match, Option, Schema } from 'effect'
import { Command, Submodel, Update } from 'foldkit'
import type { Html, HtmlBuilder } from 'foldkit/html'
import { defineMessageUnion } from 'foldkit/message'
import { defineTaggedUnion } from 'foldkit/schema'
import { modifyFields } from 'foldkit/struct'

import { Dialog, Textarea } from '@foldkit/ui'

import { type Api, request } from '../../api/client'
import { Identity } from '../../api/protocol'
import * as Button from '../../ui/button'
import * as ConfirmDialog from '../../ui/confirmDialog'
import * as Icon from '../../ui/icon'
import * as Layout from '../../ui/layout'
import * as Toast from '../../ui/toast'

const Activity = defineTaggedUnion({
  Idle: {},
  Saving: {},
  Resetting: {},
})
type Activity = typeof Activity.Type

const Content = defineTaggedUnion({
  Loading: {},
  Loaded: {
    savedText: Schema.String,
    draftText: Schema.String,
    draftVersion: Schema.Number,
    hasSaved: Schema.Boolean,
    isDefault: Schema.Boolean,
    maybeError: Schema.Option(Schema.String),
    activity: Activity,
  },
  Failed: { error: Schema.String },
})
type Content = typeof Content.Type
type Loaded = typeof Content.Loaded.Type

export const Model = Schema.Struct({
  content: Content,
  confirm: Dialog.Model,
})
export type Model = typeof Model.Type

export const Message = defineMessageUnion({
  SucceededLoadIdentity: { identity: Identity },
  FailedLoadIdentity: { error: Schema.String },
  UpdatedDraft: { text: Schema.String },
  BlurredDraft: {},
  ElapsedAutosaveDelay: { version: Schema.Number },
  SucceededSaveIdentity: { identity: Identity },
  FailedSaveIdentity: { error: Schema.String },
  ClickedReset: {},
  GotConfirmMessage: { message: Dialog.Message },
  ConfirmedReset: {},
  SucceededResetIdentity: { identity: Identity },
  FailedResetIdentity: { error: Schema.String },
})
export type Message = typeof Message.Type

export const OutMessage = defineMessageUnion({
  ShowedToast: { variant: Toast.ToastVariant, title: Schema.String },
})
export type OutMessage = typeof OutMessage.Type

const LoadIdentity = Command.define('LoadIdentity', {
  messages: [Message.SucceededLoadIdentity, Message.FailedLoadIdentity],
  execute: request('identity.get', {}).pipe(
    Effect.map(identity => Message.SucceededLoadIdentity({ identity })),
    Effect.catch(error =>
      Effect.succeed(Message.FailedLoadIdentity({ error: error.message })),
    ),
  ),
})

const SaveIdentity = Command.define('SaveIdentity', {
  args: { text: Schema.String },
  messages: [Message.SucceededSaveIdentity, Message.FailedSaveIdentity],
  execute: ({ text }) =>
    request('identity.set', { text }).pipe(
      Effect.map(identity => Message.SucceededSaveIdentity({ identity })),
      Effect.catch(error =>
        Effect.succeed(Message.FailedSaveIdentity({ error: error.message })),
      ),
    ),
})

const WaitForAutosave = Command.define('WaitForAutosave', {
  args: { version: Schema.Number },
  messages: [Message.ElapsedAutosaveDelay],
  execute: ({ version }) =>
    Effect.sleep('800 millis').pipe(
      Effect.map(() => Message.ElapsedAutosaveDelay({ version })),
    ),
})

const ResetIdentity = Command.define('ResetIdentity', {
  messages: [Message.SucceededResetIdentity, Message.FailedResetIdentity],
  execute: request('identity.reset', {}).pipe(
    Effect.map(identity => Message.SucceededResetIdentity({ identity })),
    Effect.catch(error =>
      Effect.succeed(Message.FailedResetIdentity({ error: error.message })),
    ),
  ),
})

type UpdateReturn = Update.ReturnWithOutMessage<Model, Message, OutMessage, Api>

const loadedFrom = (identity: Identity): Content =>
  Content.Loaded({
    savedText: identity.text,
    draftText: identity.text,
    draftVersion: 0,
    hasSaved: false,
    isDefault: identity.isDefault,
    maybeError: Option.none(),
    activity: Activity.Idle(),
  })

const setContent = (model: Model, content: Content): Model =>
  modifyFields(model, { content: () => content })

const onLoaded = (
  model: Model,
  f: (loaded: Loaded) => UpdateReturn,
): UpdateReturn =>
  Match.value(model.content).pipe(
    Match.withReturnType<UpdateReturn>(),
    Match.tag('Loaded', f),
    Match.orElse(() => ({ model })),
  )

const isIdle = (loaded: Loaded): boolean =>
  Activity.guards.Idle(loaded.activity)

const canSave = (loaded: Loaded): boolean =>
  loaded.draftText !== loaded.savedText && isIdle(loaded)

const saveDraft = (model: Model): UpdateReturn =>
  onLoaded(model, loaded => {
    if (!canSave(loaded)) {
      return { model }
    }

    if (loaded.draftText.trim() === '') {
      return {
        model: setContent(
          model,
          modifyFields(loaded, {
            maybeError: () => Option.some("Identity can't be blank."),
          }),
        ),
      }
    }

    return {
      model: setContent(
        model,
        modifyFields(loaded, {
          activity: () => Activity.Saving(),
          maybeError: () => Option.none(),
        }),
      ),
      commands: [SaveIdentity({ text: loaded.draftText })],
    }
  })

const startReset = (model: Model): Update.Return<Model, Message, Api> =>
  Match.value(model.content).pipe(
    Match.withReturnType<Update.Return<Model, Message, Api>>(),
    Match.tag('Loaded', loaded => {
      if (!isIdle(loaded)) {
        return { model }
      }

      return {
        model: setContent(
          model,
          modifyFields(loaded, {
            activity: () => Activity.Resetting(),
            draftVersion: version => version + 1,
          }),
        ),
        commands: [ResetIdentity()],
      }
    }),
    Match.orElse(() => ({ model })),
  )

const readConfirm = (model: Model): Option.Option<Dialog.Model> =>
  Option.some(model.confirm)

const writeConfirm = (model: Model, confirm: Dialog.Model): Model =>
  modifyFields(model, { confirm: () => confirm })

const foldConfirmOutMessage = Dialog.OutMessage.match<
  Update.Step<Model, Message>
>({
  Opened: () => model => ({ model }),
  Closed: () => model => ({ model }),
})

const foldConfirmOpen = Update.foldChildStep({
  update: Dialog.open,
  read: readConfirm,
  write: writeConfirm,
  toParentMessage: message => Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const foldConfirmClose = Update.foldChildStep({
  update: Dialog.close,
  read: readConfirm,
  write: writeConfirm,
  toParentMessage: message => Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

const foldConfirmMessage = Update.foldChild({
  update: Dialog.update,
  read: readConfirm,
  write: writeConfirm,
  toParentMessage: message => Message.GotConfirmMessage({ message }),
  foldOutMessage: foldConfirmOutMessage,
})

export const init = (): Update.Return<Model, Message, Api> => ({
  model: {
    content: Content.Loading(),
    confirm: Dialog.init({ id: 'reset-identity' }),
  },
  commands: [LoadIdentity()],
})

export const update = (model: Model, message: Message) =>
  Message.match<UpdateReturn>(message, {
    SucceededLoadIdentity: ({ identity }) => ({
      model: setContent(model, loadedFrom(identity)),
    }),

    FailedLoadIdentity: ({ error }) => ({
      model: setContent(model, Content.Failed({ error })),
      outMessage: OutMessage.ShowedToast({ variant: 'Error', title: error }),
    }),

    UpdatedDraft: ({ text }) =>
      onLoaded(model, loaded => {
        const version = loaded.draftVersion + 1
        return {
          model: setContent(
            model,
            modifyFields(loaded, {
              draftText: () => text,
              draftVersion: () => version,
              hasSaved: () => false,
              maybeError: () =>
                text.trim() === ''
                  ? Option.some("Identity can't be blank.")
                  : Option.none(),
            }),
          ),
          commands: [WaitForAutosave({ version })],
        }
      }),

    BlurredDraft: () => saveDraft(model),

    ElapsedAutosaveDelay: ({ version }) =>
      onLoaded(model, loaded =>
        version === loaded.draftVersion ? saveDraft(model) : { model },
      ),

    SucceededSaveIdentity: ({ identity }) =>
      onLoaded(model, loaded => {
        const nextModel = setContent(
          model,
          modifyFields(loaded, {
            savedText: () => identity.text,
            isDefault: () => identity.isDefault,
            hasSaved: () => loaded.draftText === identity.text,
            activity: () => Activity.Idle(),
          }),
        )
        return loaded.draftText === identity.text
          ? { model: nextModel }
          : saveDraft(nextModel)
      }),

    FailedSaveIdentity: ({ error }) =>
      onLoaded(model, loaded => ({
        model: setContent(
          model,
          modifyFields(loaded, {
            activity: () => Activity.Idle(),
            maybeError: () => Option.some(error),
          }),
        ),
      })),

    ClickedReset: () => foldConfirmOpen(model),

    GotConfirmMessage: ({ message }) => foldConfirmMessage(model, message),

    ConfirmedReset: () => Update.combine(model, [foldConfirmClose, startReset]),

    SucceededResetIdentity: ({ identity }) =>
      onLoaded(model, loaded => ({
        model: setContent(
          model,
          modifyFields(loaded, {
            savedText: () => identity.text,
            draftText: () => identity.text,
            isDefault: () => identity.isDefault,
            hasSaved: () => true,
            maybeError: () => Option.none(),
            activity: () => Activity.Idle(),
          }),
        ),
      })),

    FailedResetIdentity: ({ error }) =>
      onLoaded(model, loaded => ({
        model: setContent(
          model,
          modifyFields(loaded, {
            activity: () => Activity.Idle(),
            maybeError: () => Option.some(error),
          }),
        ),
      })),
  })

const identityTextClassName =
  'col-start-1 row-start-1 min-h-56 w-full px-3.5 py-3 whitespace-pre-wrap break-words text-body leading-relaxed text-ink'

const identityFieldView = (
  loaded: Loaded,
  attributes: Textarea.TextareaAttributes<Message>,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [h.Class('flex flex-col')],
    [
      h.div(
        [
          h.Class(
            `grid grid-cols-1 rounded-2xl border bg-white focus-within:ring-[3px] focus-within:ring-ring/50 ${Option.isSome(loaded.maybeError) ? 'border-alarm' : 'border-control focus-within:border-ring'}`,
          ),
        ],
        [
          h.div(
            [h.AriaHidden(true), h.Class(`${identityTextClassName} invisible`)],
            [`${loaded.draftText} `],
          ),
          h.textarea([
            ...attributes.textarea,
            h.AriaLabel('Identity'),
            h.Class(
              `${identityTextClassName} h-full resize-none overflow-hidden bg-transparent caret-ink outline-none`,
            ),
            h.OnBlur(Message.BlurredDraft()),
            h.Disabled(Activity.guards.Resetting(loaded.activity)),
          ]),
        ],
      ),
      h.p(
        [
          ...attributes.description,
          h.AriaLive('polite'),
          h.Class('mt-2 text-caption text-alarm empty:sr-only'),
        ],
        [Option.getOrElse(loaded.maybeError, () => '')],
      ),
    ],
  )

const loadedView = (loaded: Loaded, h: HtmlBuilder<Message>): Html =>
  Textarea.view(
    {
      id: 'identity',
      value: loaded.draftText,
      onInput: text => Message.UpdatedDraft({ text }),
      isInvalid: Option.isSome(loaded.maybeError),
      hasDescription: true,
      rows: 1,
      toView: attributes => identityFieldView(loaded, attributes, h),
    },
    h,
  )

const statusText = (loaded: Loaded): string =>
  Activity.guards.Saving(loaded.activity)
    ? 'Saving…'
    : loaded.hasSaved && Option.isNone(loaded.maybeError)
      ? 'Saved'
      : ''

const actionsView = (
  model: Model,
  h: HtmlBuilder<Message>,
): ReadonlyArray<Html> =>
  Content.match<ReadonlyArray<Html>>(model.content, {
    Loading: () => [],
    Failed: () => [],
    Loaded: loaded => [
      h.span(
        [
          h.AriaLive('polite'),
          h.Class('mr-1 text-caption text-mute empty:sr-only'),
        ],
        [statusText(loaded)],
      ),
      ...(loaded.isDefault
        ? []
        : [
            Button.iconView(
              {
                label: 'Reset to default',
                icon: Icon.undo('size-4'),
                onClick: Message.ClickedReset(),
                isDisabled: !isIdle(loaded),
              },
              h,
            ),
          ]),
    ],
  })

export const view = Submodel.defineView<Model, Message>((model, h) =>
  Layout.page(
    {
      title: 'Identity',
      actions: actionsView(model, h),
      children: [
        Content.match<Html>(model.content, {
          Loading: () => h.p([h.Class('text-slate')], ['Loading…']),
          Failed: ({ error }) => h.p([h.Class('text-alarm')], [error]),
          Loaded: loaded => loadedView(loaded, h),
        }),
        ConfirmDialog.view(
          {
            model: model.confirm,
            title: 'Reset identity?',
            maybeDetail: 'This replaces your custom identity with the default.',
            confirmLabel: 'Reset',
            onConfirm: Message.ConfirmedReset(),
            toParentMessage: message => Message.GotConfirmMessage({ message }),
          },
          h,
        ),
      ],
    },
    h,
  ),
)
