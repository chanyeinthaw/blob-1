import type { Weekday } from '@blob-1/contracts'

export interface ZonedDate {
  readonly year: number
  readonly month: number // 1..12
  readonly day: number // 1..31
  readonly hour: number // 0..23
  readonly minute: number // 0..59
  readonly weekday: Weekday
  readonly key: string // YYYY-MM-DD
}

function parseShortWeekday(short: string): Weekday {
  switch (short) {
    case 'Tue':
      return 'tue'
    case 'Wed':
      return 'wed'
    case 'Thu':
      return 'thu'
    case 'Fri':
      return 'fri'
    case 'Sat':
      return 'sat'
    case 'Sun':
      return 'sun'
    default:
      return 'mon'
  }
}

function weekdayOffset(day: Weekday): number {
  switch (day) {
    case 'sun':
      return 0
    case 'mon':
      return 1
    case 'tue':
      return 2
    case 'wed':
      return 3
    case 'thu':
      return 4
    case 'fri':
      return 5
    case 'sat':
      return 6
  }
}

export function pad2(n: number): string {
  return String(n).padStart(2, '0')
}

export function dateKey(year: number, month: number, day: number): string {
  return `${year}-${pad2(month)}-${pad2(day)}`
}

/** Converts a unix timestamp (seconds) into calendar parts in `timeZone`. */
export function toZonedDate(unixSeconds: number, timeZone: string): ZonedDate {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    weekday: 'short',
    hourCycle: 'h23',
  }).formatToParts(new Date(unixSeconds * 1000))

  const get = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find(part => part.type === type)?.value ?? ''

  const year = Number(get('year'))
  const month = Number(get('month'))
  const day = Number(get('day'))
  const hour = Number(get('hour')) % 24
  const minute = Number(get('minute'))
  const weekday = parseShortWeekday(get('weekday'))
  return { year, month, day, hour, minute, weekday, key: dateKey(year, month, day) }
}

export interface DayCell {
  readonly year: number
  readonly month: number
  readonly day: number
  readonly weekday: Weekday
  readonly weekdayShort: string
  readonly key: string
  readonly inMonth: boolean
  readonly isToday: boolean
}

const WEEKDAY_BY_UTC_DAY: ReadonlyArray<{ readonly id: Weekday; readonly short: string }> = [
  { id: 'sun', short: 'Sun' },
  { id: 'mon', short: 'Mon' },
  { id: 'tue', short: 'Tue' },
  { id: 'wed', short: 'Wed' },
  { id: 'thu', short: 'Thu' },
  { id: 'fri', short: 'Fri' },
  { id: 'sat', short: 'Sat' },
]

function makeCell(utcDate: Date, activeMonth: number, todayKey: string): DayCell {
  const year = utcDate.getUTCFullYear()
  const month = utcDate.getUTCMonth() + 1
  const day = utcDate.getUTCDate()
  const wd = WEEKDAY_BY_UTC_DAY[utcDate.getUTCDay()] ?? { id: 'mon', short: 'Mon' }
  const key = dateKey(year, month, day)
  return {
    year,
    month,
    day,
    weekday: wd.id,
    weekdayShort: wd.short,
    key,
    inMonth: month === activeMonth,
    isToday: key === todayKey,
  }
}

export function addDays(
  anchor: { readonly year: number; readonly month: number; readonly day: number },
  delta: number,
) {
  const d = new Date(Date.UTC(anchor.year, anchor.month - 1, anchor.day + delta))
  return { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() }
}

export function addMonths(
  anchor: { readonly year: number; readonly month: number },
  delta: number,
) {
  const d = new Date(Date.UTC(anchor.year, anchor.month - 1 + delta, 1))
  return { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: 1 }
}

/** Returns the 7 Monday-start days containing `anchor`. */
export function buildWeekCells(
  anchor: { readonly year: number; readonly month: number; readonly day: number },
  todayKey: string,
): ReadonlyArray<DayCell> {
  const anchorUtc = new Date(Date.UTC(anchor.year, anchor.month - 1, anchor.day))
  const wd = WEEKDAY_BY_UTC_DAY[anchorUtc.getUTCDay()]?.id ?? 'mon'
  const mondayOffset = -weekdayOffset(wd)
  return Array.from({ length: 7 }, (_, i) => {
    const d = new Date(Date.UTC(anchor.year, anchor.month - 1, anchor.day + mondayOffset + i))
    return makeCell(d, anchor.month, todayKey)
  })
}

/** Returns a 6x7 (or 5x7) Monday-start month grid containing `anchor`. */
export function buildMonthCells(
  anchor: { readonly year: number; readonly month: number },
  todayKey: string,
): ReadonlyArray<DayCell> {
  const first = new Date(Date.UTC(anchor.year, anchor.month - 1, 1))
  const wd = WEEKDAY_BY_UTC_DAY[first.getUTCDay()]?.id ?? 'mon'
  const startOffset = -weekdayOffset(wd)
  const lastDay = new Date(Date.UTC(anchor.year, anchor.month, 0)).getUTCDate()
  const totalSlots = Math.ceil((lastDay - startOffset) / 7) * 7
  return Array.from({ length: totalSlots }, (_, i) => {
    const d = new Date(Date.UTC(anchor.year, anchor.month - 1, 1 + startOffset + i))
    return makeCell(d, anchor.month, todayKey)
  })
}

export interface UtcRange {
  readonly start: number
  readonly end: number
}

/** Wide UTC range (with +/- 1 day padding for any IANA offset) covering `cells`. */
export function cellsUtcRange(cells: ReadonlyArray<DayCell>): UtcRange {
  const first = cells[0]
  const last = cells.at(-1)
  if (first === undefined || last === undefined) {
    const now = Math.floor(Date.now() / 1000)
    return { start: now - 86400, end: now + 7 * 86400 }
  }
  const start = Math.floor(Date.UTC(first.year, first.month - 1, first.day - 1) / 1000)
  const end = Math.floor(Date.UTC(last.year, last.month - 1, last.day + 2) / 1000)
  return { start, end }
}

const SHORT_MONTHS = [
  'Jan',
  'Feb',
  'Mar',
  'Apr',
  'May',
  'Jun',
  'Jul',
  'Aug',
  'Sep',
  'Oct',
  'Nov',
  'Dec',
]
const LONG_MONTHS = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
]

export function formatWeekTitle(cells: ReadonlyArray<DayCell>): string {
  const first = cells[0]
  const last = cells.at(-1)
  if (first === undefined || last === undefined) return ''
  const m1 = SHORT_MONTHS[first.month - 1] ?? ''
  const m2 = SHORT_MONTHS[last.month - 1] ?? ''
  if (first.month === last.month) {
    return `${m1} ${first.day} – ${last.day}, ${last.year}`
  }
  if (first.year === last.year) {
    return `${m1} ${first.day} – ${m2} ${last.day}, ${last.year}`
  }
  return `${m1} ${first.day}, ${first.year} – ${m2} ${last.day}, ${last.year}`
}

export function formatMonthTitle(anchor: {
  readonly year: number
  readonly month: number
}): string {
  return `${LONG_MONTHS[anchor.month - 1] ?? ''} ${anchor.year}`
}

export function formatSlotLocal(cell: DayCell, hour: number): string {
  return `${cell.key}T${pad2(hour)}:00`
}
