// The AI watch officer at work, for the person's Inbox (rules 7, docs/design/watch-officer.md): what
// it is deciding now and how long it has, what it just settled and why, how much it settled and
// passed on lately, and, plainly, when its decisions are not landing (every item reaching the
// person at its deadline). So a person whose Inbox is quiet can see that it is quiet because the
// officer is handling things, not because nothing is happening. Display only. Pure (no browser):
// officerDesk.check.ts.

/** One item the officer triaged (`GET /api/triage`), times in the bridge's clock, s. */
export interface TriageRecord {
  item: string
  class: 'held' | 'alert'
  kind: string
  about?: { intent?: number; uids?: (number | string)[]; alert?: string; uid?: number | string; callsign?: string | null; detail?: string; rationale?: { text?: string } | null } | null
  entered_t: number
  stage: 'with_officer' | 'bypassed' | 'handled' | 'escalated' | 'turned_down' | 'untriaged'
  deadline_t?: number
  note?: string
  recommendation?: string | null
  why?: string
  intents?: number[]
  at_t?: number
  ended?: string
}

export interface TriageState {
  officer: string | null
  counts: { on_watch: boolean; with_officer: number; handled: number }
  open: TriageRecord[]
  history: TriageRecord[]
}

export interface Working {
  item: string
  what: string
  /** s left before it reaches the person anyway */
  leftS: number
  /** of the time it had, the part left (0 to 1) */
  left: number
}

export interface Done {
  item: string
  stage: 'handled' | 'turned_down' | 'escalated' | 'untriaged'
  verb: string
  what: string
  /** its note, its first sentence */
  why: string
  atMs: number
}

export interface Desk {
  onWatch: boolean
  working: Working[]
  recent: Done[]
  /** in the last ten minutes */
  settled: number
  passed: number
  untriaged: number
  /** its last items all reached the person at their deadlines, none decided: its verdicts are not
   *  landing (a broken tool, a stuck turn) */
  stalled: boolean
  /** the line under its name */
  headline: string
}

const WINDOW_MS = 600_000
/** this many of its latest items reached the person at their deadlines, and none was decided since */
export const STALL_RUN = 3

const VERB: Record<Done['stage'], string> = { handled: 'Settled', turned_down: 'Turned down', escalated: 'Passed to you', untriaged: 'Ran out of time on' }
const HELD_WORD: Record<string, string> = { deliver: 'delivery', pickup: 'pickup', return_to_hub: 'return', confirm_drop_zone: 'drop zone confirmation' }

/** An item in a few words: "DRONE 14's pickup", "delivery failed, DRONE 10". */
export function itemWords(r: Pick<TriageRecord, 'class' | 'kind' | 'about'>, nameOf: (uid: number | string) => string): string {
  const a = r.about ?? {}
  if (r.class === 'held') {
    const who = a.uids?.length ? a.uids.map(nameOf).join(', ') : null
    const what = HELD_WORD[r.kind] ?? r.kind.replaceAll('_', ' ')
    return who ? `${who}'s ${what}` : `a ${what} order`
  }
  const who = a.callsign ?? (a.uid != null ? nameOf(a.uid) : null)
  return `${r.kind.replaceAll('_', ' ')}${who ? `, ${who}` : ''}`
}

/** A note's first sentence, cut short. */
export function firstSentence(note: string | undefined, max = 120): string {
  const s = (note ?? '').trim().split(/(?<=[.!?])\s/)[0] ?? ''
  return s.length > max ? `${s.slice(0, max - 1).trimEnd()}…` : s
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

export function officerDesk(s: TriageState, nowMs: number, nameOf: (uid: number | string) => string): Desk {
  const all = [...s.open, ...s.history]
  const working: Working[] = all
    .filter((r) => r.stage === 'with_officer' && r.deadline_t != null)
    .map((r) => {
      const span = Math.max(1, (r.deadline_t! - r.entered_t) * 1000)
      const leftMs = Math.max(0, r.deadline_t! * 1000 - nowMs)
      return { item: r.item, what: itemWords(r, nameOf), leftS: Math.round(leftMs / 1000), left: Math.min(1, leftMs / span) }
    })
    .sort((a, b) => a.leftS - b.leftS)
  const finished = all
    .filter((r): r is TriageRecord & { stage: Done['stage'] } => ['handled', 'turned_down', 'escalated', 'untriaged'].includes(r.stage) && r.at_t != null)
    .sort((a, b) => (b.at_t ?? 0) - (a.at_t ?? 0))
  const recent: Done[] = finished.map((r) => ({
    item: r.item,
    stage: r.stage,
    verb: VERB[r.stage],
    what: itemWords(r, nameOf),
    why: r.stage === 'untriaged' ? (r.why === 'officer_absent' ? 'the officer went quiet; it is yours' : 'its deadline passed; it is yours') : firstSentence(r.note),
    atMs: (r.at_t ?? 0) * 1000,
  }))
  const lately = recent.filter((d) => nowMs - d.atMs <= WINDOW_MS)
  const settled = lately.filter((d) => d.stage === 'handled' || d.stage === 'turned_down').length
  const passed = lately.filter((d) => d.stage === 'escalated').length
  const untriaged = lately.filter((d) => d.stage === 'untriaged').length
  const run = recent.slice(0, STALL_RUN)
  const stalled = s.counts.on_watch && run.length === STALL_RUN && run.every((d) => d.stage === 'untriaged')
  let headline: string
  if (!s.counts.on_watch) headline = 'Off watch: everything comes straight to you.'
  else {
    const now = working.length ? `Deciding ${plural(working.length, 'item')} now` : 'Nothing waiting on it'
    const done = settled || passed ? `; in the last 10 min it settled ${settled} and passed ${passed} to you` : lately.length ? '' : '; nothing has come in for 10 min'
    headline = `${now}${done}.`
  }
  return { onWatch: s.counts.on_watch, working, recent, settled, passed, untriaged, stalled, headline }
}
