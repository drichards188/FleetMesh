// Checks for fleetMap.ts: node web/src/fleetMap.check.ts (prints "fleetMap: ok"), as make check runs
// it. The claims (docs/design/fleet-organisation.md): every hub has a mark of its own at every zoom,
// with its state's line; the vehicles parked at their own base (ready or charging) collapse into it,
// with its counts; zoomed out, the hubs of a site share one mark; a vehicle away from its base,
// under way, in trouble, selected or followed stays on its own; opening a hub draws its vehicles one
// by one (the mark hollow), and from EXPAND_ZOOM in nothing is collapsed but the marks stay.
import assert from 'node:assert/strict'
import { NO_SCREEN } from './aloft.ts'
import { BADGE, MARK_BOX, baseLabelCandidates, collapseAtBases, EXPAND_ZOOM, MIN_COLLAPSE, SITE_ZOOM, toggleExpanded } from './fleetMap.ts'
import { type FleetDoc, type FleetVehicle, FLEET_STATES } from './fleetModel.ts'
import type { LngLat } from './types'

const MARINA: LngLat = [-122.437, 37.8057]
const KERB: LngLat = [-122.436, 37.8045]
const PIER: LngLat = [-122.41, 37.8089]

const fv = (uid: number, base: string | null, at_hub: string | null, state: FleetVehicle['state']): FleetVehicle => ({ uid, base, at_hub, state, why: '', since_s: 0, parcel: null })

const doc: FleetDoc = {
  states: FLEET_STATES,
  sites: [{ id: 'marina', name: 'Marina', hubs: ['hub-marina', 'hub-marina-kerb'] }],
  bases: [
    { id: 'hub-marina', name: 'Marina Green hub', at: MARINA, radius_m: 80, site: 'marina', vehicles: 7, counts: { attention: 1, ready: 3, loading: 1, outbound: 1, charging: 1 }, here: 5, visitors: 0, parked: {} },
    { id: 'hub-marina-kerb', name: 'Marina car kerb', at: KERB, radius_m: 40, site: 'marina', vehicles: 2, counts: { ready: 2 }, here: 2, visitors: 0, parked: {} },
    { id: 'hub-pier', name: 'Pier 39 hub', at: PIER, radius_m: 80, site: null, vehicles: 2, counts: { ready: 2 }, here: 3, visitors: 1, parked: {} },
  ],
  vehicles: [
    fv(1, 'hub-marina', 'hub-marina', 'ready'),
    fv(2, 'hub-marina', 'hub-marina', 'ready'),
    fv(3, 'hub-marina', 'hub-marina', 'charging'),
    fv(4, 'hub-marina', 'hub-marina', 'attention'), // in trouble: on its own
    fv(5, 'hub-marina', 'hub-marina', 'loading'), // about to leave: on its own
    fv(6, 'hub-marina', null, 'outbound'), // under way
    fv(7, 'hub-marina-kerb', 'hub-marina-kerb', 'ready'),
    fv(8, 'hub-marina-kerb', 'hub-marina-kerb', 'ready'),
    fv(9, 'hub-pier', 'hub-pier', 'ready'),
    fv(10, 'hub-marina', 'hub-pier', 'ready'), // a visitor: away from its base
    fv(11, 'hub-pier', 'hub-pier', 'ready'),
  ],
  counts: {},
}
const placed = new Set(doc.vehicles.map((v) => v.uid))
const none = { keep: new Set<number>(), expanded: new Set<string>() }

// at base level (zoomed in past the sites)
const bases = collapseAtBases(doc, placed, { ...none, zoom: SITE_ZOOM + 1 })
assert.deepEqual([...bases.collapsed].sort((a, b) => a - b), [1, 2, 3, 7, 8, 9, 11], 'parked at their own base: ready or charging')
assert.deepEqual(bases.markers.map((m) => m.id), ['base:hub-marina', 'base:hub-marina-kerb', 'base:hub-pier'])
assert.equal(bases.markers[0].text, 'Marina Green hub: 1 needs you · 2 out · 3 ready', "its name and its state's line")
assert.deepEqual(bases.markers[0].counts, { ready: 2, charging: 1 }, 'what is collapsed into it')
assert.equal(bases.markers[0].n, 3)
assert.ok(!bases.collapsed.has(10), 'a visitor stays on its own')
assert.ok(![4, 5, 6].some((u) => bases.collapsed.has(u)), 'in trouble, loading, under way: on their own')

// zoomed out: a site's bases share one marker, at their middle
const sites = collapseAtBases(doc, placed, { ...none, zoom: SITE_ZOOM - 2 })
assert.deepEqual(sites.markers.map((m) => m.id), ['site:marina', 'base:hub-pier'])
assert.equal(sites.markers[0].text, 'Marina: 1 needs you · 2 out · 5 ready')
assert.deepEqual(sites.markers[0].bases, ['hub-marina', 'hub-marina-kerb'])
assert.ok(Math.abs(sites.markers[0].at[0] - (MARINA[0] + KERB[0]) / 2) < 1e-9)
assert.deepEqual(collapseAtBases(doc, placed, { ...none, zoom: null }).markers.map((m) => m.id), sites.markers.map((m) => m.id), 'no zoom yet: as zoomed out')

// selected, followed or named by a waiting order: on its own; a base left with one parked does not collapse
const kept = collapseAtBases(doc, placed, { keep: new Set([9]), expanded: new Set(), zoom: SITE_ZOOM + 1 })
assert.ok(!kept.collapsed.has(9))
assert.ok(!kept.collapsed.has(11), `fewer than ${MIN_COLLAPSE} parked: drawn as itself`)
assert.deepEqual(kept.markers.find((m) => m.id === 'base:hub-pier')?.n, 0, 'a hub with nothing collapsed keeps its mark')
// without a position: never inside a marker (nowhere to be drawn anyway)
assert.ok(!collapseAtBases(doc, new Set([1, 3]), { ...none, zoom: SITE_ZOOM + 1 }).collapsed.has(2))

// a click opens a marker's bases: its vehicles one by one, a hollow marker to close it; again, closed
const opened = toggleExpanded(new Set(), sites.markers[0])
assert.deepEqual([...opened].sort(), ['hub-marina', 'hub-marina-kerb'])
const open = collapseAtBases(doc, placed, { keep: new Set(), expanded: opened, zoom: SITE_ZOOM - 2 })
assert.ok([1, 2, 3, 7, 8].every((u) => !open.collapsed.has(u)))
assert.deepEqual(open.markers.map((m) => [m.id, m.open]), [['base:hub-marina', true], ['base:hub-marina-kerb', true], ['base:hub-pier', false]], 'opened bases keep a marker of their own, out of the site')
assert.equal(open.markers[0].text, 'Marina Green hub: its vehicles shown one by one')
assert.deepEqual([...toggleExpanded(opened, open.markers[0])], ['hub-marina-kerb'], "a click on an open base's marker closes it")

// from EXPAND_ZOOM in: every vehicle on its own
const near = collapseAtBases(doc, placed, { ...none, zoom: EXPAND_ZOOM })
assert.equal(near.collapsed.size, 0)
assert.deepEqual(near.markers.map((m) => [m.id, m.n]), [['base:hub-marina', 0], ['base:hub-marina-kerb', 0], ['base:hub-pier', 0]], 'the marks stay, empty')

// the label budget: the house as an obstacle, its name and line beside it clear of the house's room
const cands = baseLabelCandidates(bases.markers, { ...NO_SCREEN, at: () => [100, 100] })
assert.equal(cands.length, 2 * bases.markers.length)
const disc = cands.find((c) => c.key === 'fbd:base:hub-marina')
const label = cands.find((c) => c.key === 'fb:base:hub-marina')
assert.ok(disc?.obstacle && label && !label.obstacle && label.priority === 2)
assert.ok(label.full[0] - 2 >= disc.full[2] + 2, 'the label starts clear of the house and both their pads')
assert.deepEqual(disc.full, MARK_BOX)
assert.ok(MARK_BOX[3] - (-BADGE.lift + BADGE.bottom) >= 12, "the house's floor stands clear of a vehicle parked on the point (its symbol about 12 px each way)")
assert.ok(label.full[3] - label.full[1] > label.short[3] - label.short[1], 'in full: the name and its line; short: the name')

console.log('fleetMap: ok')
