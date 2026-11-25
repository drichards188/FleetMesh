// Checks for traffic.ts (no test runner in web/): node web/src/traffic.check.ts, as make check does.
// The claims: traffic near our aircraft (horizontally and vertically) or our boats is near, an
// airliner at altitude is far whoever is under it, a flagged or named track is near wherever it
// is, the near ones drawn in full are capped (the nearest kept, the flagged never demoted), a
// domain switched off is hidden and counted; a coasting, stale or tentative track fades but never
// vanishes; a bearing-only track is a wedge along its line (its range interval, two sigma either
// side, rising with its elevation), never a point; the prediction is constant velocity with the
// tracker's own growth of uncertainty; a disagreement pairs a broadcast with what the sensors see;
// truth is matched by identity, then by position, and what is left is missed; a silent feed sorts
// first. Throws on the first failure, prints "traffic: ok" otherwise.
import assert from 'node:assert/strict'
import {
  bearingWedge,
  declutter,
  destination,
  disagreementPairs,
  distanceToPath,
  drawAltM,
  feedRows,
  FLATTEN_ALT_M,
  heightAlong,
  leaderEnd,
  localXY,
  matchTruth,
  MAX_NEAR,
  MIN_ALPHA,
  predictPath,
  relevance,
  trackAlpha,
  type FleetContext,
  type Track,
  type TruthObject,
} from './traffic.ts'
import type { LngLat } from './types.ts'

const HOME: LngLat = [-122.4150, 37.8170]
const dist = (a: LngLat, b: LngLat) => Math.hypot(...localXY(a, b))
const near = (a: number, b: number, tol: number, what: string) => assert.ok(Math.abs(a - b) <= tol, `${what}: ${a} is not within ${tol} of ${b}`)

function track(over: Partial<Track> & { at?: LngLat }): Track {
  const at = over.at ?? HOME
  return {
    id: 'trk-1',
    t: 1_000_000,
    domain: 'air',
    class: 'helicopter',
    cooperative: true,
    ids: { icao24: 'a1b2c3', callsign: 'N123', anonymous: false },
    lat: at[1],
    lon: at[0],
    alt_m: 300,
    alt_ref: 'baro',
    h_sigma_m: 15,
    v_sigma_m: 10,
    vel_mps: [10, 0],
    vs_mps: 0,
    heading_deg: 90,
    state: 'confirmed',
    sources: [{ kind: 'adsb_1090', rx: 'hub-pier', last_t: 999_500 }],
    flags: [],
    static: {},
    ...over,
  }
}

// ---- geometry ----
const east = destination(HOME, 90, 1000)
near(dist(HOME, east), 1000, 2, 'a kilometre east is a kilometre away')
near(localXY(HOME, east)[1], 0, 1, 'and due east')
near(distanceToPath(destination(HOME, 0, 500), [destination(HOME, 270, 2000), destination(HOME, 90, 2000)]), 500, 2, 'the distance to a route is to its nearest segment')
assert.equal(distanceToPath(HOME, []), Infinity, 'no route, no distance')

// ---- relevance: near, mid, far ----
const drone: FleetContext = { ours: [{ uid: 7, at: HOME, altM: 90, domain: 'air' }], routes: [] }
const boat: FleetContext = { ours: [{ uid: 9, at: HOME, altM: null, domain: 'surface' }], routes: [] }
assert.equal(relevance(track({ at: destination(HOME, 45, 1500), alt_m: 350 }), drone).tier, 'near', 'a helicopter 1.5 km off and 260 m above a drone is near')
assert.equal(relevance(track({ at: destination(HOME, 45, 1500), alt_m: 350 }), drone).nearest, 7, 'and names the drone')
assert.equal(relevance(track({ at: destination(HOME, 45, 1500), alt_m: 1200 }), drone).tier, 'mid', 'the same 1,100 m above it is mid')
assert.equal(relevance(track({ at: destination(HOME, 45, 20_000), alt_m: 300 }), drone).tier, 'far', 'twenty km off is far')
assert.equal(relevance(track({ class: 'airliner', alt_m: FLATTEN_ALT_M + 2000 }), drone).tier, 'far', 'an airliner at altitude is far even overhead')
assert.equal(relevance(track({ alt_m: null, alt_ref: 'none', at: destination(HOME, 0, 2000) }), drone).tier, 'near', 'an unknown altitude counts as close vertically')
assert.equal(relevance(track({}), boat).tier, 'far', 'aircraft matter to our aircraft, not to our boats')
const routeOnly: FleetContext = { ours: [], routes: [{ uid: 3, path: [destination(HOME, 270, 5000), destination(HOME, 90, 5000)], domain: 'air' }] }
assert.equal(relevance(track({ at: destination(HOME, 0, 1000), alt_m: 150 }), routeOnly).tier, 'near', 'near a route of ours is near')
const ferry = track({ domain: 'surface', class: 'ferry', alt_m: null, alt_ref: 'none', at: destination(HOME, 90, 1200) })
assert.equal(relevance(ferry, boat).tier, 'near', 'a ferry 1.2 km from our boat is near')
assert.equal(relevance({ ...ferry, ...{ lat: destination(HOME, 90, 3000)[1], lon: destination(HOME, 90, 3000)[0] } }, boat).tier, 'mid', 'at 3 km it is mid')
assert.equal(relevance({ ...ferry, lat: destination(HOME, 90, 500)[1], lon: destination(HOME, 90, 500)[0] }, drone).tier, 'mid', 'a big vessel under one of our aircraft is mid')
assert.equal(relevance(ferry, drone).tier, 'far', 'not 1.2 km off it')
assert.equal(relevance(track({ domain: 'surface', class: 'small_craft', alt_m: null, at: destination(HOME, 90, 300) }), drone).tier, 'far', 'a kayak under a drone is not')
assert.equal(relevance(track({ at: destination(HOME, 0, 40_000) }), drone, true).tier, 'near', 'a forced track is near wherever it is')
assert.ok(relevance(track({ at: destination(HOME, 0, 40_000) }), drone, true).why.startsWith('selected or named by an alert; otherwise far'), 'and says why, and what it would be otherwise')
assert.ok(relevance(track({ at: destination(HOME, 0, 40_000), flags: ['emergency'] }), drone, true).why.startsWith('flagged'), 'a flagged one says so')

// ---- declutter ----
const many: Track[] = Array.from({ length: MAX_NEAR + 20 }, (_, k) => track({ id: `n${k}`, at: destination(HOME, (k * 37) % 360, 100 + k * 20), alt_m: 150 }))
const flagged = track({ id: 'bad', at: destination(HOME, 10, 2900), alt_m: 150, flags: ['sensor_disagree'] })
const out = declutter([...many, flagged, track({ id: 'ship', domain: 'surface', class: 'cargo', alt_m: null, at: destination(HOME, 0, 900) })], drone, { air: true, surface: false })
const tierOf = (id: string) => out.shown.find((r) => r.track.id === id)?.rel.tier
assert.equal(out.counts.near, MAX_NEAR, 'at most MAX_NEAR drawn in full')
assert.equal(tierOf('n0'), 'near', 'the nearest kept')
assert.equal(tierOf(`n${MAX_NEAR + 19}`), 'mid', 'the furthest of the near ones demoted')
assert.equal(tierOf('bad'), 'near', 'the flagged one never demoted')
assert.ok(out.shown.find((r) => r.track.id === 'bad')!.alarm, 'and drawn as an alarm')
assert.equal(tierOf('ship'), undefined, 'surface off: hidden')
assert.equal(out.counts.hidden, 1, 'and counted as hidden')
assert.equal(out.counts.air, MAX_NEAR + 21, 'every aircraft counted')
const forced = declutter([track({ id: 'x', at: destination(HOME, 0, 30_000) })], drone, { air: true, surface: true, force: new Set(['x']) })
assert.equal(forced.shown[0].rel.tier, 'near', 'a track an alert names is near')

// ---- fade ----
assert.equal(trackAlpha({ state: 'confirmed', flags: [] }, 'near'), 1, 'a fresh near track at full strength')
assert.ok(trackAlpha({ state: 'coasting', flags: [] }, 'near') < trackAlpha({ state: 'confirmed', flags: ['stale'] }, 'near'), 'coasting fades more than stale')
assert.ok(trackAlpha({ state: 'confirmed', flags: ['stale'] }, 'near') < 1, 'stale fades')
assert.ok(trackAlpha({ state: 'tentative', flags: [] }, 'near') < 1, 'tentative fades a little')
assert.ok(trackAlpha({ state: 'confirmed', flags: [] }, 'far') < trackAlpha({ state: 'confirmed', flags: [] }, 'mid'), 'far is fainter than mid')
assert.equal(trackAlpha({ state: 'coasting', flags: ['stale'] }, 'far'), MIN_ALPHA, 'but never gone')
assert.ok(out.shown.find((r) => r.track.id === 'n0')!.provisional === false && declutter([track({ state: 'tentative' })], drone, { air: true, surface: true }).shown[0].provisional, 'a tentative track is drawn provisional')

// ---- height drawn ----
assert.equal(drawAltM('air', 300), 300, 'an aircraft at its height')
assert.equal(drawAltM('air', FLATTEN_ALT_M + 1), 0, 'one above FLATTEN_ALT_M at its foot')
assert.equal(drawAltM('air', null), 0, 'no altitude: at the surface')
assert.equal(drawAltM('surface', 12), 0, 'a vessel on the surface')

// ---- bearing-only wedge ----
const line = { sensor_lat: HOME[1], sensor_lon: HOME[0], sensor_alt_m: 100, bearing_deg: 90, sigma_bearing_deg: 2, elev_deg: 5, range_m: [20, 2500] as [number, number], t: 0 }
const w = bearingWedge(line, 10)
near(dist(HOME, [w.centre[0][0], w.centre[0][1]]), 20, 1, 'the line starts at the near end of the interval')
near(dist(HOME, [w.centre[1][0], w.centre[1][1]]), 2500, 3, 'and ends at the far end')
near(localXY(HOME, [w.centre[1][0], w.centre[1][1]])[1], 0, 2, 'along its bearing')
near(w.centre[1][2], 100 + 2500 * Math.tan((5 * Math.PI) / 180), 0.5, 'rising with its elevation')
assert.equal(w.halfDeg, 4, 'two sigma either side')
const farEdge = w.polygon[11] // the far arc's first point (the arcs are 11 points each)
near(dist(HOME, [farEdge[0], farEdge[1]]), 2500, 3, 'the far arc at the far range')
near(Math.abs(localXY(HOME, [farEdge[0], farEdge[1]])[1]), 2500 * Math.sin((4 * Math.PI) / 180), 5, 'spread by the half-angle')
assert.deepEqual(w.polygon[0], w.polygon[w.polygon.length - 1], 'a closed outline')
assert.equal(heightAlong({ sensor_alt_m: 50, elev_deg: null }, 1000), 50, 'no elevation: the sensor height')
assert.equal(heightAlong({ sensor_alt_m: 50, elev_deg: -30 }, 1000), 0, 'never under the sea')
assert.equal(bearingWedge({ ...line, sigma_bearing_deg: 0 }).halfDeg, 0.5, 'never a bare line')
const bo = track({ vel_mps: null, heading_deg: null, flags: ['bearing_only'], bearing_only: line, cooperative: false })
assert.deepEqual(predictPath(bo), [], 'a bearing-only track has no prediction')
assert.equal(leaderEnd(bo), null, 'nor a speed leader')

// ---- prediction ----
const p = predictPath(track({ vel_mps: [0, 100], alt_m: 1000, vs_mps: -5 }), 120, 60)
assert.equal(p.length, 3, '0, 60 and 120 s')
near(dist(HOME, p[2].at), 12_000, 15, 'constant velocity: 12 km in two minutes at 100 m/s')
assert.equal(p[2].altM, 400, 'and the descent')
near(p[2].sigmaM, 1517, 5, "the tracker's own growth: about 1.5 km after two minutes for ADS-B")
near(predictPath(track({ domain: 'surface', vel_mps: [5, 0], alt_m: null, h_sigma_m: 10 }), 120, 120)[1].sigmaM, 170, 2, "and about 170 m for a ship")
near(dist(HOME, leaderEnd(track({ vel_mps: [10, 0] }))!), 300, 2, 'an aircraft leader is 30 s ahead')
assert.equal(leaderEnd(track({ vel_mps: [0.1, 0] })), null, 'nothing ahead of a still one')

// ---- disagreements ----
const broadcast = track({ id: 'b', flags: ['sensor_disagree'] })
const sensed = track({ id: 's', cooperative: false, ids: { anonymous: false }, flags: ['sensor_disagree'], at: destination(HOME, 0, 700) })
const decoy = track({ id: 'd', cooperative: false, ids: { anonymous: false }, flags: ['sensor_disagree'], at: destination(HOME, 0, 4000) })
const pairs = disagreementPairs([broadcast, sensed, decoy])
assert.equal(pairs.length, 1, 'one broadcast, one pair')
assert.equal(pairs[0].sensed.id, 's', 'the nearest sensed track')
near(pairs[0].distM, 700, 2, 'with the offset')

// ---- truth ----
const truth: TruthObject[] = [
  { id: 'heli', domain: 'air', class: 'helicopter', lat: HOME[1], lon: HOME[0], alt_m: 300, ids: { icao24: 'A1B2C3' } },
  { id: 'kayak', domain: 'surface', class: 'small_craft', lat: destination(HOME, 0, 120)[1], lon: destination(HOME, 0, 120)[0], alt_m: 0 },
  { id: 'dark', domain: 'surface', class: 'fishing', lat: destination(HOME, 180, 3000)[1], lon: destination(HOME, 180, 3000)[0], alt_m: 0 },
]
const radar = track({ id: 'r', domain: 'surface', class: 'small_craft', cooperative: false, ids: { anonymous: false }, at: destination(HOME, 0, 160), h_sigma_m: 40 })
const m = matchTruth(truth, [track({ id: 'h' }), radar])
assert.equal(m.find((x) => x.truth.id === 'heli')!.track?.id, 'h', 'matched by its ICAO address, whatever the case')
assert.equal(m.find((x) => x.truth.id === 'kayak')!.track?.id, 'r', 'a radar track matched by position')
assert.equal(m.find((x) => x.truth.id === 'dark')!.track, null, 'what nothing stands for is missed')

// ---- feeds ----
const rows = feedRows({ state: 'degraded', reasons: [], feeds: { 'hub-pier ais': { connected: true, last_t: 0, lines_per_s: 0 }, 'hub-pier sbs': { connected: true, last_t: 99_000, lines_per_s: 50 }, json: { connected: false, last_t: null, lines_per_s: 0 }, 'net-ais': { connected: true, last_t: 80_000, lines_per_s: 1 } } }, 100_000)
assert.deepEqual(rows.map((r) => [r.name, r.tone]), [['hub-pier ais', 'bad'], ['json', 'bad'], ['net-ais', 'warn'], ['hub-pier sbs', 'ok']], 'silent and disconnected first, quiet next')
assert.deepEqual(feedRows(null, 0), [], 'no picture, no rows')

console.log('traffic: ok')
