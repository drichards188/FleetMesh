// Checks for nodeLoss.ts (no test runner in web/): node web/src/nodeLoss.check.ts, as make check
// does. Throws on the first failure, prints "nodeLoss: ok" otherwise. Not part of the bundle.
import { confirmEffects, nodeLossView, paddedRing, regionLines, regionRings, type ReviewNodeLoss, type ReviewNodeLost, staleBadge, standingOf } from './nodeLoss.ts'

function eq(a: unknown, b: unknown, what: string) {
  const x = JSON.stringify(a)
  const y = JSON.stringify(b)
  if (x !== y) throw new Error(`${what}: got ${x}, want ${y}`)
}

function loss(uid: number, state: ReviewNodeLoss['state']): ReviewNodeLoss {
  return {
    kind: 'node_loss',
    id: `N-${uid}`,
    uid,
    callsign: `DRONE ${uid}`,
    platform: 'delivery_boat',
    state,
    since: 1_000_000,
    since_t: 1000,
    silent_s: 42,
    last_position: [-122.485, 37.813],
    presumed_t: state === 'presumed_lost' ? 1031 : null,
    evidence: [{ kind: 'operator', seq: 12, t: 1001, detail: 'flagged by an operator: no report since the crew called' }],
    needs_note: state !== 'presumed_lost',
    would_void: [],
    would_end: [],
    would_close: null,
    refused: null,
    region: null,
    actions: ['confirm'],
  }
}

const lost: ReviewNodeLost = {
  kind: 'node_lost',
  id: 'L-3',
  uid: 3,
  callsign: 'DRONE 3',
  platform: 'delivery_boat',
  since: 1_000_000,
  since_t: 1000,
  silent_s: 90,
  last_position: null,
  by: 'op-2',
  t: 1060,
  note: null,
  closed: {},
  holds: true,
  region: null,
  actions: ['revoke'],
}

// a run before rules 30: no node lists, nothing changes (the stale badge reads as it always did)
const before = nodeLossView({})
eq(before.on, false, 'no node lists: off')
eq(standingOf(before, 1, true), null, 'off: no standing')
eq(staleBadge(standingOf(before, 1, true), 12.7), 'LOST 12s', 'off: the link-lost badge as before')
eq(nodeLossView(undefined).on, false, 'no review: off')

// rules 30: empty lists still say the run decides under them
const empty = nodeLossView({ node_losses: [], nodes_lost: [] })
eq(empty.on, true, 'empty lists: on')
eq(standingOf(empty, 1, false), null, 'reporting: no standing')
eq(staleBadge(standingOf(empty, 1, true), 12.7), 'SILENT 12s', 'silent')

// presumed (the engine's), flagged by hand (silent still), confirmed
const v = nodeLossView({ node_losses: [loss(1, 'presumed_lost'), loss(2, 'silent')], nodes_lost: [lost] })
eq(standingOf(v, 1, true), 'presumed_lost', 'presumed')
eq(staleBadge('presumed_lost', 40), 'PRESUMED LOST 40s', 'presumed badge')
eq(standingOf(v, 2, true), 'silent', 'a hand flag presumes nothing')
eq(standingOf(v, 3, true), 'lost', 'confirmed')
eq(standingOf(v, 3, false), 'lost', 'confirmed, whatever the link shows until the bridge revokes it')
eq(staleBadge('lost', 400), 'LOST', 'a confirmed loss has no clock')

// what confirming does: always the keep-out, no order and undone if heard; the rest when it ends something
const plain = confirmEffects(loss(1, 'presumed_lost'))
eq(plain.length, 3, 'nothing to end: three lines')
eq(plain[0].startsWith('It leaves every keep-out'), true, 'keep-outs first')
eq(plain[2].startsWith('Undone at once if it reports again'), true, 'undone last')
const ends = confirmEffects({ would_void: [7], would_end: [4, 5], would_close: { FS_GCS_ENABLE: 1 } })
eq(ends[1], 'Its waiting order is voided: i7.', 'one voided')
eq(ends[2], 'Its running orders end on it: i4, i5.', 'two ended')
eq(ends[4], 'The lost-link values owed back to it (FS_GCS_ENABLE) close: their restore stops.', 'owed values')

// the engine names our sensors by uid ("1:eo_ir"): a person reads callsigns

// the region (stage 2's, as the keep-out counts it): its parts' padded outlines and its reach
const M = 111_320
const lat0 = 37.8
const kx = M * Math.cos((lat0 * Math.PI) / 180)
const at = (east: number, north: number): [number, number] => [-122.48 + east / kx, lat0 + north / M]
const metres = (p: [number, number]): [number, number] => [(p[0] + 122.48) * kx, (p[1] - lat0) * M]
const near = (x: number, y: number, what: string) => {
  if (Math.abs(x - y) > 0.5) throw new Error(`${what}: got ${x}, want ${y}`)
}
// a disc: every point its radius from the centre
const disc = paddedRing([at(0, 0)], 150).map(metres)
for (const [x, y] of disc) near(Math.hypot(x, y), 150, 'a disc')
eq(disc[0], disc[disc.length - 1], 'a closed ring')
// a corridor 1 km east, 50 m either side: its capsule
const cap = paddedRing([at(0, 0), at(1000, 0)], 50).map(metres)
near(Math.max(...cap.map((p) => p[1])), 50, 'the corridor north')
near(Math.min(...cap.map((p) => p[1])), -50, 'the corridor south')
near(Math.max(...cap.map((p) => p[0])), 1050, 'the corridor east')
near(Math.min(...cap.map((p) => p[0])), -50, 'the corridor west')
for (const [x, y] of cap) {
  const d = x < 0 ? Math.hypot(x, y) : x > 1000 ? Math.hypot(x - 1000, y) : Math.abs(y)
  near(d, 50, 'every point its pad from the leg')
}
// a hull (a way home: a triangle, a point inside it left out), 20 m around it
const tri = paddedRing([at(0, 0), at(400, 0), at(200, 50), at(0, 300)], 20).map(metres)
near(Math.min(...tri.map((p) => p[1])), -20, 'the hull south')
near(Math.min(...tri.map((p) => p[0])), -20, 'the hull west')
near(Math.max(...tri.map((p) => p[1])), 320, 'the hull north')
eq(paddedRing([], 10), [], 'no point: nothing')
eq(paddedRing([at(0, 0), at(100, 0)], 0).length, 3, 'no pad: the outline itself, closed')
// a region's parts and its reach
eq(regionRings(null), { parts: [], reach: null }, 'no region: nothing drawn')
const r = regionRings({
  p0: at(0, 0),
  t0: 0,
  at: 60_000,
  span_s: 60,
  reach_mps: 25,
  reach: { points: [at(0, 0)], pad_m: 1500 },
  parts: [
    { hypothesis: 'order', kind: 'route', shape: { points: [at(0, 0), at(0, 900)], pad_m: 50 }, what: 'on its patrol leg' },
    { hypothesis: 'top_speed', kind: 'top_speed', shape: { points: [at(0, 0)], pad_m: 150 }, what: 'anywhere its top speed allows' },
  ],
})
eq(r.parts.map((p) => [p.hypothesis, p.what]), [['order', 'on its patrol leg'], ['top_speed', 'anywhere its top speed allows']], 'its parts, in order')
near(Math.max(...(r.reach ?? []).map((p) => metres(p)[1])), 1500, 'the reach')
// the region in words: one line per distinct part, in the engine's words and order, the number
// of shapes where one description covers several (DRONE 2's of 2026-11-13: its failsafe's way
// home widened from each of its patrol's five shapes)
const home = 'if it lost us too, its rtl failsafe: from anywhere on its way, straight home'
const leg = 'on its patrol (intent i2): on its way to a waypoint, within 225 m'
const part = (hypothesis: 'order' | 'failsafe' | 'top_speed', what: string) => ({ hypothesis, kind: 'route', shape: { points: [at(0, 0)], pad_m: 10 }, what })
eq(
  regionLines([
    part('top_speed', 'anywhere its top speed allows since its last report (presumed lost)'),
    ...[1, 2, 3, 4, 5].map(() => part('failsafe', home)),
    part('failsafe', 'if it lost us too, its rtl failsafe: at home'),
    part('order', leg),
    part('order', leg),
    part('order', 'on its patrol (intent i2): along its route, within 225 m'),
  ]),
  [
    'anywhere its top speed allows since its last report (presumed lost)',
    `${home} (5 shapes)`,
    'if it lost us too, its rtl failsafe: at home',
    `${leg} (2 shapes)`,
    'on its patrol (intent i2): along its route, within 225 m',
  ],
  'one line per distinct part',
)
eq(regionLines([]), [], 'no part: no line')

console.log('nodeLoss: ok')
