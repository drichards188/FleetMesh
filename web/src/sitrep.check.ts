// Checks for sitrep.ts: node web/src/sitrep.check.ts (prints "sitrep: ok").
import assert from 'node:assert/strict'
import { ANY, filterNodes, linkHealth, nodeRows, tally, windowOf, worstFirst, type SitrepContext } from './sitrep.ts'
import type { Vehicle } from './types.ts'

const v = (uid: number, o: { lost?: boolean; mode?: string; speed?: number; battery?: number | null; domain?: string; short?: string } = {}): Vehicle =>
  ({
    uid, sysid: uid, kind: 'copter', mode: o.mode ?? 'GUIDED', groundspeed: o.speed ?? 0, battery_pct: o.battery === undefined ? 80 : o.battery,
    link: { stale: o.lost ?? false, since_last_ms: o.lost ? 90_000 : 200, expected_interval_ms: 1000 },
    identity: { callsign: `V${uid}`, platform: 'quad', short: o.short ?? 'QUAD', domain: o.domain ?? 'air' },
  }) as unknown as Vehicle
const fleet = [v(1), v(2, { lost: true }), v(3, { mode: 'RTL' }), v(4, { battery: 10 }), v(5, { speed: 5, domain: 'surface', short: 'Boat' }), v(6)]
const by = new Map(fleet.map((x) => [x.uid, x]))
const ctx: SitrepContext = { latest: new Map([[1, { state: 'active', task: 'orbit' }]]) }

// strength by state
const t = tally([1, 2, 3, 4, 6, 99], by, ctx)
assert.equal(t.here, 5, 'a member not in the fleet is not counted')
assert.deepEqual([t.counts.station, t.counts.lost, t.counts.returning, t.counts.idle], [1, 1, 1, 2])
assert.equal(t.lowBattery, 1)

// nodes: filters and the worst first
const nodes = nodeRows(fleet, ctx, new Map([[1, ['Mine']]]))
assert.equal(nodes.length, 6)
assert.equal(nodes.find((n) => n.uid === 4)!.lowBattery, true)
assert.equal(nodes.find((n) => n.uid === 1)!.order, 'orbit active')
assert.deepEqual(filterNodes(nodes, { ...ANY, unheard: true }).map((n) => n.uid), [2])
assert.deepEqual(filterNodes(nodes, { ...ANY, low: true }).map((n) => n.uid), [4])
assert.deepEqual(filterNodes(nodes, { ...ANY, text: 'v3' }).map((n) => n.uid), [3])
assert.deepEqual(filterNodes(nodes, { ...ANY, within: new Set([1, 2]) }).map((n) => n.uid), [1, 2])
assert.deepEqual(worstFirst(nodes).slice(0, 3).map((n) => n.uid), [2, 4, 1], 'lost, low battery')
assert.equal(linkHealth(v(1, { lost: true })), 'bad')

// the window: only what is in view, with a margin, never past the ends
assert.deepEqual(windowOf(1000, 0, 300, 30), { from: 0, to: 16 })
assert.deepEqual(windowOf(1000, 3000, 300, 30), { from: 94, to: 116 })
assert.deepEqual(windowOf(10, 0, 300, 30), { from: 0, to: 10 })
assert.deepEqual(windowOf(0, 0, 300, 30), { from: 0, to: 0 })

console.log('sitrep: ok')
