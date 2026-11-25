// Checks for orderWords.ts (no test runner in web/): node web/src/orderWords.check.ts, as make
// check does. The claims (R3, ledger 1 "Needs a new piece" 1): without an order a vehicle is moving
// or idle, returning home by its mode; with one, the order's words by its task and its op's state,
// with the distance while en route; a group order
// names its group; a mode that takes it home overrides an order that does not; a failed order says
// nothing of what it does; and the command log's one word per op. Throws on the first failure,
// prints "orderWords: ok" otherwise. Not part of the bundle.
import assert from 'node:assert/strict'
import { doingNow, doingWords, latestOrders, opWord, opWordIsBad, type OrderIntent, type OrderRef } from './orderWords.ts'
import type { LngLat } from './types.ts'

const here: LngLat = [-122.4783, 37.8199]
const north = (m: number): LngLat => [here[0], here[1] + m / 111_195]
const v = (o: { mode?: string; groundspeed?: number; position?: LngLat } = {}) => ({ mode: o.mode ?? 'AUTO', groundspeed: o.groundspeed ?? 0, position: o.position ?? here })
const ref = (task: string, state: string, params: Record<string, unknown> = {}, intent?: OrderIntent, parent?: OrderIntent): OrderRef => ({
  tasking: { id: 1, intent_id: intent?.id, task: task as never, params },
  op: { state: state as never, detail: '' },
  intent,
  parent,
})

// no order: vehicleState's words
assert.equal(doingWords(v(), undefined), 'idle')
assert.equal(doingWords(v({ groundspeed: 12 }), undefined), 'moving')
assert.equal(doingWords(v({ mode: 'RTL', groundspeed: 12 }), undefined), 'returning home')

// by task, running
assert.equal(doingWords(v(), ref('patrol', 'active', { route: [north(500), north(900)] })), 'patrolling')
assert.equal(doingWords(v(), ref('hold', 'done')), 'holding')
assert.equal(doingWords(v(), ref('rtl', 'active')), 'returning home')
assert.equal(doingWords(v(), ref('rtl', 'done')), 'home')
// an orbit: en route with the distance to its ring, then orbiting
assert.equal(doingWords(v(), ref('orbit', 'active', { center: north(2500), radius: 400 })), 'en route to its orbit, 2.1 km')
assert.equal(doingWords(v({ position: north(2100) }), ref('orbit', 'active', { center: north(2500), radius: 400 })), 'orbiting')
assert.equal(doingWords(v(), ref('orbit', 'done', { center: north(2500), radius: 400 })), 'orbiting', 'an orbit done is established')
// a transit: the way to its end, then holding there
assert.equal(doingWords(v(), ref('transit', 'active', { route: [north(300), north(800)] })), 'en route, 800 m to go')
assert.equal(doingWords(v(), ref('transit', 'done', { route: [north(300), north(800)] })), 'holding at the end of its route')

// before it runs
assert.equal(doingWords(v(), ref('patrol', 'sent')), 'starting its patrol')
assert.equal(doingWords(v(), ref('orbit', 'queued')), 'waiting for a position fix to start its orbit')

// a group order names its group, a member's share its group order's
const group: OrderIntent = { id: 78, kind: 'patrol', params: {}, uids: ['1', '2'], group: { swarm: 'North route' } }
const share: OrderIntent = { id: 80, kind: 'patrol', params: {}, uids: ['1'], parent: 78 }
assert.equal(doingWords(v(), ref('patrol', 'active', {}, share, group)), 'patrolling · North route')

// a mode that takes it home overrides an order that does not
assert.equal(doingWords(v({ mode: 'RTL' }), ref('patrol', 'active')), 'returning home')
// an order that failed or was replaced no longer says what it does
assert.equal(doingWords(v({ groundspeed: 5 }), ref('orbit', 'failed')), 'moving')
assert.equal(doingWords(v(), ref('orbit', 'superseded')), 'idle')
assert.equal(doingWords(v(), ref('orbit', 'requeued')), 'idle; its orbit back in the queue')

// the latest order per vehicle: of the newest tasking, with its intent and group order
const intents = new Map<number, OrderIntent>([[78, group], [80, share], [2, { id: 2, kind: 'orbit', params: {}, uids: ['1'] }]])
const latest = latestOrders(
  [
    { id: 3, intent_id: 80, task: 'patrol', params: {}, ops: [{ id: 1, tasking: 3, task: 'patrol', uid: 1, sysid: 1, state: 'active', detail: '', progress: null, history: [] }] },
    { id: 1, intent_id: 2, task: 'orbit', params: {}, ops: [{ id: 2, tasking: 1, task: 'orbit', uid: 1, sysid: 1, state: 'done', detail: '', progress: null, history: [] }] },
  ],
  intents,
)
assert.equal(latest.get(1)?.tasking.id, 3, 'the newest tasking wins')
assert.equal(latest.get(1)?.parent?.id, 78, 'with its group order')

// the command log's word
assert.equal(opWord({ state: 'active', detail: '' }, 'orbit'), 'running')
assert.equal(opWord({ state: 'done', detail: '' }, 'orbit'), 'running', 'an orbit done goes on')
assert.equal(opWord({ state: 'done', detail: '' }, 'rtl'), 'done')
assert.equal(opWord({ state: 'acked', detail: '' }, 'patrol'), 'starting')
assert.equal(opWord({ state: 'superseded', detail: '' }, 'patrol'), 'replaced')
assert.equal(opWord({ state: 'failed', detail: 'not sent before the bridge restarted' }, 'patrol'), 'not sent')
assert.equal(opWord({ state: 'unconfirmed', detail: 'the vehicle does not run it now' }, 'patrol'), 'not running')
assert.ok(opWordIsBad('failed') && !opWordIsBad('running') && !opWordIsBad('replaced'))

// one not heard from is said as last heard
{
  const still = { mode: 'AUTO', groundspeed: 0, position: null } as unknown as Parameters<typeof doingNow>[0]
  assert.equal(doingNow(still, undefined, { silent: true }), 'last heard idle')
  assert.equal(doingNow(still, undefined), 'idle')
}

console.log('orderWords: ok')
