// Checks for delivery.ts: node web/src/delivery.check.ts (prints "delivery: ok"). The claims: a
// parcel is late only while open and past its due time; each filter holds the parcels its name says
// and the counts agree with the lists; the due time reads in words; the operator-load readout counts
// what waits, the oldest wait and the decisions of a person in ten minutes (a check's rejection, an
// expiry and a voiding are not decisions); payload and navigation words.
import assert from 'node:assert/strict'
import { decisionsIn, dueText, failureText, filterCounts, filterParcels, inFilter, isLate, loadTone, loadWords, navOf, navWords, observationText, observationTone, operatorLoad, parcelAt, payloadWords, spanText, waitingSince, type DropZone, type Hub, type Parcel } from './delivery.ts'

const p = (id: string, state: string, due_s: number, o: Partial<Parcel> = {}): Parcel => ({ id, state, from: 'hub-a', to: 'dz-1', mass_kg: 1, priority: 2, class: 'normal', ready_s: 0, due_s, vehicle: null, tries: 0, ...o })

// late: open and past due
assert.equal(isLate(p('a', 'waiting', 100), 99), false)
assert.equal(isLate(p('a', 'waiting', 100), 101), true)
assert.equal(isLate(p('a', 'loaded', 100), 500), true, 'on its way and past due is late')
assert.equal(isLate(p('a', 'delivered', 100), 500), false, 'delivered is never late')
assert.equal(isLate(p('a', 'failed', 100), 500), false)

const all = [p('w1', 'waiting', 900), p('r1', 'ready', 50), p('a1', 'assigned', 70), p('t1', 'loaded', 400), p('d1', 'delivered', 10), p('f1', 'failed', 10)]
const now = 100
assert.deepEqual(filterParcels(all, 'waiting', now).map((x) => x.id), ['r1', 'w1'], 'waiting holds waiting and ready; the late one first')
assert.deepEqual(filterParcels(all, 'assigned', now).map((x) => x.id), ['a1'])
assert.deepEqual(filterParcels(all, 'transit', now).map((x) => x.id), ['t1'], 'in transit is loaded')
assert.deepEqual(filterParcels(all, 'delivered', now).map((x) => x.id), ['d1'])
assert.deepEqual(filterParcels(all, 'late', now).map((x) => x.id), ['r1', 'a1'], 'late: open and past due, by due time')
assert.equal(filterParcels(all, 'all', now).length, 6)
const c = filterCounts(all, now)
assert.deepEqual(c, { all: 6, waiting: 2, assigned: 1, transit: 1, delivered: 1, late: 2 })
for (const f of ['waiting', 'assigned', 'transit', 'delivered', 'late'] as const) assert.equal(all.filter((x) => inFilter(x, f, now)).length, c[f], `count of ${f} agrees with its list`)
// a late parcel is first in its list
assert.equal(filterParcels([p('x', 'waiting', 500), p('y', 'waiting', 50)], 'waiting', 100)[0].id, 'y')

// words
assert.equal(spanText(45), '45 s')
assert.equal(spanText(600), '10 min')
assert.equal(spanText(5400 + 300), '1 h 35 min')
// why a parcel failed, in a line; nothing for one that has not
assert.equal(failureText(p('a', 'delivered', 10)), '')
assert.equal(
  failureText(p('f', 'failed', 10, { failure: { stage: 'deliver', ended: 'rejected', codes: ['SEP_ROUTE_CONFLICT'], detail: '14: predicted loss of separation at +120 s', at_s: 761 } })),
  'deliver rejected (SEP_ROUTE_CONFLICT): 14: predicted loss of separation at +120 s',
)
assert.equal(failureText(p('f', 'failed', 10, { failure: { stage: 'look', ended: 'kept', codes: ['DROP_NOT_CLEAR'], detail: 'not clear', at_s: 1 } })), 'look kept (DROP_NOT_CLEAR): not clear')
assert.equal(dueText(p('a', 'waiting', 700), 100), 'due in 10 min')
assert.equal(dueText(p('a', 'waiting', 100), 280), 'late by 3 min')
assert.equal(dueText(p('a', 'delivered', 100), 280), 'delivered')

// where a parcel is
const hubs = new Map<string, Hub>([['hub-a', { id: 'hub-a', name: 'A', at: [1, 2], radius_m: 50, pads: 2, packs: 3, open: true }]])
const zones = new Map<string, DropZone>([['dz-1', { id: 'dz-1', kind: 'doorstep', at: [3, 4], radius_m: 4, surface: 'land', closed: false, populated: false, hub: null, observation: null }]])
const veh = (u: string): [number, number] | null => (u === '7' ? [5, 6] : null)
assert.deepEqual(parcelAt(p('a', 'waiting', 1), hubs, zones, veh), [1, 2])
assert.deepEqual(parcelAt(p('a', 'assigned', 1), hubs, zones, veh), [1, 2])
assert.deepEqual(parcelAt(p('a', 'loaded', 1, { vehicle: '7' }), hubs, zones, veh), [5, 6], 'on its vehicle while loaded')
assert.deepEqual(parcelAt(p('a', 'loaded', 1, { vehicle: '9' }), hubs, zones, veh), [1, 2], 'a vehicle with no position: its hub')
assert.deepEqual(parcelAt(p('a', 'delivered', 1), hubs, zones, veh), [3, 4])
assert.equal(parcelAt(p('a', 'failed', 1), hubs, zones, veh), null)

// observations
assert.equal(observationText(null), 'not looked at')
assert.equal(observationText({ state: 'occupied', count: 2, age_s: 12 }), 'occupied: 2 people seen, 12 s ago')
assert.equal(observationText({ state: 'clear', count: 0, age_s: 0.4 }), 'clear, just now')
assert.equal(observationTone(null), 'unknown')
assert.equal(observationTone({ state: 'clear', count: 0, age_s: 1 }), 'ok')
assert.equal(observationTone({ state: 'occupied', count: 1, age_s: 1 }), 'bad')
assert.equal(observationTone({ state: 'clear', count: 0, age_s: 1 }, true), 'bad', 'a closed zone is bad whatever was seen')

// payload and navigation
assert.match(payloadWords('loaded'), /^Loaded/)
assert.match(payloadWords('released'), /^Released/)
assert.match(payloadWords('empty'), /^Empty/)
assert.equal(payloadWords(undefined), 'Not reported')
for (const s of ['trusted', 'degraded', 'untrusted', 'unknown']) assert.match(navWords(s).toLowerCase(), new RegExp(`^${s}`))
assert.equal(navOf({ vehicles: { '3': { state: 'untrusted' } } }, 3), 'untrusted')
assert.equal(navOf({ vehicles: {} }, 3), 'unknown', 'no row: unknown, never trusted by default')

// the operator's load
const pend = (created: number, pendingAt?: number) => ({ created_t: created, history: [{ state: 'proposed', t: created }, ...(pendingAt != null ? [{ state: 'pending_approval', t: pendingAt }] : [])] })
assert.equal(waitingSince(pend(1000, 5000)), 5000, 'it waits from its move to pending')
assert.equal(waitingSince(pend(1000)), 1000, 'else from its creation')
const NOW = 1_000_000
const l = operatorLoad([pend(0, NOW - 90_000), pend(0, NOW - 30_000)], NOW, 7)
assert.deepEqual(l, { waiting: 2, oldestS: 90, decided: 7 })
assert.deepEqual(operatorLoad([], NOW, 0), { waiting: 0, oldestS: null, decided: 0 })
assert.equal(loadWords(l), '2 waiting · oldest 2 min · 7 decided in 10 min')
assert.equal(loadWords({ waiting: 0, oldestS: null, decided: 3 }), '0 waiting · 3 decided in 10 min')
assert.equal(loadTone({ waiting: 0, oldestS: null, decided: 0 }), 'quiet')
assert.equal(loadTone({ waiting: 3, oldestS: 10, decided: 0 }), 'busy')
assert.equal(loadTone({ waiting: 1, oldestS: 400, decided: 0 }), 'heavy')
// decisions from intent histories when the bridge sends no count
const h = (...steps: [string, number][]) => ({ created_t: 0, history: steps.map(([state, t]) => ({ state, t })) })
const intents = [
  h(['checked', 1], ['pending_approval', 2], ['approved', NOW - 100_000]),
  h(['checked', 1], ['pending_approval', 2], ['rejected', NOW - 50_000]),
  h(['checked', 1], ['rejected', NOW - 10_000]), // refused by the checks: not a decision
  h(['checked', 1], ['pending_approval', 2], ['expired', NOW - 10_000]), // nobody decided
  h(['checked', 1], ['pending_approval', 2], ['approved', NOW - 700_000]), // too long ago
]
assert.equal(decisionsIn(intents, NOW, 600_000), 2)
assert.equal(operatorLoad([], NOW, undefined, intents).decided, 2, 'a bridge without the count: read from the intents')
assert.equal(operatorLoad([], NOW, 9, intents).decided, 9, 'the bridge count wins')

console.log('delivery: ok')
