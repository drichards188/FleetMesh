// Checks for deliveryLabelCandidates (deliveryLabels.ts): a place's parcel count is in the one label
// budget beside its mark, a hub's under the hub's house (its name and state are right of the house),
// one candidate per place and none for a vehicle carrying its one parcel; a vehicle label that would
// print on it gives way.
import assert from 'node:assert/strict'
import { deliveryLabelCandidates, parcelOffset } from './deliveryLabels.ts'
import { planLabels, type LabelCandidate } from './declutter.ts'
import { parcelPlaces, type Hub, type Parcel } from './delivery.ts'
import { baseLabelCandidates, MARK_BOX, type BaseMarker } from './fleetMap.ts'

const hub = { id: 'hub-a', name: 'Marina car hub', at: [-122.443, 37.799], radius_m: 40, pads: 3, packs: 4, open: true } as Hub
const on = { areas: true, parcels: true }
const screen = { at: (lon: number, lat: number) => [(lon + 122.443) * 1e3 + 200, 200 - (lat - 37.799) * 1e3] as [number, number] }
const parcel = (id: string, state: string, vehicle: string | null = null): Parcel => ({ id, state, from: 'hub-a', to: 'dz-1', mass_kg: 1, priority: 2, class: 'normal', ready_s: 0, due_s: 900, vehicle, tries: 0 })
const places = parcelPlaces([parcel('a', 'ready'), parcel('b', 'ready'), parcel('c', 'loaded', '4')], new Map([[hub.id, hub]]), new Map(), () => [-122.44, 37.8], 0)

const cands = deliveryLabelCandidates({ areas: [], places }, on, screen as never)
assert.deepEqual(cands.map((c) => c.key), ['dp:hub:hub-a'], "one for the hub's two; none for the vehicle's one")
const [count] = cands
const [, dy] = parcelOffset(places[0], screen as never)
assert.ok(dy > 0, "under the hub's point")
assert.ok(count.full[1] >= MARK_BOX[3], "the count's room starts under the house's foot")
// clear of the hub's own name and line, right of its house, whatever the line says
const mark = { id: 'base:hub-a', name: hub.name, at: hub.at, open: false, view: { line: '2 need you · Marginal: gusts 12.5 m/s · 18 holding' } } as BaseMarker
const name = baseLabelCandidates([mark], screen as never).find((c) => c.key === 'fb:base:hub-a')!
const wide = deliveryLabelCandidates({ areas: [], places: [{ ...places[0], parcels: Array(424).fill(places[0].parcels[0]) }] }, on, screen as never)[0]
const overlap = (a: number[], b: number[]) => a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
assert.ok(!overlap(count.full, name.full) && !overlap(wide.full, name.full), `${count.full} or ${wide.full} overlaps ${name.full}`)
assert.equal(deliveryLabelCandidates({ areas: [], places }, { ...on, parcels: false }, screen as never).length, 0)

// a vehicle label right on the count is dropped to its short form or none; the count keeps its place
const vehicle: LabelCandidate = { key: 'v:1', at: [200, 200], short: [-10, 20, 60, 34], full: [-10, 20, 160, 34], priority: 4 }
const plan = planLabels([vehicle, ...cands], { zoom: 12, width: 800, height: 600 })
assert.notEqual(plan.level('dp:hub:hub-a'), 'none')
assert.notEqual(plan.level('v:1'), 'full', 'the vehicle label does not print over the count in full')
console.log('deliveryLabels.check ok')
