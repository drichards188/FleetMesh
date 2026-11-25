// Checks for needsYou.ts (no test runner in web/): node web/src/needsYou.check.ts, as make check
// does. Throws on the first failure, prints "needsYou: ok" otherwise. Not part of the bundle.
import assert from 'node:assert/strict'
import { needsYou } from './needsYou.ts'
import type { Alert } from './decisions'

const alert = (id: string, severity: Alert['severity'], extra: Partial<Alert> = {}) => ({ id, kind: 'lost_link', severity, uid: 1, callsign: 'DRONE 1', detail: '', since_t: 0, ...extra }) as unknown as Alert

// one presumed loss to confirm, a lost link (high), one alert for information
const inbox = {
  pending: [],
  alerts: [alert('link:1', 'high'), alert('node_lost:2', 'info')],
  review: { node_losses: [{}] },
}
const n = needsYou(inbox, new Set())
assert.equal(n.total, 2, 'the loss once and the high alert')
assert.deepEqual([n.review, n.high, n.info], [1, 1, 1])
assert.equal(n.says, '2 need you: 1 item for review, 1 high alert. Not counted: 1 alert for information.')
// an approval waiting
const m = needsYou({ ...inbox, pending: [{}] }, new Set())
assert.deepEqual([m.total, m.approvals], [3, 1])
// acknowledged in this browser: not counted, said (there is no snooze)
const k = needsYou(inbox, new Set(['link:1']))
assert.deepEqual([k.total, k.high, k.muted], [1, 0, 1])
// a swarm's member alerts count once, as the swarm's
const folded = { pending: [], alerts: [alert('link:1', 'high', { folded_into: 'sw:ROUTE' }), alert('link:2', 'high', { folded_into: 'sw:ROUTE' }), alert('sw:ROUTE', 'high')] }
assert.equal(needsYou(folded, new Set()).total, 1)
assert.deepEqual(needsYou(null, new Set()), { total: 0, approvals: 0, review: 0, high: 0, medium: 0, info: 0, muted: 0, says: 'Nothing needs you.' })

console.log('needsYou: ok')
