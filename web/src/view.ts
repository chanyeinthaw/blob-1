import { Match } from 'effect'
import { Document, Html, HtmlBuilder } from 'foldkit/html'

import { Message } from './message'
import { Model, Page } from './model'
import * as Login from './page/login'
import { shellView } from './shell'
import * as Blob from './ui/blob'

const pageTitle = (page: Page): string =>
  Page.match(page, {
    Identity: () => 'Identity',
    Models: () => 'Models',
    Providers: () => 'Providers',
    Chat: () => 'Chat platforms',
    Mcp: () => 'MCP servers',
  })

const signedOutView = (login: Login.Model, h: HtmlBuilder<Message>): Html =>
  h.div(
    [h.Class('grid min-h-dvh place-items-center bg-fog px-5 py-16')],
    [
      h.div(
        [h.Class('flex w-full max-w-[320px] flex-col gap-6')],
        [
          h.div(
            [h.Class('flex flex-col items-center gap-3')],
            [
              h.span(
                [
                  h.Class(
                    'grid size-9 place-items-center rounded-lg bg-accent text-white',
                  ),
                ],
                [Blob.view('size-[18px]', h)],
              ),
              h.h1(
                [h.Class('text-heading font-semibold text-ink')],
                ['Sign in to Blob-1'],
              ),
            ],
          ),
          h.submodel({
            slotId: 'login',
            model: login,
            view: Login.view,
            toParentMessage: message => Message.GotLoginMessage({ message }),
          }),
        ],
      ),
    ],
  )

export const view = (model: Model, h: HtmlBuilder<Message>): Document =>
  Match.value(model).pipe(
    Match.tagsExhaustive({
      SignedOut: signedOut => ({
        title: 'Blob-1',
        body: signedOutView(signedOut.login, h),
      }),
      SignedIn: signedIn => ({
        title: `${pageTitle(signedIn.page)} | Blob-1`,
        body: shellView(signedIn, h),
      }),
    }),
  )
