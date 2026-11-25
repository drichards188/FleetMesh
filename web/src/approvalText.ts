// The words an order is asked and decided in, one sentence per kind, said the same way on the
// approval card, the waiting rows at the head of every drawer, the Queue and the Decisions list
// (murmur-i9zw, ledger 2 section 3): "Send 10 DRONE vehicles to patrol a 4.2 km loop". Built from
// the order's kind, its parameters and the fleet, never from the proposer's text; a part with no
// data behind it is left out, never filled. Display only: nothing here decides or is logged.
// Pure: approvalText.check.ts.
import { distanceM } from './geo.ts'
import type { LngLat } from './types'

/** An order as the sentence needs it (decisions.ts `Intent`). */
export interface AskIntent {
  kind: string
  params: Record<string, unknown>
  uids: readonly string[]
  group?: { swarm?: string; spread?: string } | null
}

/** The fleet, as the sentence needs it: one of ours by its uid. */
export interface AskFleet {
  callsign(uid: string): string
  /** the model's short label ("Drone"), or null when unknown */
  model(uid: string): string | null
  /** the swarm it belongs to, or null */
  swarm(uid: string): string | null
}

function km(m: number): string {
  return m >= 1000 ? `${(m / 1000).toFixed(1)} km` : `${Math.round(m / 10) * 10} m`
}

function loopM(r: readonly LngLat[]): number {
  let len = 0
  for (let k = 0; k < r.length; k++) len += distanceM(r[k], r[(k + 1) % r.length])
  return len
}

function pathM(r: readonly LngLat[]): number {
  let len = 0
  for (let k = 1; k < r.length; k++) len += distanceM(r[k - 1], r[k])
  return len
}

function commonOf(xs: (string | null)[]): string | null {
  const first = xs[0]
  return first && xs.every((x) => x === first) ? first : null
}

/** Who the order goes to: one or a few by callsign, more by their group and count. */
export function subjectOf(i: AskIntent, fleet: AskFleet): string {
  const n = i.uids.length
  if (n === 0) return 'no vehicle'
  if (n <= 3) return i.uids.map((u) => fleet.callsign(u)).join(', ')
  const swarm = i.group?.swarm ?? commonOf(i.uids.map((u) => fleet.swarm(u)))
  if (swarm) return `${n} ${swarm} vehicles`
  const model = commonOf(i.uids.map((u) => fleet.model(u)))
  return model ? `${n} ${model} vehicles` : `${n} vehicles`
}

/** What the order has them do, after "Send X": "to orbit, 2.0 km radius". Null for a kind that is
 *  not sent to vehicles. */
function predicate(i: AskIntent): string | null {
  const p = i.params as { center?: LngLat; radius_m?: number; route?: LngLat[] }
  switch (i.kind) {
    case 'orbit':
      return p.radius_m ? `to orbit, ${km(p.radius_m)} radius` : 'to orbit'
    case 'patrol': {
      const r = p.route ?? []
      return r.length > 1 ? `to patrol a ${km(loopM(r))} loop of ${r.length} waypoints` : 'to patrol'
    }
    case 'transit': {
      const r = p.route ?? []
      return r.length > 1 ? `along a ${km(pathM(r))} route of ${r.length} points, to hold at its end` : 'to a point, to hold there'
    }
    case 'rtl':
      return 'to return to launch'
    case 'hold':
      return 'to hold position'
    // delivery orders (docs/design/delivery-orders.md)
    case 'pickup': {
      const q = i.params as { parcel?: string; site?: { id?: string } }
      return `to pick up ${q.parcel ?? 'a parcel'} at ${q.site?.id ?? 'a point'}`
    }
    case 'deliver': {
      const q = i.params as { parcel?: string; drop_zone?: string }
      return `to deliver ${q.parcel ?? 'a parcel'} to ${q.drop_zone ?? 'a drop zone'}`
    }
    case 'reroute':
      return 'to take a new route'
    case 'return_to_hub': {
      const q = i.params as { hub?: { id?: string } }
      return q.hub?.id ? `to return to ${q.hub.id}` : 'to return to a hub'
    }
    case 'land_now':
      return 'to land now'
    case 'abort':
      return 'to stop its current order'
    case 'swap_battery': {
      const q = i.params as { hub?: string }
      return `to swap the battery at ${q.hub ?? 'a hub'}`
    }
    default:
      return null
  }
}

/** The ask as one sentence: "Send 10 DRONE vehicles to patrol a 4.2 km loop of 6 waypoints". In
 *  the past tense ("Sent ...", for the Decisions list) when `past`. */
export function askSentence(i: AskIntent, fleet: AskFleet, past = false): string {
  if (i.kind === 'confirm_drop_zone') {
    // a question to the person, not an order to the vehicle (so the same words past or present)
    const q = i.params as { drop_zone?: string; parcel?: string }
    const waiting = i.uids.length ? ` (${subjectOf(i, fleet)} waiting)` : ''
    return `Confirm drop zone ${q.drop_zone ?? 'the drop zone'} is clear for parcel ${q.parcel ?? 'the parcel'}${waiting}`
  }
  const what = predicate(i)
  const verb = past ? 'Sent' : 'Send'
  const who = subjectOf(i, fleet)
  return what ? `${verb} ${who} ${what}` : `${verb} ${who}: ${i.kind}`
}

/** The place an order names, in coordinates for a fold (a place list would say "west of the bridge"). */
export function placeLine(i: AskIntent): string | null {
  const p = i.params as { center?: LngLat; route?: LngLat[] }
  const ll = (q: LngLat) => `${q[1].toFixed(4)}, ${q[0].toFixed(4)}`
  if (p.center) return `centre ${ll(p.center)}`
  if (p.route?.length) return `${p.route.length === 1 ? 'point' : 'route from'} ${ll(p.route[0])}${p.route.length > 1 ? ` to ${ll(p.route[p.route.length - 1])}` : ''}`
  return null
}

const SEND_LABEL: Record<string, string> = {
  orbit: 'Send the orbit',
  patrol: 'Send the patrol',
  transit: 'Send the transit',
  rtl: 'Send the return',
  hold: 'Send the hold',
  pickup: 'Send the pickup',
  deliver: 'Send the delivery',
  reroute: 'Send the reroute',
  return_to_hub: 'Send the return to the hub',
  land_now: 'Land it',
  abort: 'Send the abort',
  swap_battery: 'Swap the battery',
  confirm_drop_zone: 'Confirm the drop zone',
}

/** The approve button, named for what it does. */
export function sendLabel(kind: string): string {
  return SEND_LABEL[kind] ?? 'Send the order'
}

export const DONT_SEND = "Don't send"
export const NOTHING_SENT = 'Nothing is sent unless you approve.'
