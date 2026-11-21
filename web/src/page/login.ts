import { Effect, Match, Option, Schema } from 'effect'
import { Command, Submodel, type Update } from 'foldkit'
import { defineMessageUnion } from 'foldkit/message'
import { defineTaggedUnion } from 'foldkit/schema'
import { modifyFields } from 'foldkit/struct'

import * as Session from '../session'
import * as Button from '../ui/button'
import * as Input from '../ui/input'

export const Status = defineTaggedUnion({
  Idle: {},
  Submitting: {},
  Failed: { message: Schema.String },
  Unavailable: {},
})
export type Status = typeof Status.Type

export const Model = Schema.Struct({
  password: Schema.String,
  status: Status,
})
export type Model = typeof Model.Type

export const Message = defineMessageUnion({
  ChangedPassword: { value: Schema.String },
  SubmittedForm: {},
  SucceededSignIn: {},
  FailedSignIn: { message: Schema.String },
  FailedSignInUnavailable: {},
})
export type Message = typeof Message.Type

export const OutMessage = defineMessageUnion({
  SignedIn: {},
})
export type OutMessage = typeof OutMessage.Type

type UpdateReturn = Update.ReturnWithOutMessage<Model, Message, OutMessage>

export const init = (): Update.Return<Model, Message> => ({
  model: { password: '', status: Status.Idle() },
})

const SignIn = Command.define('SignIn', {
  args: { password: Schema.String },
  messages: [
    Message.SucceededSignIn,
    Message.FailedSignIn,
    Message.FailedSignInUnavailable,
  ],
  execute: ({ password }) =>
    Session.signIn(password).pipe(
      Effect.as(Message.SucceededSignIn()),
      Effect.catch(error =>
        Effect.succeed(
          error._tag === 'WrongPassword'
            ? Message.FailedSignIn({ message: error.message })
            : Message.FailedSignInUnavailable(),
        ),
      ),
    ),
})

export const update = (model: Model, message: Message) =>
  Message.match<UpdateReturn>(message, {
    ChangedPassword: ({ value }) => ({
      model: modifyFields(model, {
        password: () => value,
        status: () => Status.Idle(),
      }),
    }),

    SubmittedForm: () =>
      Match.value(model.status).pipe(
        Match.withReturnType<UpdateReturn>(),
        Match.tag('Submitting', () => ({ model })),
        Match.orElse(() => ({
          model: modifyFields(model, { status: () => Status.Submitting() }),
          commands: [SignIn({ password: model.password })],
        })),
      ),

    SucceededSignIn: () => ({
      model,
      outMessage: OutMessage.SignedIn(),
    }),

    FailedSignIn: ({ message }) => ({
      model: modifyFields(model, { status: () => Status.Failed({ message }) }),
    }),

    FailedSignInUnavailable: () => ({
      model: modifyFields(model, { status: () => Status.Unavailable() }),
    }),
  })

const errorOf = (status: Status): Option.Option<string> =>
  Match.value(status).pipe(
    Match.withReturnType<Option.Option<string>>(),
    Match.tag('Failed', ({ message }) => Option.some(message)),
    Match.tag('Unavailable', () => Option.some('Sign in failed. Try again.')),
    Match.orElse(() => Option.none()),
  )

export const view = Submodel.defineView<Model, Message>((model, h) =>
  h.div(
    [h.Class('flex flex-col gap-3')],
    [
      h.form(
        [h.Class('flex flex-col gap-2'), h.OnSubmit(Message.SubmittedForm())],
        [
          h.label(
            [h.For('password'), h.Class('text-body font-medium text-ink')],
            ['Password'],
          ),
          Input.view(
            {
              id: 'password',
              type: 'password',
              autoComplete: 'current-password',
              value: model.password,
              onInput: value => Message.ChangedPassword({ value }),
              maybeError: errorOf(model.status),
              isAutofocus: true,
            },
            h,
          ),
          Button.view(
            {
              label:
                model.status._tag === 'Submitting' ? 'Signing in…' : 'Sign in',
              variant: 'primary',
              type: 'submit',
              isFullWidth: true,
              isDisabled: model.status._tag === 'Submitting',
            },
            h,
          ),
        ],
      ),
    ],
  ),
)
