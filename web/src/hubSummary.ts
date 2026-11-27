// What a hub is doing, said for the person supervising it: its mark on the map (a tone and one line
// under its name) and its details (a few plain sentences and the figures behind them). In the order
// situation awareness asks for it: what needs the person, then whether the hub can fly (the wind
// against the limits of what is based there), then what its vehicles and parcels are doing and
// what comes next. Display only: nothing here decides. Pure (no browser): hubSummary.check.ts.
import { dueText, isLate, type Parcel } from './delivery.ts'
import { type Counts, FLEET_STATES, type FleetBase, type FleetStateId } from './fleetModel.ts'

/** The wind at a hub against the strictest limits of the platforms based there (`weather` of a
 *  base in GET /api/fleet). */
export interface HubWind {
  gust_mps: number | null
  wind_mps: number | null
  /** whose limits these are: the platform the wind holds first */
  platform: string | null
  gust_soft_mps: number | null
  gust_hard_mps: number | null
  wind_soft_mps: number | null
  wind_hard_mps: number | null
}

/** over: what would leave is refused (the hard limit); marginal: held for a decision (the soft) */
export type WindBand = 'ok' | 'marginal' | 'over'

export interface WindJudgement {
  band: WindBand
  /** what decides the band: the gust, or the mean wind when it is the worse */
  by: 'gust' | 'wind'
  value: number
  soft: number | null
  hard: number | null
  /** "drones": whose limits */
  who: string
}

const RANK: Record<WindBand, number> = { ok: 0, marginal: 1, over: 2 }

function band(v: number, soft: number | null, hard: number | null): WindBand {
  if (hard != null && v >= hard) return 'over'
  if (soft != null && v >= soft) return 'marginal'
  return 'ok'
}

/** "delivery_drone" -> "drones" */
export function platformWord(id: string | null): string {
  if (!id) return 'vehicles'
  const w = id.replace(/^delivery_/, '').replace(/_/g, ' ')
  return w.endsWith('s') ? w : `${w}s`
}

/** The band the wind at a hub is in, by the gust and by the mean wind (the worse counts, the gust
 *  on a tie). Null with no sample or no limits to judge it by. */
export function judgeWind(w: HubWind | null | undefined): WindJudgement | null {
  if (!w || w.gust_mps == null || (w.gust_soft_mps == null && w.gust_hard_mps == null)) return null
  const who = platformWord(w.platform)
  const gust: WindJudgement = { band: band(w.gust_mps, w.gust_soft_mps, w.gust_hard_mps), by: 'gust', value: w.gust_mps, soft: w.gust_soft_mps, hard: w.gust_hard_mps, who }
  if (w.wind_mps == null || (w.wind_soft_mps == null && w.wind_hard_mps == null)) return gust
  const mean: WindJudgement = { band: band(w.wind_mps, w.wind_soft_mps, w.wind_hard_mps), by: 'wind', value: w.wind_mps, soft: w.wind_soft_mps, hard: w.wind_hard_mps, who }
  return RANK[mean.band] > RANK[gust.band] ? mean : gust
}

/** What the hard limit does to a hub, in a word: aircraft are grounded, the rest stopped. */
export function stopWord(w: Pick<WindJudgement, 'who'>): string {
  return /drone|plane|copter|vtol/.test(w.who) ? 'Grounded' : 'Stopped'
}

const n = (c: Counts, ...ids: FleetStateId[]) => ids.reduce((s, id) => s + (c[id] ?? 0), 0)
const m1 = (v: number) => v.toFixed(1)

export type HubTone = 'quiet' | 'warn' | 'bad' | 'closed'

export interface HubView {
  id: string
  name: string
  tone: HubTone
  /** under its name on the map: what matters most first, at most three parts */
  line: string
  /** its own vehicles a person should look at: needing attention, or silent */
  needYou: number
  wind: WindJudgement | null
}

type Base = Pick<FleetBase, 'id' | 'name' | 'counts'> & Partial<Pick<FleetBase, 'open' | 'weather' | 'packs' | 'pads' | 'vehicles' | 'here' | 'visitors'>>

/** A hub as its mark on the map says it. */
export function hubView(b: Base): HubView {
  const c = b.counts
  const needYou = n(c, 'attention', 'offline')
  const wind = judgeWind(b.weather)
  const closed = b.open === false
  const holding = n(c, 'holding')
  const parts: string[] = []
  if (needYou) parts.push(`${needYou} need${needYou === 1 ? 's' : ''} you`)
  if (closed) parts.push('Closed')
  else if (wind?.band === 'over') parts.push(`${stopWord(wind)}: ${wind.by === 'gust' ? 'gusts' : 'wind'} ${m1(wind.value)} m/s`)
  else if (wind?.band === 'marginal') parts.push(`Marginal: ${wind.by === 'gust' ? 'gusts' : 'wind'} ${m1(wind.value)} m/s`)
  if (holding) parts.push(`${holding} holding`)
  const out = n(c, 'loading', 'outbound', 'delivering', 'returning')
  if (out) parts.push(`${out} out`)
  const ready = n(c, 'ready')
  if (ready) parts.push(`${ready} ready`)
  if (!parts.length) parts.push(n(c, 'charging') ? `${n(c, 'charging')} charging` : 'no vehicles')
  const tone: HubTone = needYou || wind?.band === 'over' ? 'bad' : closed ? 'closed' : wind?.band === 'marginal' || holding ? 'warn' : 'quiet'
  return { id: b.id, name: b.name || b.id, tone, line: parts.slice(0, 3).join(' · '), needYou, wind }
}

/** A hub's state in two or three words, for its details' header, and their colour: whether it can
 *  fly first, then whether it waits on the person. */
export function stateWords(v: Pick<HubView, 'tone' | 'wind' | 'needYou'>, closed = false): { words: string; tone: 'ok' | 'warn' | 'bad' | 'closed' } {
  if (closed) return { words: 'Closed', tone: 'closed' }
  if (v.wind?.band === 'over') return { words: stopWord(v.wind), tone: 'bad' }
  if (v.wind?.band === 'marginal') return { words: 'Marginal wind', tone: 'warn' }
  if (v.needYou) return { words: 'Needs you', tone: 'bad' }
  if (v.tone === 'warn') return { words: 'Holding', tone: 'warn' }
  return { words: 'Operating normally', tone: 'ok' }
}

/** A site's mark, zoomed out: its hubs together, the worst of them deciding the tone. */
export function siteView(id: string, name: string, hubs: readonly Base[]): HubView {
  const counts: Counts = {}
  for (const h of hubs) for (const [k, v] of Object.entries(h.counts)) counts[k as FleetStateId] = (counts[k as FleetStateId] ?? 0) + (v ?? 0)
  const views = hubs.map(hubView)
  const worst = views.reduce<WindJudgement | null>((w, v) => (v.wind && (!w || RANK[v.wind.band] > RANK[w.band]) ? v.wind : w), null)
  const v = hubView({ id, name, counts, open: hubs.some((h) => h.open !== false), weather: null })
  const parts = v.line.split(' · ')
  if (worst && worst.band !== 'ok') parts.splice(v.needYou ? 1 : 0, 0, `${worst.band === 'over' ? stopWord(worst) : 'Marginal'}: ${worst.by === 'gust' ? 'gusts' : 'wind'} ${m1(worst.value)} m/s`)
  const order: HubTone[] = ['quiet', 'closed', 'warn', 'bad']
  const tone = views.reduce<HubTone>((t, x) => (order.indexOf(x.tone) > order.indexOf(t) ? x.tone : t), v.tone)
  return { ...v, tone, line: parts.slice(0, 3).join(' · '), wind: worst }
}

// ---- the details ------------------------------------------------------------------------------------

export interface FleetSegment {
  state: FleetStateId
  label: string
  n: number
}

export interface HubParcels {
  /** at the hub now: ready, or assigned to a vehicle not yet loaded */
  waiting: number
  late: number
  /** the soonest due of those waiting and not yet late, s from now; null with none */
  nextDueS: number | null
  inTransit: number
  delivered: number
  failed: number
  /** orders not placed yet (their ready time is still to come) */
  later: number
}

export interface HubDetails extends HubView {
  /** the summary, a few plain sentences: the hub's state, what needs the person, its vehicles, its
   *  parcels */
  summary: string[]
  /** its own vehicles by state, problems first then the delivery cycle (the Fleet list's order) */
  fleet: FleetSegment[]
  based: number
  here: number
  visitors: number
  parcels: HubParcels
  pads: number | null
  packs: number | null
}

export function hubParcels(hubId: string, parcels: readonly Parcel[], elapsedS: number): HubParcels {
  const out: HubParcels = { waiting: 0, late: 0, nextDueS: null, inTransit: 0, delivered: 0, failed: 0, later: 0 }
  for (const p of parcels) {
    if (p.from !== hubId) continue
    if (p.state === 'waiting') out.later++
    else if (p.state === 'ready' || p.state === 'assigned') {
      out.waiting++
      if (isLate(p, elapsedS)) out.late++
      else if (out.nextDueS == null || p.due_s - elapsedS < out.nextDueS) out.nextDueS = p.due_s - elapsedS
    } else if (p.state === 'loaded') out.inTransit++
    else if (p.state === 'delivered') out.delivered++
    else if (p.state === 'failed') out.failed++
  }
  return out
}

function list(parts: string[]): string {
  if (parts.length <= 1) return parts.join('')
  return `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}`
}

/** A hub in full, for its details. */
export function hubDetails(b: Base, parcels: readonly Parcel[], elapsedS: number): HubDetails {
  const v = hubView(b)
  const c = b.counts
  const w = v.wind
  const summary: string[] = []
  // whether it can fly
  if (b.open === false) summary.push('Closed: nothing leaves it.')
  else if (w?.band === 'over')
    summary.push(`${w.by === 'gust' ? 'Gusts' : 'Wind'} of ${m1(w.value)} m/s are over the ${w.who}' ${m1(w.hard ?? w.value)} m/s limit: no new delivery leaves, and those in the air are sent home.`)
  else if (w?.band === 'marginal')
    summary.push(`${w.by === 'gust' ? 'Gusts' : 'Wind'} of ${m1(w.value)} m/s are over the ${w.who}' ${m1(w.soft ?? w.value)} m/s soft limit: new deliveries are held for a decision.`)
  else if (w) summary.push(`Operating normally: ${w.by === 'gust' ? 'gusts' : 'wind'} ${m1(w.value)} m/s, under the ${w.who}' ${m1(w.soft ?? w.value)} m/s limit.`)
  else summary.push('Operating normally.')
  // what needs the person
  summary.push(v.needYou ? `${v.needYou} of its vehicles need${v.needYou === 1 ? 's' : ''} you.` : 'Nothing here needs you.')
  // its vehicles
  const doing = [
    [n(c, 'loading', 'outbound', 'delivering'), 'out on deliveries'],
    [n(c, 'returning'), 'returning'],
    [n(c, 'holding'), 'holding'],
    [n(c, 'charging'), 'charging'],
    [n(c, 'ready'), 'ready'],
  ].flatMap(([k, words]) => ((k as number) > 0 ? [`${k} ${words}`] : []))
  if (doing.length) summary.push(`Of its vehicles, ${list(doing)}.`)
  // its parcels
  const p = hubParcels(b.id, parcels, elapsedS)
  if (p.waiting) {
    const late = p.late ? `, ${p.late} late` : ''
    const next = p.nextDueS != null ? `; the next is ${dueText({ state: 'ready', due_s: p.nextDueS }, 0)}` : ''
    summary.push(`${p.waiting} parcel${p.waiting === 1 ? '' : 's'} wait${p.waiting === 1 ? 's' : ''} here${late}${next}.`)
  } else if (p.later) summary.push('No parcel waits here now.')
  const fleet = FLEET_STATES.filter((s) => (c[s.id] ?? 0) > 0).map((s) => ({ state: s.id, label: s.label, n: c[s.id] ?? 0 }))
  return {
    ...v,
    summary,
    fleet,
    based: b.vehicles ?? fleet.reduce((s, x) => s + x.n, 0),
    here: b.here ?? 0,
    visitors: b.visitors ?? 0,
    parcels: p,
    pads: b.pads ?? null,
    packs: b.packs ?? null,
  }
}
