import { Html, inertHtml as ih } from 'foldkit/html'

const stroke = (className: string, d: string): Html =>
  ih.svg(
    [
      ih.AriaHidden(true),
      ih.Class(className),
      ih.Xmlns('http://www.w3.org/2000/svg'),
      ih.Fill('none'),
      ih.ViewBox('0 0 24 24'),
      ih.StrokeWidth('1.6'),
      ih.Stroke('currentColor'),
    ],
    [ih.path([ih.StrokeLinecap('round'), ih.StrokeLinejoin('round'), ih.D(d)])],
  )

export const chevronDown = (className: string): Html =>
  stroke(className, 'M19.5 8.25l-7.5 7.5-7.5-7.5')

export const check = (className: string): Html =>
  stroke(className, 'M4.5 12.75l6 6 9-13.5')

export const trash = (className: string): Html =>
  stroke(
    className,
    'm14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0',
  )

export const refresh = (className: string): Html =>
  stroke(
    className,
    'M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0 3.181 3.183a8.25 8.25 0 0 0 13.803-3.7M4.031 9.865a8.25 8.25 0 0 1 13.803-3.7l3.181 3.182m0-4.991v4.99',
  )

export const plus = (className: string): Html =>
  stroke(className, 'M12 4.5v15m7.5-7.5h-15')

export const signOut = (className: string): Html =>
  stroke(
    className,
    'M15.75 9V5.25A2.25 2.25 0 0 0 13.5 3h-6a2.25 2.25 0 0 0-2.25 2.25v13.5A2.25 2.25 0 0 0 7.5 21h6a2.25 2.25 0 0 0 2.25-2.25V15m3 0 3-3m0 0-3-3m3 3H9',
  )

export const user = (className: string): Html =>
  stroke(
    className,
    'M15.75 6a3.75 3.75 0 11-7.5 0 3.75 3.75 0 017.5 0zM4.5 20.1a7.5 7.5 0 0115 0A17.9 17.9 0 0112 21.75c-2.7 0-5.2-.6-7.5-1.65z',
  )

export const sliders = (className: string): Html =>
  stroke(
    className,
    'M10.5 6h9.75M10.5 6a1.5 1.5 0 11-3 0m3 0a1.5 1.5 0 10-3 0M3.75 6H7.5m3 12h9.75m-9.75 0a1.5 1.5 0 01-3 0m3 0a1.5 1.5 0 00-3 0m-3.75 0H7.5m9-6h3.75m-3.75 0a1.5 1.5 0 01-3 0m3 0a1.5 1.5 0 00-3 0m-9.75 0h9.75',
  )

export const server = (className: string): Html =>
  stroke(
    className,
    'M5 4.5h14A1.5 1.5 0 0120.5 6v3a1.5 1.5 0 01-1.5 1.5H5A1.5 1.5 0 013.5 9V6A1.5 1.5 0 015 4.5zM5 13.5h14a1.5 1.5 0 011.5 1.5v3a1.5 1.5 0 01-1.5 1.5H5A1.5 1.5 0 013.5 18v-3A1.5 1.5 0 015 13.5zM7 7.5h.01M7 16.5h.01',
  )

export const chat = (className: string): Html =>
  stroke(
    className,
    'M21 12c0 4.4-4 8-9 8a9.8 9.8 0 01-2.6-.35A6 6 0 015.4 21a6 6 0 01-.5-.07 4.5 4.5 0 001-2.05c.1-.45-.13-.9-.47-1.22C3.9 16.2 3 14.2 3 12c0-4.4 4-8 9-8s9 3.6 9 8z',
  )

export const terminal = (className: string): Html =>
  stroke(
    className,
    'M3.75 4.5h16.5a.75.75 0 01.75.75v13.5a.75.75 0 01-.75.75H3.75a.75.75 0 01-.75-.75V5.25a.75.75 0 01.75-.75zM7.5 9l3 3-3 3m5.25 0h3.75',
  )

export const link = (className: string): Html =>
  stroke(
    className,
    'M13.19 8.69a4.5 4.5 0 011.24 7.24l-4.5 4.5a4.5 4.5 0 01-6.37-6.37l1.76-1.75m13.35-.62l1.76-1.76a4.5 4.5 0 00-6.37-6.37l-4.5 4.5a4.5 4.5 0 001.24 7.24',
  )

export const key = (className: string): Html =>
  stroke(
    className,
    'M15.75 5.25a3 3 0 013 3m3 0a6 6 0 01-7.03 5.91c-.56-.1-1.16.03-1.56.43L10.5 17.25H8.25v2.25H6v2.25H2.25v-2.82c0-.6.24-1.17.66-1.59l6.5-6.5c.4-.4.53-1 .43-1.56A6 6 0 1121.75 8.25z',
  )

export const search = (className: string): Html =>
  stroke(
    className,
    'M21 21l-5.2-5.2m0 0A7.5 7.5 0 105.2 5.2a7.5 7.5 0 0010.6 10.6z',
  )

export const undo = (className: string): Html =>
  stroke(className, 'M9 15L3 9m0 0l6-6M3 9h12a6 6 0 010 12h-3')
