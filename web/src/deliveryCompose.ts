// The delivery order composer's logic (docs/design/delivery-orders.md section 5): which fields each
// order kind needs, what is missing, and the request it makes for the console's own order path
// (`POST /api/tasks`, `POST /api/intents/preview`). It decides nothing: the engine checks the
// order and policy routes it, as for any order. Pure: deliveryCompose.check.ts.

export type ComposeKind = 'pickup' | 'deliver' | 'reroute' | 'return_to_hub' | 'land_now' | 'abort' | 'swap_battery'

export const COMPOSE_KINDS: { kind: ComposeKind; label: string; hint: string }[] = [
  { kind: 'pickup', label: 'Pick up', hint: 'Send the vehicle to a hub to load a parcel.' },
  { kind: 'deliver', label: 'Deliver', hint: 'Take the loaded parcel to a drop zone and release it.' },
  { kind: 'reroute', label: 'Reroute', hint: 'Replace the rest of its route: click the map to add points.' },
  { kind: 'return_to_hub', label: 'Return to hub', hint: 'Go to a hub and land. No hub chosen: the vehicle’s own way home.' },
  { kind: 'land_now', label: 'Land now', hint: 'Land, or stop, as soon as it is safe, where it is.' },
  { kind: 'abort', label: 'Abort', hint: 'Stop a running order and do something safe instead.' },
  { kind: 'swap_battery', label: 'Swap battery', hint: 'The hub swaps the battery of the vehicle on its pad.' },
]

export const RELEASES = ['land', 'winch', 'dock', 'slip'] as const
export const REROUTE_CAUSES = ['operator', 'keep_out', 'weather', 'conflict', 'battery'] as const
export const ABORT_THEN = ['return_to_hub', 'hold', 'land_now'] as const

/** Kinds that name a parcel and so take one vehicle (a parcel rides one vehicle). */
const ONE_VEHICLE: ReadonlySet<ComposeKind> = new Set(['pickup', 'deliver', 'reroute', 'abort'])

/** What the person has chosen. */
export interface ComposeSpec {
  kind: ComposeKind
  /** the vehicles selected (uids) */
  uids: number[]
  parcel: string
  hub: string
  dropZone: string
  release: (typeof RELEASES)[number] | ''
  /** reroute: the points the map clicks gave, [lon, lat] */
  route: [number, number][]
  cause: (typeof REROUTE_CAUSES)[number]
  /** abort: the order to stop (an intent id) and what then */
  target: number | null
  then: (typeof ABORT_THEN)[number]
  pad: string
}

export const EMPTY_SPEC: ComposeSpec = {
  kind: 'pickup',
  uids: [],
  parcel: '',
  hub: '',
  dropZone: '',
  release: '',
  route: [],
  cause: 'operator',
  target: null,
  then: 'return_to_hub',
  pad: '',
}

/** What the picker can offer: ids that exist now. */
export interface ComposeWorld {
  parcels: readonly { id: string; state: string; from: string; to: string }[]
  hubs: readonly { id: string; open: boolean }[]
  dropZones: readonly { id: string; closed: boolean }[]
  /** the orders still running or waiting on the selected vehicles (abort's choices) */
  stoppable: readonly { id: number; uids: string[] }[]
}

/** The body the console's order path takes (`TaskRequest` with the delivery fields). */
export interface DeliveryRequest {
  task: ComposeKind
  uids: number[]
  parcel?: string
  site?: string
  hub?: string
  pad?: string
  drop_zone?: string
  release?: string
  route?: [number, number][]
  cause?: string
  target?: number
  then?: string
}

export interface Composed {
  /** what is missing or wrong, in words; empty when the order can be sent */
  problems: string[]
  /** one request per vehicle (the delivery orders are one vehicle's each); empty while there are problems */
  requests: DeliveryRequest[]
}

/** The order the spec makes, or what stops it. Whether the order is allowed is the engine's to
 *  say (the preview shows it); this only says whether it can be asked. */
export function compose(s: ComposeSpec, w: ComposeWorld): Composed {
  const problems: string[] = []
  if (s.uids.length === 0) problems.push('Select at least one vehicle.')
  if (ONE_VEHICLE.has(s.kind) && s.uids.length > 1) problems.push(`${labelOf(s.kind)} is one vehicle’s order: select one vehicle (${s.uids.length} are selected).`)
  const hubOk = (id: string) => w.hubs.some((h) => h.id === id)
  const parcel = w.parcels.find((p) => p.id === s.parcel)
  switch (s.kind) {
    case 'pickup':
      if (!s.parcel) problems.push('Choose the parcel to load.')
      else if (!parcel) problems.push(`No parcel ${s.parcel}.`)
      else if (parcel.state === 'delivered' || parcel.state === 'failed') problems.push(`${s.parcel} is ${parcel.state}: nothing to load.`)
      else if (parcel.state === 'loaded') problems.push(`${s.parcel} is already loaded on a vehicle.`)
      if (!s.hub) problems.push('Choose the hub to load it at.')
      else if (!hubOk(s.hub)) problems.push(`No hub ${s.hub}.`)
      break
    case 'deliver':
      if (!s.parcel) problems.push('Choose the parcel to deliver.')
      else if (!parcel) problems.push(`No parcel ${s.parcel}.`)
      else if (parcel.state !== 'loaded') problems.push(`${s.parcel} is not loaded on a vehicle yet (it is ${parcel.state}): pick it up first.`)
      if (!s.dropZone) problems.push('Choose the drop zone.')
      else if (!w.dropZones.some((z) => z.id === s.dropZone)) problems.push(`No drop zone ${s.dropZone}.`)
      break
    case 'reroute':
      if (s.route.length < 1) problems.push('Click the map to add at least one point of the new route.')
      break
    case 'return_to_hub':
      if (s.hub && !hubOk(s.hub)) problems.push(`No hub ${s.hub}.`)
      break
    case 'swap_battery':
      if (!s.hub) problems.push('Choose the hub that swaps the battery.')
      else if (!hubOk(s.hub)) problems.push(`No hub ${s.hub}.`)
      break
    case 'abort':
      if (s.target == null) problems.push('Choose the order to stop.')
      else if (!w.stoppable.some((t) => t.id === s.target)) problems.push(`Order i${s.target} is not running on the selected vehicle.`)
      break
    case 'land_now':
      break
  }
  if (problems.length) return { problems, requests: [] }
  const base = (uid: number): DeliveryRequest => ({ task: s.kind, uids: [uid] })
  const requests = s.uids.map((uid): DeliveryRequest => {
    const r = base(uid)
    switch (s.kind) {
      case 'pickup':
        r.parcel = s.parcel
        r.site = s.hub
        break
      case 'deliver':
        r.parcel = s.parcel
        r.drop_zone = s.dropZone
        if (s.release) r.release = s.release
        break
      case 'reroute':
        r.route = s.route
        r.cause = s.cause
        break
      case 'return_to_hub':
        if (s.hub) r.hub = s.hub
        break
      case 'swap_battery':
        r.hub = s.hub
        if (s.pad.trim()) r.pad = s.pad.trim()
        break
      case 'abort':
        r.target = s.target ?? undefined
        r.then = s.then
        break
      case 'land_now':
        break
    }
    return r
  })
  return { problems, requests }
}

export function labelOf(kind: ComposeKind): string {
  return COMPOSE_KINDS.find((k) => k.kind === kind)?.label ?? kind
}

/** The parcels a picker offers for a kind: pickup takes those not yet loaded or finished, deliver
 *  those loaded. A vehicle's own parcel (assigned or loaded on it) comes first. */
export function parcelChoices(kind: ComposeKind, parcels: readonly { id: string; state: string; vehicle?: string | null }[], uid: number | null): { id: string; state: string }[] {
  const want = kind === 'deliver' ? (st: string) => st === 'loaded' : (st: string) => st === 'waiting' || st === 'ready' || st === 'assigned'
  const mine = (p: { vehicle?: string | null }) => uid != null && p.vehicle === String(uid)
  return parcels
    .filter((p) => want(p.state))
    .sort((a, b) => Number(mine(b)) - Number(mine(a)) || a.id.localeCompare(b.id))
}

/** The orders a vehicle has that an abort may stop: not finished, naming it. */
export function stoppableFor(
  intents: readonly { id: number; kind: string; state: string; uids: string[] }[],
  uid: number | null,
): { id: number; kind: string; state: string; uids: string[] }[] {
  const live = new Set(['approved', 'auto_approved', 'queued', 'dispatched', 'pending_approval'])
  return intents.filter((i) => uid != null && i.uids.includes(String(uid)) && live.has(i.state) && i.kind !== 'abort').sort((a, b) => b.id - a.id)
}
