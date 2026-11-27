// The delivery picture the console reads: parcels (`GET /api/parcels`), hubs and drop zones
// (`GET /api/places`), each vehicle's navigation trust (`GET /api/nav`), and the operator's load
// (docs/design/attention-cost.md section 4.1). Display only: nothing here decides or sends. Pure
// (no browser): delivery.check.ts runs it under node.

import type { LngLat } from './types.ts'

/** A parcel as the bridge reports it (`delivery_rt.rs` `parcels_json`). Times are seconds into the
 *  scenario, not clock times. */
export interface Parcel {
  id: string
  /** waiting, ready, assigned, loaded, delivered or failed */
  state: string
  /** the origin hub's id */
  from: string
  /** the destination drop zone's id */
  to: string
  mass_kg: number
  priority: number
  class: string
  ready_s: number
  due_s: number
  /** the vehicle carrying or assigned to it (its uid), if any */
  vehicle: string | null
  tries: number
  /** why it failed (absent while it has not): the end that made automation give it up */
  failure?: ParcelEnd | null
  /** how its earlier attempts fell short, oldest first (the bridge keeps the last few) */
  ends?: ParcelEnd[]
}

/** How one attempt at a parcel fell short (`delivery_rt.rs` `TryEnd`). */
export interface ParcelEnd {
  /** pickup, deliver, look (the look before departure or over the zone kept it) or return */
  stage: string
  /** rejected, failed, expired, voided, aborted or kept */
  ended: string
  /** the refusing or holding checks' reason codes (none for a task-layer failure) */
  codes?: string[]
  detail: string
  vehicle?: string
  intent?: number
  /** seconds into the scenario */
  at_s: number
}

/** Why a parcel failed, in a line: "deliver rejected (SEP_ROUTE_CONFLICT): ...". Empty when it has
 *  not failed. */
export function failureText(p: Pick<Parcel, 'failure'>): string {
  const f = p.failure
  if (!f) return ''
  const codes = f.codes?.length ? ` (${f.codes.join(', ')})` : ''
  return `${f.stage} ${f.ended}${codes}: ${f.detail}`
}

export interface ParcelsDoc {
  scenario: { name: string; title: string; seed: number; duration_s: number } | null
  /** seconds into the scenario now */
  elapsed_s: number
  counts: Record<string, number>
  parcels: Parcel[]
}

export interface Hub {
  id: string
  name: string
  at: LngLat
  radius_m: number
  pads: number
  /** charged battery packs now */
  packs: number
  open: boolean
}

/** The latest look at a drop zone (a scenario's feed), with its age. */
export interface Observation {
  state: 'clear' | 'occupied' | 'unknown'
  count: number
  age_s: number
}

export interface DropZone {
  id: string
  kind: string
  at: LngLat
  radius_m: number
  surface: 'land' | 'water'
  closed: boolean
  populated: boolean
  hub: string | null
  /** null: no one has looked (only zones with an open parcel are watched) */
  observation: Observation | null
}

export interface PlacesDoc {
  hubs: Hub[]
  drop_zones: DropZone[]
}

export type NavTrust = 'trusted' | 'degraded' | 'untrusted' | 'unknown'

export interface NavInfo {
  state: NavTrust
  /** what the navigation monitor proposed for it, while it is untrusted */
  action?: { intent: number; task: string; at_wall_s: number; why: string } | null
}

export interface NavDoc {
  vehicles: Record<string, NavInfo>
}

export const EMPTY_PARCELS: ParcelsDoc = { scenario: null, elapsed_s: 0, counts: {}, parcels: [] }
export const EMPTY_PLACES: PlacesDoc = { hubs: [], drop_zones: [] }
export const EMPTY_NAV: NavDoc = { vehicles: {} }

// ---- parcels --------------------------------------------------------------------------------------

export type ParcelFilter = 'all' | 'waiting' | 'assigned' | 'transit' | 'delivered' | 'late'

export const PARCEL_FILTERS: { key: ParcelFilter; label: string; title: string }[] = [
  { key: 'all', label: 'All', title: 'Every parcel' },
  { key: 'waiting', label: 'Waiting', title: 'Not yet assigned to a vehicle (waiting for its ready time, or for a vehicle)' },
  { key: 'assigned', label: 'Assigned', title: 'A vehicle has the parcel and is going to pick it up' },
  { key: 'transit', label: 'In transit', title: 'Loaded on a vehicle, on its way to the drop zone' },
  { key: 'delivered', label: 'Delivered', title: 'Released at the drop zone' },
  { key: 'late', label: 'Late', title: 'Past its due time and not delivered' },
]

/** A parcel's state in words. */
export const PARCEL_STATE_LABEL: Record<string, string> = {
  waiting: 'Waiting',
  ready: 'Waiting',
  assigned: 'Assigned',
  loaded: 'In transit',
  delivered: 'Delivered',
  failed: 'Failed',
}

export function parcelStateLabel(state: string): string {
  return PARCEL_STATE_LABEL[state] ?? state
}

/** Open: not delivered and not failed. */
export function isOpen(p: Pick<Parcel, 'state'>): boolean {
  return p.state !== 'delivered' && p.state !== 'failed'
}

/** Past its due time and still open. `elapsedS`: seconds into the scenario now. */
export function isLate(p: Pick<Parcel, 'state' | 'due_s'>, elapsedS: number): boolean {
  return isOpen(p) && elapsedS > p.due_s
}

/** Whether a parcel belongs under a filter. A late parcel is also under its own state's filter. */
export function inFilter(p: Parcel, f: ParcelFilter, elapsedS: number): boolean {
  switch (f) {
    case 'all':
      return true
    case 'waiting':
      return p.state === 'waiting' || p.state === 'ready'
    case 'assigned':
      return p.state === 'assigned'
    case 'transit':
      return p.state === 'loaded'
    case 'delivered':
      return p.state === 'delivered'
    case 'late':
      return isLate(p, elapsedS)
  }
}

/** How many parcels each filter holds. */
export function filterCounts(parcels: readonly Parcel[], elapsedS: number): Record<ParcelFilter, number> {
  const out: Record<ParcelFilter, number> = { all: 0, waiting: 0, assigned: 0, transit: 0, delivered: 0, late: 0 }
  for (const p of parcels) for (const f of PARCEL_FILTERS) if (inFilter(p, f.key, elapsedS)) out[f.key]++
  return out
}

/** The parcels under a filter, most pressing first: late ones, then by due time, then by id. */
export function filterParcels(parcels: readonly Parcel[], f: ParcelFilter, elapsedS: number): Parcel[] {
  return parcels
    .filter((p) => inFilter(p, f, elapsedS))
    .sort((a, b) => Number(isLate(b, elapsedS)) - Number(isLate(a, elapsedS)) || a.due_s - b.due_s || a.id.localeCompare(b.id))
}

/** "12 min", "45 s", "1 h 05 min" */
export function spanText(seconds: number): string {
  const s = Math.max(0, Math.round(seconds))
  if (s < 90) return `${s} s`
  const m = Math.round(s / 60)
  if (m < 90) return `${m} min`
  return `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')} min`
}

/** A parcel's due time against now: "due in 12 min", "late by 3 min", "delivered". */
export function dueText(p: Pick<Parcel, 'state' | 'due_s'>, elapsedS: number): string {
  if (p.state === 'delivered') return 'delivered'
  if (p.state === 'failed') return 'failed'
  const left = p.due_s - elapsedS
  return left >= 0 ? `due in ${spanText(left)}` : `late by ${spanText(-left)}`
}

// ---- parcels by place (the map draws one mark for each place, with how many are there) ----------

export type ParcelPlaceKind = 'hub' | 'vehicle' | 'zone'

/** Where a parcel is now, as a place: `hub:<id>` open at its hub (ready, or assigned to a vehicle
 *  not yet there), `vehicle:<uid>` loaded on it, `zone:<id>` delivered there. Null while its order
 *  is not placed yet (waiting for its ready time: it is not at the hub yet) and once it failed. */
export function parcelPlaceKey(p: Pick<Parcel, 'state' | 'from' | 'to' | 'vehicle'>): string | null {
  switch (p.state) {
    case 'ready':
    case 'assigned':
      return `hub:${p.from}`
    case 'loaded':
      return p.vehicle ? `vehicle:${p.vehicle}` : `hub:${p.from}`
    case 'delivered':
      return `zone:${p.to}`
    default:
      return null
  }
}

export interface ParcelPlace {
  /** `hub:<id>`, `vehicle:<uid>` or `zone:<id>` (parcelPlaceKey) */
  key: string
  kind: ParcelPlaceKind
  /** the hub's or drop zone's id, or the vehicle's uid */
  id: string
  /** the hub's name, the drop zone's id, or the vehicle's uid (the caller has its callsign) */
  name: string
  at: LngLat
  /** the hub's or drop zone's radius (0 for a vehicle), for where the mark goes beside it */
  radius_m: number
  /** the parcels there now, most pressing first: late ones, then by due time */
  parcels: Parcel[]
  late: number
  /** a hub's orders not placed yet (their ready time is still to come); 0 elsewhere */
  later: number
}

/** The places parcels are now, one each however many are there: hubs first, then drop zones, then
 *  vehicles, each by id (a stable order for the label budget). A place with no position (a vehicle
 *  not in the picture) is left out; the Parcels list still has its parcels. */
export function parcelPlaces(
  parcels: readonly Parcel[],
  hubs: ReadonlyMap<string, Hub>,
  zones: ReadonlyMap<string, DropZone>,
  vehicleAt: (uid: string) => LngLat | null,
  elapsedS: number,
): ParcelPlace[] {
  const by = new Map<string, Parcel[]>()
  const later = new Map<string, number>()
  for (const p of parcels) {
    if (p.state === 'waiting') later.set(p.from, (later.get(p.from) ?? 0) + 1)
    const key = parcelPlaceKey(p)
    if (!key) continue
    const list = by.get(key)
    if (list) list.push(p)
    else by.set(key, [p])
  }
  const out: ParcelPlace[] = []
  for (const [key, list] of by) {
    const [kind, id] = key.split(/:(.*)/s) as [ParcelPlaceKind, string]
    const hub = kind === 'hub' ? hubs.get(id) : undefined
    const zone = kind === 'zone' ? zones.get(id) : undefined
    const at = kind === 'vehicle' ? vehicleAt(id) : (hub?.at ?? zone?.at ?? null)
    if (!at) continue
    const sorted = filterParcels(list, 'all', elapsedS)
    out.push({ key, kind, id, name: hub?.name ?? id, at, radius_m: hub?.radius_m ?? zone?.radius_m ?? 0, parcels: sorted, late: sorted.filter((p) => isLate(p, elapsedS)).length, later: kind === 'hub' ? (later.get(id) ?? 0) : 0 })
  }
  const rank: Record<ParcelPlaceKind, number> = { hub: 0, zone: 1, vehicle: 2 }
  return out.sort((a, b) => rank[a.kind] - rank[b.kind] || a.id.localeCompare(b.id, undefined, { numeric: true }))
}

const PLACE_STATE_WORDS: Record<string, string> = { ready: 'waiting for a vehicle', assigned: 'assigned', loaded: 'in transit', delivered: 'delivered' }

/** What a place's parcels are, in a few words: "3 parcels: 2 waiting for a vehicle, 1 assigned (1
 *  late)"; one parcel by its id. Its place is said by the caller. */
export function placeParcelsText(pl: Pick<ParcelPlace, 'parcels' | 'late' | 'later'>, elapsedS: number): string {
  const n = pl.parcels.length
  const later = pl.later ? `; ${pl.later} more not ordered yet` : ''
  const words = (p: Parcel) => PLACE_STATE_WORDS[p.state] ?? parcelStateLabel(p.state).toLowerCase()
  if (n === 1) {
    const p = pl.parcels[0]
    if (p.state === 'delivered') return `${p.id} delivered, ${p.mass_kg} kg${later}`
    return `${p.id} ${words(p)}, ${p.mass_kg} kg to ${p.to}, ${dueText(p, elapsedS)}${later}`
  }
  const states = new Map<string, number>()
  for (const p of pl.parcels) states.set(words(p), (states.get(words(p)) ?? 0) + 1)
  const late = pl.late ? ` (${pl.late} late)` : ''
  if (states.size === 1) return `${n} parcels ${[...states.keys()][0]}${late}${later}`
  return `${n} parcels: ${[...states].map(([s, k]) => `${k} ${s}`).join(', ')}${late}${later}`
}


// ---- drop zones and vehicles ----------------------------------------------------------------------

/** What the latest look at a zone says, in a few words. */
export function observationText(o: Observation | null): string {
  if (!o) return 'not looked at'
  const age = o.age_s < 1.5 ? 'just now' : `${spanText(o.age_s)} ago`
  if (o.state === 'occupied') return `occupied: ${o.count} ${o.count === 1 ? 'person' : 'people'} seen, ${age}`
  if (o.state === 'clear') return `clear, ${age}`
  return `could not tell, ${age}`
}

export type ObservationTone = 'ok' | 'bad' | 'unknown'

export function observationTone(o: Observation | null, closed = false): ObservationTone {
  if (closed) return 'bad'
  if (!o || o.state === 'unknown') return 'unknown'
  return o.state === 'occupied' ? 'bad' : 'ok'
}

export type PayloadState = 'empty' | 'loaded' | 'released'

export const PAYLOAD_WORDS: Record<PayloadState, string> = {
  empty: 'Empty: nothing loaded',
  loaded: 'Loaded: a parcel is aboard',
  released: 'Released: the parcel was let go',
}

export function payloadWords(p: string | null | undefined): string {
  return p && p in PAYLOAD_WORDS ? PAYLOAD_WORDS[p as PayloadState] : 'Not reported'
}

export const NAV_WORDS: Record<NavTrust, string> = {
  trusted: 'Trusted: the position fix is good',
  degraded: 'Degraded: a fix, but less accurate than it should be',
  untrusted: 'Untrusted: the position cannot be relied on',
  unknown: 'Unknown: no recent position facts to judge it by',
}

export function navWords(s: string | null | undefined): string {
  return s && s in NAV_WORDS ? NAV_WORDS[s as NavTrust] : NAV_WORDS.unknown
}

/** The navigation trust of a vehicle by its uid, unknown when the monitor has no row. */
export function navOf(nav: NavDoc, uid: string | number): NavTrust {
  return nav.vehicles[String(uid)]?.state ?? 'unknown'
}

// ---- the operator's load --------------------------------------------------------------------------

export interface LoadIntent {
  history: readonly { state: string; t: number }[]
  created_t: number
}

export interface Load {
  /** items waiting for a person */
  waiting: number
  /** how long the oldest has waited, s; null with none waiting */
  oldestS: number | null
  /** decisions a person made in the last ten minutes */
  decided: number
}

/** When an item began to wait for a person (ms): its move to pending approval, else its creation. */
export function waitingSince(i: LoadIntent): number {
  for (let k = i.history.length - 1; k >= 0; k--) if (i.history[k].state === 'pending_approval') return i.history[k].t
  return i.created_t
}

/** The operator's load readout (attention-cost.md 4.1): items pending, the oldest one's age, and
 *  decisions in the last ten minutes. `decided10m` is the bridge's own count (`inbox.counts`); a
 *  bridge without it is read from the intents the console holds (the latest few, so a busy run
 *  undercounts). `nowMs`: the bridge's clock. */
export function operatorLoad(pending: readonly LoadIntent[], nowMs: number, decided10m: number | null | undefined, intents: readonly LoadIntent[] = []): Load {
  let oldest: number | null = null
  for (const i of pending) {
    const age = Math.max(0, (nowMs - waitingSince(i)) / 1000)
    if (oldest == null || age > oldest) oldest = age
  }
  const decided = decided10m ?? decisionsIn(intents, nowMs, 600_000)
  return { waiting: pending.length, oldestS: oldest, decided }
}

/** Decisions in the window from intents' histories: a move from pending approval to approved or
 *  rejected. (A check's rejection, an expiry and a voiding are not a person's decision.) */
export function decisionsIn(intents: readonly LoadIntent[], nowMs: number, windowMs: number): number {
  let n = 0
  for (const i of intents)
    for (let k = 1; k < i.history.length; k++) {
      const h = i.history[k]
      if (i.history[k - 1].state === 'pending_approval' && (h.state === 'approved' || h.state === 'rejected') && nowMs - h.t <= windowMs) n++
    }
  return n
}

/** The load as the top bar says it: "3 waiting · oldest 2 min · 7 decided in 10 min". */
export function loadWords(l: Load): string {
  const oldest = l.oldestS == null ? '' : ` · oldest ${spanText(l.oldestS)}`
  return `${l.waiting} waiting${oldest} · ${l.decided} decided in 10 min`
}

/** The load's tone against what the attention-cost draft says: quiet, busy (several waiting or one
 *  waiting long), or heavy. These are display thresholds, not a measure. */
export function loadTone(l: Load): 'quiet' | 'busy' | 'heavy' {
  if (l.waiting >= 8 || (l.oldestS ?? 0) >= 300) return 'heavy'
  if (l.waiting >= 3 || (l.oldestS ?? 0) >= 120) return 'busy'
  return 'quiet'
}
