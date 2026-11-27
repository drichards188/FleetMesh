// Checks for officerDesk.ts: node web/src/officerDesk.check.ts (prints "officerDesk: ok"). The
// claims: what the officer is deciding comes soonest deadline first with the time it has left;
// what it finished comes newest first, each with its verb and its note's first sentence; the
// counts are the last ten minutes'; its decisions not landing (its latest items all reaching the
// person at their deadlines) is said; off watch, everything is the person's.
import assert from 'node:assert/strict'
import { firstSentence, itemWords, officerDesk, type TriageRecord, type TriageState } from './officerDesk.ts'

const name = (u: number | string) => `DRONE ${u}`
const T = 10_000 // the bridge's clock now, s
const rec = (item: string, stage: TriageRecord['stage'], o: Partial<TriageRecord> = {}): TriageRecord => ({
  item,
  class: item.startsWith('intent:') ? 'held' : 'alert',
  kind: item.startsWith('intent:') ? 'pickup' : 'delivery_failed',
  about: item.startsWith('intent:') ? { intent: 3, uids: ['14'] } : { uid: 10 }, // the bridge sends a held order's uids as text
  entered_t: T - 30,
  stage,
  ...o,
})
const state = (open: TriageRecord[], history: TriageRecord[], on_watch = true): TriageState => ({ officer: 'agent-watch', counts: { on_watch, with_officer: open.length, handled: 0 }, open, history })

assert.equal(itemWords(rec('intent:3', 'with_officer'), name), "DRONE 14's pickup")
assert.equal(itemWords(rec('alert:a', 'with_officer'), name), 'delivery failed, DRONE 10')
assert.equal(firstSentence('Pickup held for marginal gusts. The dispatcher retries.'), 'Pickup held for marginal gusts.')
assert.ok(firstSentence('x'.repeat(300)).length <= 120)

const busy = officerDesk(
  state(
    [rec('intent:3', 'with_officer', { deadline_t: T + 50 }), rec('alert:a', 'with_officer', { entered_t: T - 80, deadline_t: T + 10 })],
    [
      rec('intent:1', 'turned_down', { at_t: T - 20, note: 'Held only for marginal gusts. It goes again in 2 min.' }),
      rec('alert:b', 'handled', { at_t: T - 100, note: 'Automation kept the parcel.' }),
      rec('intent:2', 'escalated', { at_t: T - 200, note: 'A gust at the zone.' }),
      rec('alert:c', 'handled', { at_t: T - 900, note: 'Long ago.' }),
    ],
  ),
  T * 1000,
  name,
)
assert.deepEqual(busy.working.map((w) => [w.what, w.leftS]), [['delivery failed, DRONE 10', 10], ["DRONE 14's pickup", 50]], 'soonest deadline first')
assert.ok(Math.abs(busy.working[0].left - 10 / 90) < 1e-9, 'the part of its time left')
assert.deepEqual(busy.recent.map((d) => d.verb), ['Turned down', 'Settled', 'Passed to you', 'Settled'], 'newest first')
assert.equal(busy.recent[0].why, 'Held only for marginal gusts.')
assert.deepEqual([busy.settled, busy.passed, busy.untriaged], [2, 1, 0], 'the last ten minutes only')
assert.equal(busy.stalled, false)
assert.equal(busy.headline, 'Deciding 2 items now; in the last 10 min it settled 2 and passed 1 to you.')

// its decisions not landing: its latest three all reached the person at their deadlines
const stuck = officerDesk(
  state([], [rec('intent:5', 'untriaged', { at_t: T - 5, why: 'deadline' }), rec('intent:4', 'untriaged', { at_t: T - 15, why: 'deadline' }), rec('alert:d', 'untriaged', { at_t: T - 25, why: 'deadline' }), rec('intent:1', 'handled', { at_t: T - 400 })]),
  T * 1000,
  name,
)
assert.equal(stuck.stalled, true)
assert.equal(stuck.recent[0].why, 'its deadline passed; it is yours')
assert.equal(officerDesk(state([], [rec('intent:5', 'untriaged', { at_t: T - 5 }), rec('intent:4', 'handled', { at_t: T - 15 })]), T * 1000, name).stalled, false, 'one decided among them: landing')

// quiet, and off watch
assert.equal(officerDesk(state([], []), T * 1000, name).headline, 'Nothing waiting on it; nothing has come in for 10 min.')
assert.equal(officerDesk(state([], [], false), T * 1000, name).headline, 'Off watch: everything comes straight to you.')
console.log('officerDesk: ok')
