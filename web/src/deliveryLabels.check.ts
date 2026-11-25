// Checks for deliveryLabelCandidates (deliveryLayers.ts): a hub's name is in the one label budget
// before the vehicles' labels, so a vehicle label that would print on a hub's gives way.
import assert from 'node:assert/strict'
import { deliveryLabelCandidates } from './deliveryLabels.ts'
import { planLabels, type LabelCandidate } from './declutter.ts'
import type { Hub } from './delivery.ts'

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
console.log('deliveryLabels.check ok')
