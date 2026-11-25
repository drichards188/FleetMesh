// The top bar's glance (bd murmur-iobd, the cutdown's ledger 1 section 1): the Sitrep's bottom line
// as one quiet line (how many of ours) and one count of ours in trouble, made by the same function
// as each fleet row's and map symbol's exception slot (vehicleException.ts `exceptionOf`), so the
// count and the slots never disagree. Quiet when normal: the trouble count shows only above 0.
//
// Display only. Pure (no browser): topbar.check.ts runs it under node.
import type { Vehicle } from './types'
import { callsign } from './types.ts'
import { exceptionOf, type ExceptionContext, type ExceptionTone } from './vehicleException.ts'

/** Group scale: more of ours than this and the Sitrep's button shows (its report summarises what
 *  the lists cannot be read for at a glance). The open world's 22 vehicles stay under it. */
export const GROUP_SCALE = 40

export function groupScale(vehicles: number): boolean {
  return vehicles > GROUP_SCALE
}

/** Ours in trouble: how many, the worst tone among them (the slot's own), and which, in fleet order. */
export interface Trouble {
  n: number
  tone: ExceptionTone | null
  uids: number[]
  /** for the hover: each one and its slot's words */
  says: string
}

const TONE_RANK: Record<ExceptionTone, number> = { bad: 0, warn: 1, settled: 2 }

/** At most this many named in the hover; the rest counted. */
const NAMED = 6

export function troubleOf(vehicles: readonly Vehicle[], ctx: ExceptionContext): Trouble {
  const uids: number[] = []
  const lines: string[] = []
  let tone: ExceptionTone | null = null
  for (const v of vehicles) {
    const x = exceptionOf(v, ctx)
    // a degraded link (measured loss, an emulator's degradation) is the row's warning, not trouble
    // the whole bar should carry: the vehicle is heard and does what it is told (seen live: 12 to
    // 20 % loss on a busy host kept the bar amber with nothing to act on)
    if (!x || x.kind === 'degraded') continue
    uids.push(v.uid)
    if (lines.length < NAMED) lines.push(`${callsign(v)}: ${x.text.toLowerCase()}`)
    if (tone == null || TONE_RANK[x.tone] < TONE_RANK[tone]) tone = x.tone
  }
  const more = uids.length - lines.length
  const says = uids.length
    ? `${uids.length} of ours in trouble: ${lines.join('; ')}${more > 0 ? `; ${more} more` : ''}. A click shows the next one in the Fleet list.`
    : 'None of ours in trouble.'
  return { n: uids.length, tone, uids, says }
}

/** The one in trouble a click shows: the one after `last` in fleet order, from the first again
 *  after the last; null with none. */
export function nextTrouble(uids: readonly number[], last: number | null): number | null {
  if (!uids.length) return null
  const k = last == null ? -1 : uids.indexOf(last)
  return uids[(k + 1) % uids.length]
}

/** The glance line: how many of ours. */
export interface Glance {
  ours: number
}

/** The one after `last` in a list, from the first again after the last; null with none: a click
 *  on a problem count shows the next one it counts. */
export function nextOf<T>(list: readonly T[], last: T | null): T | null {
  if (!list.length) return null
  const k = last == null ? -1 : list.indexOf(last)
  return list[(k + 1) % list.length]
}

/** One piece of the glance line: a count and the words after it, or the punctuation between two.
 *  A problem (`trouble`) links to what it counts; a total (`ours`) does not. */
export interface GlancePiece {
  kind: 'ours' | 'trouble' | 'sep'
  /** the count (none on punctuation) */
  n?: number
  /** the words after the count, or the punctuation itself */
  text: string
}

/** The glance line in words (the cockpit layout's top bar): "105 vehicles · 3 of ours in trouble".
 *  Folded, for a bar with no room for all of it, only the problem: "3 of ours in trouble", and
 *  nothing while nothing is wrong. */
export function glanceWords(g: Glance, trouble: number, folded: boolean): GlancePiece[] {
  const out: GlancePiece[] = folded ? [] : [{ kind: 'ours', n: g.ours, text: g.ours === 1 ? 'vehicle' : 'vehicles' }]
  if (trouble > 0) {
    if (out.length) out.push({ kind: 'sep', text: ' · ' })
    out.push({ kind: 'trouble', n: trouble, text: 'of ours in trouble' })
  }
  return out
}

/** Where a situation stands, as the top bar says it beside the run's title. */
export interface RunStanding {
  label: string
  /** the hover: what it means for the person */
  title: string
  /** quiet, or amber when the run is not as it should be (its runner silent, or failed) */
  tone: 'quiet' | 'warn'
}

/** A situation's standing (its runner's phase and modes, `GET /api/situation`): who decides while it
 *  runs ("You decide", "Autoplay"), its phase when it does not ("Starting", "Ended",
 *  "Stopped", "Failed"), "Not reporting" once its runner has gone quiet. */
export function runStanding(r: { operator: 'live' | 'autoplay'; phase: string; stale: boolean }): RunStanding {
  if (r.stale) return { label: 'Not reporting', tone: 'warn', title: "The situation's runner has stopped reporting: what shows is its last status. The stack may still be running." }
  if (r.phase === 'starting') return { label: 'Starting', tone: 'quiet', title: 'The situation is starting: its stack, its fleet and its opening orders' }
  if (r.phase === 'ended') return { label: 'Ended', tone: 'quiet', title: "The situation's time is up. Its stack stays up for you to look around until you end it." }
  if (r.phase === 'interrupted') return { label: 'Stopped', tone: 'quiet', title: 'The situation was stopped before its time was up. Its stack stays up until you end it.' }
  if (r.phase === 'failed') return { label: 'Failed', tone: 'warn', title: 'The situation failed: the Situation panel says why' }
  return r.operator === 'live'
    ? { label: 'You decide', tone: 'quiet', title: 'You decide at this console: every approval, review and signature asked of you is yours' }
    : { label: 'Autoplay', tone: 'quiet', title: 'Scripts decide: a scripted operator approves and scripted signers sign, and you watch. Scripts, never people.' }
}
