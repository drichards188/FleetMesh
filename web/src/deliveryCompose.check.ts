// Checks for deliveryCompose.ts: node web/src/deliveryCompose.check.ts (prints "deliveryCompose: ok").
// The claims: each delivery order says what is missing before it can be sent; a parcel's order takes
// one vehicle; the request has the fields the bridge reads; the safe orders go one per vehicle; a
// pickup needs a parcel not yet loaded, a deliver one that is loaded.
import assert from 'node:assert/strict'
import { compose, EMPTY_SPEC, parcelChoices, stoppableFor, type ComposeSpec, type ComposeWorld } from './deliveryCompose.ts'

const world: ComposeWorld = {
  parcels: [
    { id: 'p-1', state: 'waiting', from: 'hub-a', to: 'dz-1' },
    { id: 'p-2', state: 'loaded', from: 'hub-a', to: 'dz-2' },
    { id: 'p-3', state: 'delivered', from: 'hub-a', to: 'dz-1' },
  ],
  hubs: [{ id: 'hub-a', open: true }],
  dropZones: [{ id: 'dz-1', closed: false }, { id: 'dz-2', closed: false }],
  stoppable: [{ id: 41, uids: ['1'] }],
}
const spec = (o: Partial<ComposeSpec>): ComposeSpec => ({ ...EMPTY_SPEC, uids: [1], ...o })

// nothing selected
assert.match(compose(spec({ uids: [] }), world).problems[0], /Select at least one vehicle/)

// pickup
let c = compose(spec({ kind: 'pickup' }), world)
assert.equal(c.requests.length, 0)
assert.ok(c.problems.some((p) => /parcel/.test(p)) && c.problems.some((p) => /hub/.test(p)), 'it says the parcel and the hub are missing')
c = compose(spec({ kind: 'pickup', parcel: 'p-1', hub: 'hub-a' }), world)
assert.deepEqual(c.problems, [])
assert.deepEqual(c.requests, [{ task: 'pickup', uids: [1], parcel: 'p-1', site: 'hub-a' }], 'the bridge reads the hub as `site`')
assert.match(compose(spec({ kind: 'pickup', parcel: 'p-2', hub: 'hub-a' }), world).problems[0], /already loaded/)
assert.match(compose(spec({ kind: 'pickup', parcel: 'p-3', hub: 'hub-a' }), world).problems[0], /delivered/)
assert.match(compose(spec({ kind: 'pickup', parcel: 'p-9', hub: 'hub-a' }), world).problems[0], /No parcel p-9/)
assert.match(compose(spec({ kind: 'pickup', parcel: 'p-1', hub: 'hub-z' }), world).problems[0], /No hub hub-z/)

// a parcel's order is one vehicle's
assert.match(compose(spec({ kind: 'pickup', uids: [1, 2], parcel: 'p-1', hub: 'hub-a' }), world).problems[0], /one vehicle/)
assert.match(compose(spec({ kind: 'deliver', uids: [1, 2], parcel: 'p-2', dropZone: 'dz-2' }), world).problems[0], /one vehicle/)

// deliver
c = compose(spec({ kind: 'deliver', parcel: 'p-2', dropZone: 'dz-2', release: 'winch' }), world)
assert.deepEqual(c.requests, [{ task: 'deliver', uids: [1], parcel: 'p-2', drop_zone: 'dz-2', release: 'winch' }])
c = compose(spec({ kind: 'deliver', parcel: 'p-2', dropZone: 'dz-2' }), world)
assert.equal('release' in c.requests[0], false, 'no release chosen: the engine decides')
assert.match(compose(spec({ kind: 'deliver', parcel: 'p-1', dropZone: 'dz-1' }), world).problems[0], /not loaded/)
assert.match(compose(spec({ kind: 'deliver', parcel: 'p-2', dropZone: 'dz-9' }), world).problems[0], /No drop zone dz-9/)
assert.ok(compose(spec({ kind: 'deliver', parcel: 'p-2' }), world).problems.some((p) => /drop zone/i.test(p)))

// reroute
assert.match(compose(spec({ kind: 'reroute' }), world).problems[0], /Click the map/)
c = compose(spec({ kind: 'reroute', route: [[1, 2], [3, 4]], cause: 'weather' }), world)
assert.deepEqual(c.requests, [{ task: 'reroute', uids: [1], route: [[1, 2], [3, 4]], cause: 'weather' }])

// the safe orders: one request per vehicle
c = compose(spec({ kind: 'land_now', uids: [1, 2, 3] }), world)
assert.deepEqual(c.requests.map((r) => r.uids), [[1], [2], [3]])
assert.deepEqual(compose(spec({ kind: 'return_to_hub' }), world).requests, [{ task: 'return_to_hub', uids: [1] }], 'no hub: its own way home')
assert.deepEqual(compose(spec({ kind: 'return_to_hub', hub: 'hub-a' }), world).requests, [{ task: 'return_to_hub', uids: [1], hub: 'hub-a' }])
assert.match(compose(spec({ kind: 'return_to_hub', hub: 'hub-z' }), world).problems[0], /No hub/)
assert.match(compose(spec({ kind: 'swap_battery' }), world).problems[0], /hub/)
assert.deepEqual(compose(spec({ kind: 'swap_battery', hub: 'hub-a', pad: ' pad-3 ' }), world).requests, [{ task: 'swap_battery', uids: [1], hub: 'hub-a', pad: 'pad-3' }])

// abort
assert.match(compose(spec({ kind: 'abort' }), world).problems[0], /order to stop/)
assert.match(compose(spec({ kind: 'abort', target: 7 }), world).problems[0], /not running/)
assert.deepEqual(compose(spec({ kind: 'abort', target: 41, then: 'hold' }), world).requests, [{ task: 'abort', uids: [1], target: 41, then: 'hold' }])

// the pickers
const ps = [{ id: 'p-b', state: 'waiting', vehicle: null }, { id: 'p-a', state: 'assigned', vehicle: '4' }, { id: 'p-c', state: 'loaded', vehicle: '4' }, { id: 'p-d', state: 'delivered', vehicle: null }]
assert.deepEqual(parcelChoices('pickup', ps, 4).map((p) => p.id), ['p-a', 'p-b'], 'a pickup offers what is not loaded or done, the vehicle’s own first')
assert.deepEqual(parcelChoices('deliver', ps, 4).map((p) => p.id), ['p-c'], 'a delivery offers what is loaded')
const its = [{ id: 5, kind: 'deliver', state: 'dispatched', uids: ['4'] }, { id: 6, kind: 'pickup', state: 'done', uids: ['4'] }, { id: 7, kind: 'pickup', state: 'dispatched', uids: ['9'] }, { id: 8, kind: 'abort', state: 'dispatched', uids: ['4'] }]
assert.deepEqual(stoppableFor(its, 4).map((i) => i.id), [5], 'only its own running order, never an abort')
assert.deepEqual(stoppableFor(its, null), [])

console.log('deliveryCompose: ok')
