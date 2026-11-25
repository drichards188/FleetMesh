// Checks for inboxOrder.ts: what goes in "Needs you", what in "For information", and in what order.
import assert from 'node:assert/strict'
import { alertRank, alertSort, inboxOrder, RANK } from './inboxOrder.ts'

// the order of the Needs you list (ledger 2): ours in trouble, review items, approvals by expiry,
// then high and medium alerts
const items = [
  { id: 'alert-med', rank: alertRank('left_task', 'medium'), sort: alertSort('medium', 5) },
  { id: 'approval-late', rank: RANK.approval, sort: 300 },
  { id: 'loss', rank: RANK.review, sort: -20 },
  { id: 'approval-soon', rank: RANK.approval, sort: 40 },
  { id: 'ours', rank: alertRank('lost_link', 'high'), sort: alertSort('high', 9) },
  { id: 'alert-high', rank: alertRank('task_failed', 'high'), sort: alertSort('high', 8) },
]
const { needs, info } = inboxOrder(items)
assert.deepEqual(
  needs.map((x) => x.id),
  ['ours', 'loss', 'approval-soon', 'approval-late', 'alert-high', 'alert-med'],
)
assert.equal(info.length, 0)

// for information, in its own order: running, info alerts, settled, acknowledged, confirmed losses
const quiet = inboxOrder([
  { id: 'lost', rank: RANK.lost, sort: null },
  { id: 'ack', rank: RANK.acknowledged, sort: null },
  { id: 'info-alert', rank: alertRank('node_lost', 'info'), sort: alertSort('info', 3) },
  { id: 'settled', rank: RANK.settled, sort: null },
  { id: 'running', rank: RANK.running, sort: null },
])
assert.deepEqual(quiet.needs, [])
assert.deepEqual(
  quiet.info.map((x) => x.id),
  ['running', 'info-alert', 'settled', 'ack', 'lost'],
)

// same rank and no time: the order they came in
assert.deepEqual(
  inboxOrder([
    { id: 'a', rank: RANK.review, sort: null },
    { id: 'b', rank: RANK.review, sort: null },
  ]).needs.map((x) => x.id),
  ['a', 'b'],
)

console.log('inboxOrder: ok')
