// Checks for hubSummary.ts: node web/src/hubSummary.check.ts (prints "hubSummary: ok"). The claims:
// the wind is judged against the limits of what is based at the hub, the worse of gust and mean wind
// counting; a hub's line puts what needs the person first, then whether it can fly, then what its
// vehicles do, three parts at most; its tone is the worst of those; a site is the worst of its hubs;
// the details' summary says the same in sentences, and its parcels are the hub's own.
import assert from 'node:assert/strict'
import type { Parcel } from './delivery.ts'
import { hubDetails, hubParcels, hubView, judgeWind, platformWord, siteView, stateWords, type HubWind } from './hubSummary.ts'

const drone = (gust: number, wind = 4): HubWind => ({ gust_mps: gust, wind_mps: wind, platform: 'delivery_drone', gust_soft_mps: 11, gust_hard_mps: 14, wind_soft_mps: 8, wind_hard_mps: 10 })

// the wind against the limits
assert.equal(judgeWind(null), null)
assert.equal(judgeWind({ ...drone(9), gust_soft_mps: null, gust_hard_mps: null }), null, 'no limits (a car hub): nothing to judge')
assert.equal(judgeWind(drone(9))?.band, 'ok')
assert.equal(judgeWind(drone(11))?.band, 'marginal', 'at the soft limit: held')
assert.equal(judgeWind(drone(14))?.band, 'over', 'at the hard limit: refused')
assert.deepEqual(judgeWind(drone(9, 9)), { band: 'marginal', by: 'wind', value: 9, soft: 8, hard: 10, who: 'drones' }, 'the mean wind when it is the worse')
assert.equal(judgeWind(drone(12, 9))?.by, 'gust', 'a tie of bands: the gust')
assert.equal(platformWord('delivery_drone'), 'drones')
assert.equal(platformWord('delivery_boats'), 'boats')
assert.equal(platformWord(null), 'vehicles')

// a hub's line and tone
const base = { id: 'hub-crissy', name: 'Crissy Field hub', counts: {} }
assert.deepEqual(hubView({ ...base, counts: { ready: 32, outbound: 4, delivering: 2 }, weather: drone(7) }), {
  id: 'hub-crissy',
  name: 'Crissy Field hub',
  tone: 'quiet',
  line: '6 out · 32 ready',
  needYou: 0,
  wind: { band: 'ok', by: 'gust', value: 7, soft: 11, hard: 14, who: 'drones' },
})
const marginal = hubView({ ...base, counts: { attention: 2, holding: 18, outbound: 6, ready: 12 }, weather: drone(12.5) })
assert.equal(marginal.line, '2 need you · Marginal: gusts 12.5 m/s · 18 holding', 'what needs the person, then the wind, then the rest; three parts')
assert.equal(marginal.tone, 'bad', 'someone needed: red')
assert.equal(hubView({ ...base, counts: { holding: 18, ready: 12 }, weather: drone(12.5) }).tone, 'warn')
assert.equal(hubView({ ...base, counts: { ready: 40 }, weather: drone(15.2) }).line, 'Grounded: gusts 15.2 m/s · 40 ready')
assert.equal(hubView({ ...base, counts: { ready: 40 }, weather: drone(15.2) }).tone, 'bad')
assert.equal(hubView({ ...base, counts: { ready: 2 }, open: false }).tone, 'closed')
assert.equal(hubView({ ...base, counts: { offline: 1 } }).line, '1 needs you', 'a silent one is looked at too')
assert.equal(hubView({ ...base, counts: {} }).line, 'no vehicles')

// a site: its hubs together, the worst deciding
const site = siteView('marina', 'Marina', [
  { id: 'a', name: 'A', counts: { ready: 10 }, weather: drone(12) },
  { id: 'b', name: 'B', counts: { ready: 3, outbound: 2 }, weather: null },
])
assert.equal(site.line, 'Marginal: gusts 12.0 m/s · 2 out · 13 ready')
assert.equal(site.tone, 'warn')

// the details
const p = (id: string, state: string, due_s: number, from = 'hub-crissy'): Parcel => ({ id, state, from, to: 'dz-1', mass_kg: 1, priority: 2, class: 'normal', ready_s: 0, due_s, vehicle: null, tries: 0 })
const parcels = [p('a', 'ready', 50), p('b', 'ready', 400), p('c', 'assigned', 700), p('d', 'loaded', 900), p('e', 'delivered', 10), p('f', 'waiting', 2000), p('g', 'ready', 10, 'hub-other')]
assert.deepEqual(hubParcels('hub-crissy', parcels, 100), { waiting: 3, late: 1, nextDueS: 300, inTransit: 1, delivered: 1, failed: 0, later: 1 })
const d = hubDetails({ ...base, counts: { attention: 2, holding: 18, outbound: 6, ready: 12 }, weather: drone(12.5), vehicles: 38, here: 30, visitors: 0, pads: 40, packs: 37 }, parcels, 100)
assert.deepEqual(d.summary, [
  "Gusts of 12.5 m/s are over the drones' 11.0 m/s soft limit: new deliveries are held for a decision.",
  '2 of its vehicles need you.',
  'Of its vehicles, 6 out on deliveries, 18 holding and 12 ready.',
  '3 parcels wait here, 1 late; the next is due in 5 min.',
])
assert.deepEqual(d.fleet.map((s) => s.state), ['attention', 'holding', 'ready', 'outbound'], "the Fleet list's order: problems first")
assert.equal(hubDetails({ ...base, counts: { ready: 40 }, weather: drone(15.2) }, [], 0).summary[0], "Gusts of 15.2 m/s are over the drones' 14.0 m/s limit: no new delivery leaves, and those in the air are sent home.")
assert.equal(hubDetails({ ...base, counts: { ready: 4 } }, [], 0).summary.join(' '), 'Operating normally. Nothing here needs you. Of its vehicles, 4 ready.')
// the header's words: whether it can fly first
assert.deepEqual(stateWords(marginal), { words: 'Marginal wind', tone: 'warn' })
assert.deepEqual(stateWords(hubView({ ...base, counts: { attention: 1 } })), { words: 'Needs you', tone: 'bad' })
assert.equal(stateWords(hubView({ ...base, counts: { ready: 3 }, weather: drone(15) })).words, 'Grounded')
assert.equal(stateWords(hubView({ ...base, counts: { holding: 3 } })).words, 'Holding')
assert.deepEqual(stateWords(hubView({ ...base, counts: { ready: 3 } })), { words: 'Operating normally', tone: 'ok' })
assert.equal(stateWords(hubView({ ...base, counts: {}, open: false }), true).words, 'Closed')
// boats are stopped, not grounded
const boat: HubWind = { ...drone(17), platform: 'delivery_boat', gust_soft_mps: 13, gust_hard_mps: 16 }
assert.equal(hubView({ ...base, counts: { ready: 4 }, weather: boat }).line, 'Stopped: gusts 17.0 m/s · 4 ready')
console.log('hubSummary: ok')
