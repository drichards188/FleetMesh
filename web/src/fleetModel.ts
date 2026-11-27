// The fleet organised as delivery operators organise theirs (docs/design/fleet-organisation.md):
// every vehicle belongs to a home base (a hub: Wing's nests, Zipline's hubs and docks, a robotaxi
// fleet's depots), bases that work together at one place form a site, and each vehicle is in one
// operational state that the bridge derives (`GET /api/fleet`, the bridge's `decision/fleet.rs`).
// The Fleet list (FleetPanel.tsx) and the map's base markers (fleetMap.ts) are organised by it.
//
// A bridge without `/api/fleet` (one started before it existed) gets a coarser view derived here
// from what the console has: the link, the orders and where each vehicle is (`fallbackFleet`),
// marked `derived`. Display only: nothing here decides or orders anything. Pure (no browser):
// fleetModel.check.ts runs it under node.
import type { Hub } from './delivery'
import type { HubWind } from './hubSummary'
import type { OrderRef } from './orderWords'
import type { LngLat, Vehicle } from './types'
import { isStale } from './types.ts'

export type FleetStateId =
  | 'attention'
  | 'offline'
  | 'out_of_service'
  | 'holding'
  | 'ready'
  | 'loading'
  | 'outbound'
  | 'delivering'
  | 'returning'
  | 'charging'
  | 'tasked'

export interface FleetStateDef {
  id: FleetStateId
  label: string
  /** a state the person should look at: listed first, whatever the vehicle's base */
  problem: boolean
  means: string
}

/** The states, problems first, then a delivery's cycle in order (as the bridge lists them). */
export const FLEET_STATES: FleetStateDef[] = [
  { id: 'attention', label: 'Needs attention', problem: true, means: 'an order of it waits for a decision, an alert or an issue is open about it, or it stopped away from a hub with no order' },
  { id: 'offline', label: 'Offline', problem: true, means: 'no telemetry for longer than its link allows' },
  { id: 'out_of_service', label: 'Out of service', problem: true, means: 'confirmed lost, or damaged so it cannot move' },
  { id: 'holding', label: 'Holding', problem: false, means: 'automation is waiting something out for it and will go on by itself' },
  { id: 'ready', label: 'Ready', problem: false, means: 'at a hub, charged, with no order: free for the dispatcher' },
  { id: 'loading', label: 'Loading', problem: false, means: 'picking up its parcel, or loaded and about to leave' },
  { id: 'outbound', label: 'Outbound', problem: false, means: 'on its way to the drop zone' },
  { id: 'delivering', label: 'Delivering', problem: false, means: 'at the drop zone: hovering, looking, letting go' },
  { id: 'returning', label: 'Returning', problem: false, means: 'on its way back to a hub' },
  { id: 'charging', label: 'Charging', problem: false, means: 'having its battery swapped, or due a swap' },
  { id: 'tasked', label: 'On another order', problem: false, means: "running an order that is not the dispatcher's" },
]

export const STATE_IDS: FleetStateId[] = FLEET_STATES.map((s) => s.id)

/** Each state in a word, for a base's counts where room is short ("9 attention, 15 ready"). */
export const STATE_WORD: Record<FleetStateId, string> = {
  attention: 'attention',
  offline: 'offline',
  out_of_service: 'down',
  holding: 'holding',
  ready: 'ready',
  loading: 'loading',
  outbound: 'outbound',
  delivering: 'dropping',
  returning: 'returning',
  charging: 'charging',
  tasked: 'other',
}
const RANK = new Map<FleetStateId, number>(STATE_IDS.map((id, i) => [id, i]))
/** a state's place in the order (unknown last) */
export const stateRank = (id: string) => RANK.get(id as FleetStateId) ?? STATE_IDS.length

export type Counts = Partial<Record<FleetStateId, number>>

export interface FleetVehicle {
  uid: number
  /** its home base (a hub id), or none */
  base: string | null
  /** the hub it is at now, if any */
  at_hub: string | null
  state: FleetStateId
  why: string
  since_s: number
  parcel: string | null
}

export interface FleetBase {
  id: string
  name: string
  at: LngLat
  radius_m: number
  site: string | null
  packs?: number
  pads?: number
  /** false: closed, nothing leaves it */
  open?: boolean
  /** the wind there against the strictest limits of the platforms based there (null: no
   *  scenario weather in this run) */
  weather?: HubWind | null
  /** its own vehicles (based here) */
  vehicles: number
  counts: Counts
  /** at it now, its own and visitors */
  here: number
  visitors: number
  /** at it now, by state */
  parked: Counts
}

export interface FleetSite {
  id: string
  name: string
  hubs: string[]
}

export interface FleetDoc {
  states: FleetStateDef[]
  sites: FleetSite[]
  bases: FleetBase[]
  vehicles: FleetVehicle[]
  counts: Counts
  /** derived in the console from a bridge without /api/fleet: coarser */
  derived?: boolean
}

export const EMPTY_FLEET: FleetDoc = { states: FLEET_STATES, sites: [], bases: [], vehicles: [], counts: {} }

/** A state's definition: the bridge's when it sent them, else the console's. */
export function stateDef(doc: Pick<FleetDoc, 'states'>, id: string): FleetStateDef {
  return doc.states.find((s) => s.id === id) ?? FLEET_STATES.find((s) => s.id === id) ?? { id: id as FleetStateId, label: id.replace(/_/g, ' '), problem: false, means: '' }
}

export const isProblem = (id: string) => FLEET_STATES.find((s) => s.id === id)?.problem ?? false

/** A vehicle's type for the filter, from its platform: "drone", "car", "boat", "plane". */
export function typeOf(v: Pick<Vehicle, 'identity' | 'kind'>): string {
  const p = v.identity?.platform
  if (p) return p.replace(/^delivery_/, '')
  return v.kind ?? 'unknown'
}

/** "9 ready, 2 charging": the counts in state order, problems first, the first `max` of them
 *  (the rest as "3 more"). */
export function countsText(counts: Counts, max = 3, defs: FleetStateDef[] = FLEET_STATES): string {
  const parts = STATE_IDS.filter((id) => (counts[id] ?? 0) > 0).map((id) => `${counts[id]} ${(defs.find((d) => d.id === id) ?? stateDef({ states: defs }, id)).label.toLowerCase()}`)
  if (parts.length <= max) return parts.join(', ')
  const rest = STATE_IDS.filter((id) => (counts[id] ?? 0) > 0).slice(max).reduce((n, id) => n + (counts[id] ?? 0), 0)
  return `${parts.slice(0, max).join(', ')}, ${rest} more`
}

export function countOf(vs: readonly Pick<FleetVehicle, 'state'>[]): Counts {
  const c: Counts = {}
  for (const v of vs) c[v.state] = (c[v.state] ?? 0) + 1
  return c
}

// ---- the console's own view, for a bridge without /api/fleet --------------------------------

/** a battery under this at a hub is due a swap (the dispatcher's threshold) */
export const SWAP_BELOW_PCT = 45
/** within this of a delivery's drop zone it is delivering, m (the bridge's AT_ZONE_M) */
export const AT_ZONE_M = 80

/** How far from a hub's point a vehicle still counts as at it (the bridge's `hub_catchment`). */
export const catchment = (radiusM: number) => Math.max(radiusM * 3, 150)

export function metres(a: LngLat, b: LngLat): number {
  const k = Math.cos((((a[1] + b[1]) / 2) * Math.PI) / 180)
  return Math.hypot((a[0] - b[0]) * 111_320 * k, (a[1] - b[1]) * 111_320)
}

/** The open hub nearest `at` within its catchment, if any. */
export function hubAt(at: LngLat | null, hubs: readonly Hub[]): Hub | null {
  if (!at) return null
  let best: { h: Hub; d: number } | null = null
  for (const h of hubs) {
    if (!h.open) continue
    const d = metres(at, h.at)
    if (!best || d < best.d || (d === best.d && h.id < best.h.id)) best = { h, d }
  }
  return best && best.d <= catchment(best.h.radius_m) ? best.h : null
}

const ENDED = new Set(['done', 'failed', 'superseded'])

/** One vehicle's state from what the console has (coarser than the bridge's: no dispatcher's
 *  jobs, no automation's waits). `waiting`: it has an order or an alert waiting for a person. */
export function fallbackState(v: Vehicle, hubs: readonly Hub[], order: OrderRef | undefined, waiting: boolean): Pick<FleetVehicle, 'state' | 'why' | 'at_hub'> {
  const here = hubAt(v.position, hubs)
  const at_hub = here?.id ?? null
  if (isStale(v)) return { state: 'offline', why: 'no telemetry', at_hub }
  if (waiting) return { state: 'attention', why: 'an order or an alert of it waits for you', at_hub }
  const kind = order?.intent?.kind ?? order?.tasking.task
  if (order && kind && !ENDED.has(order.op.state)) {
    const p = order.intent?.params ?? {}
    const parcel = typeof p.parcel === 'string' ? p.parcel : 'its parcel'
    switch (kind) {
      case 'pickup':
        return { state: 'loading', why: `loading ${parcel}`, at_hub }
      case 'deliver': {
        const zone = typeof p.drop_zone === 'string' ? p.drop_zone : 'its drop zone'
        const zat = Array.isArray(p.at) && p.at.length === 2 ? (p.at as LngLat) : null
        const near = zat && v.position ? metres(zat, v.position) <= AT_ZONE_M : false
        return near ? { state: 'delivering', why: `at ${zone} with ${parcel}`, at_hub } : { state: 'outbound', why: `taking ${parcel} to ${zone}`, at_hub }
      }
      case 'return_to_hub':
      case 'rtl':
        return { state: 'returning', why: 'back to a hub', at_hub }
      case 'swap_battery':
        return { state: 'charging', why: 'swapping its battery', at_hub }
      default:
        if (here && (kind === 'hold' || kind === 'land_now')) break
        return { state: 'tasked', why: kind.replace(/_/g, ' '), at_hub }
    }
  }
  const batt = v.battery_pct == null ? 'battery not known' : `${Math.round(v.battery_pct)}%`
  if (here) {
    if (v.battery_pct != null && v.battery_pct < SWAP_BELOW_PCT) return { state: 'charging', why: `${batt} at ${here.id}: a battery swap is due`, at_hub }
    return { state: 'ready', why: `${batt} at ${here.id}`, at_hub }
  }
  return { state: 'attention', why: v.position ? 'stopped away from a hub with no order' : 'no position reported', at_hub }
}

/** The fleet as the console can see it without the bridge's /api/fleet: each vehicle's base is
 *  the hub its autopilot's home is at (where it was launched), its state `fallbackState`. */
export function fallbackFleet(vehicles: readonly Vehicle[], hubs: readonly Hub[], orders: ReadonlyMap<number, OrderRef>, waiting: ReadonlySet<number>): FleetDoc {
  const list: FleetVehicle[] = vehicles.map((v) => {
    const s = fallbackState(v, hubs, orders.get(v.uid), waiting.has(v.uid))
    return { uid: v.uid, base: hubAt(v.home, hubs)?.id ?? null, ...s, since_s: 0, parcel: null }
  })
  const bases: FleetBase[] = hubs.map((h) => {
    const own = list.filter((x) => x.base === h.id)
    const here = list.filter((x) => x.at_hub === h.id)
    return {
      id: h.id,
      name: h.name || h.id,
      at: h.at,
      radius_m: h.radius_m,
      site: null,
      packs: h.packs,
      vehicles: own.length,
      counts: countOf(own),
      here: here.length,
      visitors: here.filter((x) => x.base !== h.id).length,
      parked: countOf(here),
    }
  })
  return { states: FLEET_STATES, sites: [], bases, vehicles: list, counts: countOf(list), derived: true }
}

// ---- the Fleet list's organisation ----------------------------------------------------------

export interface FleetFilter {
  /** vehicle types shown (`typeOf`); empty: all */
  types: ReadonlySet<string>
  /** the one state shown, or all */
  state: FleetStateId | null
}

export const NO_FILTER: FleetFilter = { types: new Set(), state: null }

export const filtering = (f: FleetFilter) => f.types.size > 0 || f.state != null

/** No base: the group of the vehicles that have none. */
export const NO_BASE = ''

export interface BaseGroup {
  id: string
  name: string
  site: string | null
  /** its vehicles that pass the filter, by state in state order */
  byState: { state: FleetStateId; vehicles: Vehicle[] }[]
  counts: Counts
  total: number
}

export interface SiteGroup {
  id: string | null
  name: string | null
  bases: BaseGroup[]
}

export interface Organised {
  /** the problems, whatever their base: needs attention, offline, out of service, by state then
   *  longest in it first */
  problems: { v: Vehicle; f: FleetVehicle }[]
  /** the sites in the order the bridge gives them, then the bases of no site; within a site its
   *  bases as the bridge lists them */
  sites: SiteGroup[]
  /** each state's count over what passes the filter */
  counts: Counts
  /** each type present, with its count (the filter's choices) */
  types: { id: string; n: number }[]
}

/** The fleet by site and base, filtered, its problems first. */
export function organise(doc: FleetDoc, vehicles: readonly Vehicle[], filter: FleetFilter = NO_FILTER): Organised {
  const byUid = new Map(doc.vehicles.map((f) => [f.uid, f]))
  const typeCount = new Map<string, number>()
  for (const v of vehicles) typeCount.set(typeOf(v), (typeCount.get(typeOf(v)) ?? 0) + 1)
  const pass = (v: Vehicle, f: FleetVehicle | undefined) =>
    (filter.types.size === 0 || filter.types.has(typeOf(v))) && (filter.state == null || f?.state === filter.state)
  const shown: { v: Vehicle; f: FleetVehicle }[] = []
  for (const v of vehicles) {
    // a vehicle the bridge's view does not have yet (just heard): its own entry, not yet placed
    const f = byUid.get(v.uid) ?? { uid: v.uid, base: null, at_hub: null, state: isStale(v) ? 'offline' : 'ready', why: '', since_s: 0, parcel: null }
    if (pass(v, f)) shown.push({ v, f })
  }
  const problems = shown
    .filter((x) => isProblem(x.f.state))
    .sort((a, b) => stateRank(a.f.state) - stateRank(b.f.state) || b.f.since_s - a.f.since_s || a.v.uid - b.v.uid)
  const baseOrder = doc.bases.map((b) => b.id)
  const groups = new Map<string, { v: Vehicle; f: FleetVehicle }[]>()
  for (const x of shown) {
    const id = x.f.base != null && baseOrder.includes(x.f.base) ? x.f.base : NO_BASE
    const g = groups.get(id) ?? []
    g.push(x)
    groups.set(id, g)
  }
  const baseGroup = (id: string): BaseGroup => {
    const b = doc.bases.find((x) => x.id === id)
    const xs = groups.get(id) ?? []
    const byState = STATE_IDS.map((state) => ({ state, vehicles: xs.filter((x) => x.f.state === state).map((x) => x.v).sort((a, c) => a.uid - c.uid) })).filter((s) => s.vehicles.length > 0)
    return { id, name: b?.name || (id === NO_BASE ? 'No base' : id), site: b?.site ?? null, byState, counts: countOf(xs.map((x) => x.f)), total: xs.length }
  }
  // every base, even one with none of the filtered (its counts say so), and "No base" only when used
  const bases = [...baseOrder, ...(groups.has(NO_BASE) ? [NO_BASE] : [])].map(baseGroup)
  const sites: SiteGroup[] = []
  for (const s of doc.sites) {
    const mine = bases.filter((b) => b.site === s.id)
    if (mine.length) sites.push({ id: s.id, name: s.name, bases: mine })
  }
  const loose = bases.filter((b) => b.site == null || !doc.sites.some((s) => s.id === b.site))
  if (loose.length) sites.push({ id: null, name: sites.length ? 'Other bases' : null, bases: loose })
  return {
    problems,
    sites,
    counts: countOf(shown.map((x) => x.f)),
    types: [...typeCount.entries()].map(([id, n]) => ({ id, n })).sort((a, b) => b.n - a.n || a.id.localeCompare(b.id)),
  }
}

/** The list's items, top to bottom, at fixed heights (FleetPanel windows them). */
export type FleetItem =
  | { kind: 'head'; key: string; title: string; count: number | null; top: number }
  | { kind: 'base'; key: string; base: BaseGroup; open: boolean; top: number }
  | { kind: 'state'; key: string; state: FleetStateId; count: number; top: number }
  | { kind: 'row'; key: string; v: Vehicle; section: 'problem' | 'base'; top: number }

export const ITEM_H: Record<FleetItem['kind'], number> = { head: 32, base: 52, state: 26, row: 56 }

/** Whether a base is open: as the person set it, else open while a filter is on or when the fleet
 *  has one base only. */
export function baseOpen(id: string, open: ReadonlyMap<string, boolean>, filter: FleetFilter, nBases: number): boolean {
  return open.get(id) ?? (filtering(filter) || nBases <= 1)
}

export function layoutItems(o: Organised, open: ReadonlyMap<string, boolean>, filter: FleetFilter): { items: FleetItem[]; height: number } {
  const items: FleetItem[] = []
  let top = 0
  const push = (it: FleetItem) => {
    items.push(it)
    top += ITEM_H[it.kind]
  }
  if (o.problems.length) {
    push({ kind: 'head', key: 'h:problems', title: 'Needs attention', count: o.problems.length, top })
    for (const { v } of o.problems) push({ kind: 'row', key: `p:${v.uid}`, v, section: 'problem', top })
  }
  const nBases = o.sites.reduce((n, s) => n + s.bases.length, 0)
  for (const s of o.sites) {
    if (s.name) push({ kind: 'head', key: `h:site:${s.id ?? ''}`, title: s.name, count: s.bases.reduce((n, b) => n + b.total, 0), top })
    for (const b of s.bases) {
      // under a filter, a base with none that pass is left out
      if (filtering(filter) && b.total === 0) continue
      const isOpen = baseOpen(b.id, open, filter, nBases)
      push({ kind: 'base', key: `b:${b.id}`, base: b, open: isOpen, top })
      if (!isOpen) continue
      for (const g of b.byState) {
        push({ kind: 'state', key: `s:${b.id}:${g.state}`, state: g.state, count: g.vehicles.length, top })
        for (const v of g.vehicles) push({ kind: 'row', key: `r:${b.id}:${v.uid}`, v, section: 'base', top })
      }
    }
  }
  return { items, height: top }
}
