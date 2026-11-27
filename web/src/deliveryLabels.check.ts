// Checks for deliveryLabelCandidates (deliveryLayers.ts): a hub's name is in the one label budget
// before the vehicles' labels, so a vehicle label that would print on a hub's gives way.
import assert from 'node:assert/strict'
import { deliveryLabelCandidates } from './deliveryLabels.ts'
import { planLabels, type LabelCandidate } from './declutter.ts'
import { parcelPlaces, type Hub, type Parcel } from './delivery.ts'

const hub = { id: 'hub-a', name: 'Marina car hub', at: [-122.443, 37.799], radius_m: 40, pads: 3, packs: 4, open: true } as Hub
const on = { hubs: true, zones: true, parcels: true, routes: true, areas: true }
const screen = { at: (lon: number, lat: number) => [(lon + 122.443) * 1e3 + 200, 200 - (lat - 37.799) * 1e3] as [number, number] }
const cands = deliveryLabelCandidates({ hubs: [hub], areas: [] }, on, screen as never)
assert.equal(cands.length, 1)
assert.equal(cands[0].key, 'dh:hub-a')
assert.ok(cands[0].full[2] > cands[0].short[2], 'the full text (with packs) is wider than the name')
assert.equal(deliveryLabelCandidates({ hubs: [hub], areas: [] }, { ...on, hubs: false }, screen as never).length, 0)

// a vehicle label right on the hub's is dropped to its short form or none; the hub keeps its name
const vehicle: LabelCandidate = { key: 'v:1', at: [200, 200], short: [14, -20, 70, -6], full: [14, -20, 160, -6], priority: 4 }
const plan = planLabels([vehicle, ...cands], { zoom: 12, width: 800, height: 600 })
assert.notEqual(plan.level('dh:hub-a'), 'none')
assert.notEqual(plan.level('v:1'), 'full', 'the vehicle label does not print over the hub name in full')
// a place's parcel count is in the budget beside its mark, below and right of the hub's ring
// (its name is above): one candidate per place, none for a vehicle carrying its one parcel
const parcel = (id: string, state: string, vehicle: string | null = null): Parcel => ({ id, state, from: 'hub-a', to: 'dz-1', mass_kg: 1, priority: 2, class: 'normal', ready_s: 0, due_s: 900, vehicle, tries: 0 })
const places = parcelPlaces([parcel('a', 'ready'), parcel('b', 'ready'), parcel('c', 'loaded', '4')], new Map([[hub.id, hub]]), new Map(), () => [-122.44, 37.8], 0)
const withParcels = deliveryLabelCandidates({ hubs: [hub], areas: [], places }, on, screen as never)
assert.deepEqual(withParcels.map((c) => c.key), ['dh:hub-a', 'dp:hub:hub-a'])
const [name, count] = withParcels
assert.ok(count.full[1] > name.full[3], 'the count sits below the hub name')
assert.ok(count.full[0] > 0, 'right of the hub point')
assert.equal(deliveryLabelCandidates({ hubs: [hub], areas: [], places }, { ...on, parcels: false }, screen as never).length, 1)
console.log('deliveryLabels.check ok')
