// Checks for fleetModel.ts: node web/src/fleetModel.check.ts (prints "fleetModel: ok"), as make check
// runs it. The claims (docs/design/fleet-organisation.md): the fleet is listed by site and base, what
// needs a person first whatever its base, each base with its counts by state in state order; the
// filters by type and by state keep the counts honest; a bridge without /api/fleet still gets a
// base for each vehicle (the hub its autopilot's home is at) and a coarse state from its link, its
// order and where it is.
import assert from 'node:assert/strict'
import type { Hub } from './delivery'
import {
  baseOpen,
  countsText,
  fallbackFleet,
  fallbackState,
  type FleetDoc,
  type FleetVehicle,
  FLEET_STATES,
  hubAt,
  layoutItems,
  NO_FILTER,
  organise,
  STATE_IDS,
  STATE_WORD,
  typeOf,
} from './fleetModel.ts'
import type { OrderRef } from './orderWords'
import type { LngLat, Vehicle } from './types'

const MARINA: LngLat = [-122.437, 37.8057]
const KERB: LngLat = [-122.436, 37.8045]
const PIER: LngLat = [-122.41, 37.8089]
const hub = (id: string, at: LngLat, radius_m = 80): Hub => ({ id, name: id, at, radius_m, pads: 4, packs: 4, open: true })
const HUBS = [hub('hub-marina', MARINA), hub('hub-marina-kerb', KERB, 40), hub('hub-pier', PIER)]

function vehicle(uid: number, platform: string, at: LngLat | null, extra: Partial<Vehicle> = {}): Vehicle {
  return {
    uid,
    sysid: uid,
    identity: { callsign: `${platform.toUpperCase()} ${uid}`, platform: `delivery_${platform}`, name: platform, short: platform, role: '', domain: platform === 'boat' ? 'surface' : platform === 'car' ? 'ground' : 'air', link: 'cellular', sim: 'standin' },
    kind: platform === 'drone' ? 'copter' : platform === 'car' ? 'ugv' : platform === 'boat' ? 'boat' : 'plane',
    mode: null,
    armed: false,
    position: at,
    alt_rel: null,
    alt_msl: null,
    roll: null,
    pitch: null,
    heading: null,
    yaw: null,
    nav_ready: true,
    groundspeed: 0,
    airspeed: null,
    battery_pct: 90,
    gps_fix: 3,
    satellites: 12,
    mission_seq: null,
    home: at,
    link: { since_last_ms: 100, loss_pct: 0, rtt_ms: 20 },
    trail: [],
    statustext: [],
    ...extra,
  }
}

const fv = (uid: number, base: string | null, state: FleetVehicle['state'], since_s = 0): FleetVehicle => ({ uid, base, at_hub: base, state, why: `${state} why`, since_s, parcel: null })

// ---- the states and their words -----------------------------------------------------------
assert.deepEqual(STATE_IDS.slice(0, 3), ['attention', 'offline', 'out_of_service'], 'problems first')
assert.ok(FLEET_STATES.every((s) => s.label && s.means && !/\u2014/.test(s.means + s.label)), 'every state worded, no em dash')
assert.ok(STATE_IDS.every((id) => STATE_WORD[id]), 'every state has its word for the counts')
assert.equal(countsText({ ready: 9, charging: 2 }), '9 ready, 2 charging')
assert.equal(countsText({ charging: 2, attention: 1, ready: 9 }), '1 needs attention, 9 ready, 2 charging', 'in state order, problems first')
assert.equal(countsText({ attention: 1, ready: 9, loading: 2, outbound: 3, returning: 1 }, 3), '1 needs attention, 9 ready, 2 loading, 4 more')
assert.equal(typeOf(vehicle(1, 'drone', MARINA)), 'drone')

// ---- organised by site and base ----------------------------------------------------------
const doc: FleetDoc = {
  states: FLEET_STATES,
  sites: [{ id: 'marina', name: 'Marina', hubs: ['hub-marina', 'hub-marina-kerb'] }],
  bases: [
    { id: 'hub-marina', name: 'Marina Green hub', at: MARINA, radius_m: 80, site: 'marina', vehicles: 3, counts: {}, here: 3, visitors: 0, parked: {} },
    { id: 'hub-marina-kerb', name: 'Marina car kerb', at: KERB, radius_m: 40, site: 'marina', vehicles: 1, counts: {}, here: 1, visitors: 0, parked: {} },
    { id: 'hub-pier', name: 'Pier 39 hub', at: PIER, radius_m: 80, site: null, vehicles: 2, counts: {}, here: 2, visitors: 0, parked: {} },
  ],
  vehicles: [fv(1, 'hub-marina', 'ready'), fv(2, 'hub-marina', 'attention', 30), fv(3, 'hub-marina', 'outbound'), fv(4, 'hub-marina-kerb', 'ready'), fv(5, 'hub-pier', 'offline', 90), fv(6, 'hub-pier', 'attention', 120), fv(7, null, 'tasked')],
  counts: {},
}
const vs = [vehicle(1, 'drone', MARINA), vehicle(2, 'drone', MARINA), vehicle(3, 'drone', MARINA), vehicle(4, 'car', KERB), vehicle(5, 'drone', PIER), vehicle(6, 'boat', PIER), vehicle(7, 'plane', null)]
const o = organise(doc, vs)
assert.deepEqual(o.problems.map((x) => x.v.uid), [6, 2, 5], 'needs attention first, longest waiting first, then offline: whatever the base')
assert.deepEqual(o.sites.map((s) => s.name), ['Marina', 'Other bases'])
assert.deepEqual(o.sites[0].bases.map((b) => b.id), ['hub-marina', 'hub-marina-kerb'])
assert.deepEqual(o.sites[1].bases.map((b) => b.id), ['hub-pier', ''], 'a base of no site, then the vehicles of no base')
const marina = o.sites[0].bases[0]
assert.deepEqual(marina.counts, { ready: 1, attention: 1, outbound: 1 })
assert.deepEqual(marina.byState.map((g) => g.state), ['attention', 'ready', 'outbound'], 'a base lists its vehicles by state, problems first')
assert.equal(o.sites[1].bases[1].name, 'No base')
assert.deepEqual(o.types.map((t) => t.id), ['drone', 'boat', 'car', 'plane'], 'the types present, most numerous first')

// filters: a type, a state; the counts follow them
const drones = organise(doc, vs, { types: new Set(['drone']), state: null })
assert.equal(drones.counts.ready, 1)
assert.equal(drones.counts.attention, 1)
assert.deepEqual(drones.problems.map((x) => x.v.uid), [2, 5])
const ready = organise(doc, vs, { types: new Set(), state: 'ready' })
assert.deepEqual(ready.problems, [])
assert.equal(ready.counts.ready, 2)

// the list: closed bases show their counts only; open, their vehicles under each state
const closed = layoutItems(o, new Map(), NO_FILTER)
assert.deepEqual(
  closed.items.map((i) => i.kind),
  ['head', 'row', 'row', 'row', 'head', 'base', 'base', 'head', 'base', 'base'],
  'needs attention, then each site with its bases closed',
)
assert.equal(closed.items[0].kind === 'head' && closed.items[0].title, 'Needs attention')
const opened = layoutItems(o, new Map([['hub-marina', true]]), NO_FILTER)
const kinds = opened.items.map((i) => (i.kind === 'state' ? `state:${i.state}` : i.kind === 'base' ? `base:${i.base.id}` : i.kind))
assert.deepEqual(kinds.slice(5, 13), ['base:hub-marina', 'state:attention', 'row', 'state:ready', 'row', 'state:outbound', 'row', 'base:hub-marina-kerb'])
let top = 0
for (const it of opened.items) {
  assert.equal(it.top, top, 'items stacked at their fixed heights')
  top += { head: 32, base: 52, state: 26, row: 56 }[it.kind]
}
assert.equal(opened.height, top)
// under a filter every base with any to show is open, the rest left out; one base alone is open
const filtered = layoutItems(ready, new Map(), { types: new Set(), state: 'ready' })
assert.ok(filtered.items.some((i) => i.kind === 'state' && i.state === 'ready'))
assert.ok(!filtered.items.some((i) => i.kind === 'base' && i.base.id === 'hub-pier'), 'none ready at the Pier: left out')
assert.equal(baseOpen('x', new Map(), NO_FILTER, 1), true)
assert.equal(baseOpen('x', new Map(), NO_FILTER, 3), false)
assert.equal(baseOpen('x', new Map([['x', false]]), { types: new Set(['drone']), state: null }, 3), false, "the person's word wins")

// ---- a bridge without /api/fleet ----------------------------------------------------------
assert.equal(hubAt(KERB, HUBS)?.id, 'hub-marina-kerb')
assert.equal(hubAt([-122.43, 37.80], HUBS), null, 'away from every hub')
const order = (kind: string, state: string, params: Record<string, unknown> = {}): OrderRef => ({
  tasking: { id: 1, intent_id: 9, task: 'transit', params: {} },
  op: { state: state as never, detail: '' },
  intent: { id: 9, kind, params, uids: ['1'] },
})
const away: LngLat = [-122.43, 37.802]
assert.equal(fallbackState(vehicle(1, 'drone', MARINA), HUBS, undefined, false).state, 'ready')
assert.equal(fallbackState(vehicle(1, 'drone', MARINA, { battery_pct: 30 }), HUBS, undefined, false).state, 'charging')
assert.equal(fallbackState(vehicle(1, 'drone', away), HUBS, undefined, false).state, 'attention', 'stopped away from a hub')
assert.equal(fallbackState(vehicle(1, 'drone', MARINA, { link: { since_last_ms: 9000, loss_pct: 0, rtt_ms: 0 } }), HUBS, undefined, false).state, 'offline')
assert.equal(fallbackState(vehicle(1, 'drone', MARINA), HUBS, undefined, true).state, 'attention', 'an order or alert waits for a person')
assert.equal(fallbackState(vehicle(1, 'drone', away), HUBS, order('deliver', 'active', { parcel: 'p-1', drop_zone: 'dz-1', at: [-122.40, 37.80] }), false).state, 'outbound')
assert.equal(fallbackState(vehicle(1, 'drone', [-122.40, 37.8002]), HUBS, order('deliver', 'active', { at: [-122.40, 37.80] }), false).state, 'delivering')
assert.equal(fallbackState(vehicle(1, 'drone', away), HUBS, order('return_to_hub', 'active'), false).state, 'returning')
assert.equal(fallbackState(vehicle(1, 'drone', MARINA), HUBS, order('pickup', 'acked'), false).state, 'loading')
assert.equal(fallbackState(vehicle(1, 'drone', away), HUBS, order('deliver', 'done'), false).state, 'attention', 'an ended order is no work')
const fb = fallbackFleet([vehicle(1, 'drone', MARINA), vehicle(2, 'car', KERB), vehicle(3, 'drone', away, { home: PIER })], HUBS, new Map(), new Set())
assert.ok(fb.derived)
assert.deepEqual(fb.vehicles.map((v) => v.base), ['hub-marina', 'hub-marina-kerb', 'hub-pier'], "each one's base: the hub its autopilot's home is at")
assert.deepEqual(fb.bases.find((b) => b.id === 'hub-pier')?.counts, { attention: 1 })
assert.deepEqual(fb.bases.find((b) => b.id === 'hub-marina')?.parked, { ready: 1 })

console.log('fleetModel: ok')
