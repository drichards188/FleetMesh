// BLUF (bottom line up front): how the console turns text into a headline, a few bullets and the
// rest behind "More" (Bluf.tsx draws it). Pure functions, checked by bluf.check.ts.
//
// Three sources:
//  - an agent's rationale, by a convention it is asked to follow (parseRationale): the first line
//    is the headline, then "- " bullets, then a blank line and any detail. The logged rationale
//    stays the plain string it always was; an older free-text one falls back to its first
//    sentence and the rest behind More.
//  - an engine or bridge reason (reasonBluf): splits on "; ".
//  - a checks list (checksSummary).

export interface BlufParts {
  headline: string
  bullets: string[]
  /** the rest, shown behind "More"; empty when there is none */
  more: string
}

/** The longest headline shown whole (characters); longer ones are cut at a sentence or a word. */
export const HEADLINE_MAX = 140
/** The most bullets shown before More. */
export const BULLETS_MAX = 4

const clean = (s: string) => s.replace(/\s+/g, ' ').trim()

/** The first sentence of `s` (to a full stop, "?" or "!" followed by a space), or all of `s`. */
function firstSentence(s: string): { first: string; rest: string } {
  const m = /^(.+?[.!?])\s+(?=\S)/s.exec(s)
  if (m && m[1].length <= HEADLINE_MAX) return { first: m[1], rest: s.slice(m[0].length) }
  return { first: s, rest: '' }
}

/** `s` as a headline of at most HEADLINE_MAX: the whole, its first sentence, or a cut at a word
 *  (then `rest` is the whole, so nothing is lost). */
function cutHeadline(s: string): { first: string; rest: string } {
  if (s.length <= HEADLINE_MAX) return { first: s, rest: '' }
  const s1 = firstSentence(s)
  if (s1.rest) return s1
  const cut = s.lastIndexOf(' ', HEADLINE_MAX - 1)
  const at = cut > HEADLINE_MAX / 2 ? cut : HEADLINE_MAX - 1
  return { first: `${s.slice(0, at).trimEnd()}…`, rest: s }
}

function capBullets(bullets: string[], more: string): { bullets: string[]; more: string } {
  if (bullets.length <= BULLETS_MAX) return { bullets, more }
  const extra = bullets
    .slice(BULLETS_MAX)
    .map((b) => `- ${b}`)
    .join('\n')
  return { bullets: bullets.slice(0, BULLETS_MAX), more: more ? `${extra}\n\n${more}` : extra }
}

const BULLET = /^\s*[-*•]\s+\S/

/** A rationale in the convention (headline, "- " bullets, blank line, detail) or free text. */
export function parseRationale(text: string): BlufParts {
  const t = text.replace(/\r\n/g, '\n').trim()
  if (!t) return { headline: '', bullets: [], more: '' }
  const lines = t.split('\n')
  const head = lines[0].trim()
  let k = 1
  const bullets: string[] = []
  while (k < lines.length && BULLET.test(lines[k])) {
    bullets.push(lines[k].replace(/^\s*[-*•]\s+/, '').trim())
    k++
  }
  const rest = lines.slice(k).join('\n').trim()
  // the convention: a headline line alone, or followed by bullets or by a blank line
  const conventional = !BULLET.test(head) && (lines.length === 1 || bullets.length > 0 || (lines[1] ?? '').trim() === '')
  if (conventional) {
    const h = cutHeadline(clean(head))
    const more = [h.rest, rest].filter(Boolean).join('\n\n')
    return { headline: h.first, ...capBullets(bullets, more) }
  }
  // free text (an older rationale, or a paragraph wrapped over lines): its first sentence, the rest behind More
  const flat = clean(t)
  const s = firstSentence(flat)
  if (s.rest) return { headline: s.first, bullets: [], more: s.rest }
  const h = cutHeadline(flat)
  return { headline: h.first, bullets: [], more: h.rest ? t : '' }
}

// ---- engine and bridge reasons ----------------------------------------------------------------


export interface ReasonContext {
  /** the vehicle the reason is about, by its callsign ("DRONE 1") */
  who?: string
}

/** An engine or bridge reason as a headline, bullets and the original behind More. A reason
 *  splits on "; " (the first part is the headline). A reason with nothing to split is its own headline, with no More. */
export function reasonBluf(text: string | null | undefined, _ctx: ReasonContext = {}): BlufParts {
  const t = (text ?? '').trim()
  if (!t) return { headline: '', bullets: [], more: '' }
  const parts = t.split(/;\s+/).map((s) => s.trim()).filter(Boolean)
  const h = cutHeadline(parts[0] ?? t)
  const bullets = parts.slice(1)
  if (bullets.length === 0 && !h.rest) return { headline: h.first, bullets: [], more: '' }
  const c = capBullets(bullets, '')
  return { headline: h.first, bullets: c.bullets, more: t }
}

/** A situation's watch item: its written headline and bullets with the full text as More, or,
 *  without them, the text's first sentence and the rest. */
export function watchParts(w: { text: string; headline?: string; bullets?: string[] }): BlufParts {
  if (w.headline) return { headline: w.headline, bullets: w.bullets ?? [], more: w.text }
  return parseRationale(w.text)
}

// ---- alerts and review items ------------------------------------------------------------------

/** What an alert asks of the operator, by its kind, in plain words (the card's header already
 *  names the kind and the vehicle). The bridge's own detail follows as bullets, whole under More. */
const ALERT_HEADLINE: Record<string, string> = {
  lost_link: 'No telemetry: its link is down',
  not_moving: 'Not moving as ordered: look at it',
  left_task: 'Left its task: look at it, re-order or let it go',
  task_failed: 'Order failed: the vehicle is not carrying it out',
  ack_timeout: 'No acknowledgement: it may not have the order',
  may_be_running: 'The order may still be running: decide what to do',
  restore_pending: 'Its own failsafe values are not restored: wait or look at it',
  path_crosses_land: 'Its way home crosses land: watch it',
  node_lost: 'For information: confirmed lost; it is out of the keep-outs and takes no order',
  node_reported_again: 'Confirmed lost, but it reported again: the loss is revoked, it counts again',
  nav_monitor: 'Its navigation is untrusted: the monitor proposed a safe order for it; look at it',
  drop_not_clear: 'Not delivered: its drop zone did not look clear, and automation kept the parcel',
  zone_not_approved: 'Parcels wait for a drop zone nobody approved',
  return_failed: 'It could not be sent home: automation stopped trying; send it yourself',
  delivery_failed: 'A delivery failed its tries: automation gave the parcel up',
}

/** An alert as headline (what it asks), the bridge's detail as up to three bullets, and the
 *  detail whole under More when it was cut. `lost_link` says how long, from the age. */
export function alertParts(a: { kind: string; detail: string }, ageText?: string, who?: string): BlufParts {
  if (a.kind === 'lost_link' && ageText) return { headline: `${who ? `${who} has had no` : 'No'} telemetry for ${ageText}: its link is down`, bullets: [], more: '' }
  const headline = ALERT_HEADLINE[a.kind] ?? ''
  if (a.kind === 'lost_link') return { headline, bullets: [], more: '' }
  const d = reasonBluf(a.detail)
  if (!headline) return d
  const bullets = [d.headline, ...d.bullets].filter(Boolean)
  const c = capBullets(bullets.slice(0, 3), '')
  return { headline, bullets: c.bullets, more: bullets.length > 3 || d.more ? a.detail : '' }
}

// ---- checks ---------------------------------------------------------------------------------------

export type Outcome = 'pass' | 'fail' | 'unverified' | 'not_evaluated' | 'hold'

/** The one line a checks list collapses to: "7 checks passed", or what did not pass first. */
export function checksSummary(checks: { rule: string; outcome: Outcome }[]): { text: string; tone: 'ok' | 'bad' | 'warn' } {
  const n = checks.length
  if (n === 0) return { text: 'no checks', tone: 'ok' }
  const by = (o: Outcome) => checks.filter((c) => c.outcome === o)
  const fail = by('fail')
  const unv = by('unverified')
  const held = by('hold')
  const ne = by('not_evaluated')
  const names = (cs: { rule: string }[]) => [...new Set(cs.map((c) => c.rule))].join(', ')
  if (fail.length) {
    const rest = [unv.length ? `${unv.length} unverified` : '', ne.length ? `${ne.length} not evaluated` : ''].filter(Boolean)
    return { text: `${fail.length} failed: ${names(fail)}${rest.length ? `; ${rest.join(', ')}` : ''}`, tone: 'bad' }
  }
  if (held.length) return { text: `${held.length} to judge: ${names(held)}; a person decides`, tone: 'warn' }
  if (unv.length) return { text: `${unv.length} unverified: ${names(unv)}; judge ${unv.length === 1 ? 'it' : 'them'} yourself`, tone: 'warn' }
  if (ne.length) return { text: `${n - ne.length} of ${n} checks passed, ${ne.length} not evaluated: ${names(ne)}`, tone: 'warn' }
  return { text: `${n} check${n === 1 ? '' : 's'} passed`, tone: 'ok' }
}
