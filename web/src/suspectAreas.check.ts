// Checks for suspectAreas.ts and deliveryRoutes.ts: node web/src/suspectAreas.check.ts (prints
// "suspectAreas: ok"). The claims: an area is drawn only from what vehicles report (never an
// event's own place); vehicles near each other make one area, far apart two; an area covers its
// vehicles; a fault across most of a fleet is not a place; a delivery's route runs from the vehicle
// through its waypoints, only while its op is in play.
import assert from 'node:assert/strict'
import { suspectAreas, MIN_RADIUS_M, type AreaVehicle } from './suspectAreas.ts'
import { deliveryRoutes } from './deliveryRoutes.ts'

const v = (uid: number, lon: number, lat: number, emu?: string): AreaVehicle => ({ uid, position: [lon, lat], link: { emu: emu ? { state: emu, cause: null } : null } })
const nav = (o: Record<string, string>) => ({ vehicles: Object.fromEntries(Object.entries(o).map(([k, s]) => [k, { state: s as 'trusted' }])) })

const fleet = [v(1, -122.41, 37.8), v(2, -122.411, 37.8005), v(3, -122.5, 37.7), v(4, -122.4, 37.9), v(5, -122.3, 37.85), v(6, -122.2, 37.6), v(7, -122.1, 37.5)]
assert.deepEqual(suspectAreas(fleet, nav({ 1: 'trusted', 2: 'trusted' })), [], 'nothing reported: nothing drawn')
const a = suspectAreas(fleet, nav({ 1: 'degraded', 2: 'untrusted' }))
assert.equal(a.length, 1, 'two vehicles near each other make one area')
assert.equal(a[0].kind, 'gnss')
assert.deepEqual(a[0].uids, [1, 2])
assert.ok(a[0].radius_m >= MIN_RADIUS_M)
assert.match(a[0].label, /Suspected GNSS interference: 2 vehicles/)
// it covers its vehicles
const [lon, lat] = a[0].at
for (const u of a[0].uids) {
  const p = fleet.find((x) => x.uid === u)!.position!
  const d = Math.hypot((p[0] - lon) * 111_320 * Math.cos((lat * Math.PI) / 180), (p[1] - lat) * 111_320)
  assert.ok(d <= a[0].radius_m, `vehicle ${u} is inside its area`)
}
assert.equal(suspectAreas(fleet, nav({ 1: 'degraded', 3: 'degraded' })).length, 2, 'far apart: two areas')
assert.equal(suspectAreas(fleet, nav({ 1: 'unknown' })).length, 0, 'unknown is not interference (a vehicle yet to get a fix)')
// most of a fleet at once is not a place
assert.deepEqual(suspectAreas(fleet, nav({ 1: 'untrusted', 2: 'untrusted', 3: 'untrusted', 4: 'untrusted', 5: 'untrusted', 6: 'untrusted' })), [])
// link outages and degradation, by the emulator's state
const l = suspectAreas([v(1, -122.41, 37.8, 'down'), v(2, -122.4105, 37.8, 'down'), v(3, -122.5, 37.7, 'degraded'), v(4, -122.4, 37.9, 'ok'), v(5, 0, 0, 'ok'), v(6, 1, 1, 'ok'), v(7, 2, 2)], nav({}))
assert.deepEqual(l.map((x) => `${x.kind}:${x.uids.join()}`).sort(), ['link_degraded:3', 'link_down:1,2'])
assert.deepEqual(suspectAreas([{ uid: 1, position: null, link: { emu: { state: 'down', cause: null } } }], nav({})), [], 'no position: no area')

// routes
const taskings: Parameters<typeof deliveryRoutes>[0] = [
  { id: 1, task: 'transit', params: { action: 'deliver', parcel: 'p-1', route: [[2, 2], [3, 3]], via: { '5': [[1.5, 1.5]] } }, ops: [{ uid: 5, state: 'active' }] },
  { id: 2, task: 'transit', params: { action: 'pickup', route: [[9, 9]] }, ops: [{ uid: 6, state: 'done' }] },
  { id: 3, task: 'orbit', params: { route: [[9, 9], [8, 8]] }, ops: [{ uid: 7, state: 'active' }] },
  { id: 4, task: 'transit', params: { route: [[9, 9]] }, ops: [{ uid: 8, state: 'active' }] },
]
const routes = deliveryRoutes(taskings, [{ uid: 5, position: [1, 1] }, { uid: 6, position: [0, 0] }])
assert.equal(routes.length, 1, 'only a delivery order with an op in play')
assert.deepEqual(routes[0].path, [[1, 1], [1.5, 1.5], [2, 2], [3, 3]], 'from the vehicle, through the planner’s waypoints, to the end')
assert.equal(routes[0].action, 'deliver')
assert.equal(routes[0].parcel, 'p-1')
assert.deepEqual(deliveryRoutes(taskings, []), [{ key: '1:5', uid: 5, action: 'deliver', path: [[1.5, 1.5], [2, 2], [3, 3]], parcel: 'p-1' }], 'a vehicle with no position: the route as flown')

console.log('suspectAreas: ok')
