// Checks for situationText.ts (no test runner in web/): node web/src/situationText.check.ts, as
// make check does. Throws on the first failure, prints "situationText: ok" otherwise. The records
// are as eval/proxy.py writes them. Not part of the bundle.
import assert from 'node:assert/strict'
import { proposalState, scriptedLine, situationTime, situationZero, STALE_S, whatHappened, withoutCommands } from './situationText.ts'

const nameOf = (uid: string) => ({ '1': 'DRONE 1', '2': 'CAR 1' })[uid] ?? `vehicle ${uid}`
const zero = 1795517866.0
const l = scriptedLine({ t: zero + 230, intent_id: 9, action: 'approve', by: 'op-auto', reason: 'scripted: approves every request', result: 'dispatched' }, zero, nameOf)
assert.equal(l.at, 'T+3:50')
assert.equal(l.text, 'op-auto approved i9: dispatched', 'said once, as what it was')
assert.equal(l.why, 'scripted: approves every request')
assert.equal(scriptedLine({ t: zero, intent_id: 9, action: 'approve', by: 'op-auto' }, null, nameOf).at, null, 'no clock yet: no time')

// the rest of what the scripts decide
assert.equal(scriptedLine({ t: zero, intent_id: 9, action: 'approve', by: 'op-auto', granted: true, result: 'dispatched' }, zero, nameOf).text, 'op-auto approved i9 (it grants every request of the AI): dispatched')
assert.equal(scriptedLine({ t: zero, intent_id: 10, action: 'sign', by: 'op-2', result: 'pending_approval' }, zero, nameOf).text, 'op-2 signed i10: pending_approval')
assert.equal(scriptedLine({ t: zero, intent_id: 10, action: 'sign_failed', by: 'op-3', result: 'error 409: stale' }, zero, nameOf).text, 'op-3 could not sign i10: error 409: stale')
assert.equal(
  scriptedLine({ t: zero, intent_id: null, action: 'confirm_lost', by: 'op-2', uid: 1, callsign: null, result: 'confirmed lost (ended [1])' }, zero, nameOf).text,
  'op-2 confirmed DRONE 1 lost: confirmed lost (ended [1])',
  'a loss confirmed: never "confirm_lost inull"',
)
assert.equal(situationZero({ t: zero + 509, elapsed_s: 509 }), zero)
assert.equal(situationZero({ t: zero, elapsed_s: null }), null)

// the clock on this console: run on from the last status while the runner runs, held otherwise
const clock = { elapsed_s: 100, duration_s: 720 }
assert.deepEqual(situationTime(clock, 'running', { elapsed: 100, at: 50 }, 53), { elapsed: 103, left: 617, stale: false, running: true, sincePost: 3 })
assert.equal(situationTime(clock, 'running', { elapsed: 100, at: 50 }, 50 + STALE_S + 1).stale, true, 'no status for STALE_S: not reporting')
assert.equal(situationTime(clock, 'running', { elapsed: 100, at: 50 }, 50 + STALE_S + 1).elapsed, 100, 'and its clock is held')
assert.equal(situationTime(clock, 'ended', { elapsed: 720, at: 50 }, 90).left, 0, 'ended: nothing left')
assert.equal(situationTime({ elapsed_s: 715, duration_s: 720 }, 'running', { elapsed: 715, at: 0 }, 9).elapsed, 720, 'never past its duration')
assert.equal(situationTime({ elapsed_s: null, duration_s: 720 }, 'starting', null, 9).elapsed, null, 'before it starts')

// the end line without the command
assert.equal(
  withoutCommands("the situation's time is up; the stack stays up for inspection until you end it (End in the menu, or make down)"),
  "the situation's time is up; the stack stays up for inspection until you end it (End in the menu)",
)
assert.equal(withoutCommands('stopping the runner; the stack keeps running: make down stops it'), 'stopping the runner; the stack keeps running')

// what happened: the timeline, the scripts' decisions and the AI's proposals, the newest first
const happened = whatHappened(
  [{ t_s: 230, kind: 'autoplay', text: 'op-auto (scripted) approved i9' }, { t_s: 210, kind: 'runner', text: 'the fleet is up: our vehicle 1 ready' }],
  [{ id: 4, kind: 'orbit', created_t: (zero + 60) * 1000, state: 'dispatched', uids: ['1'], approval: { by: 'op-auto' }, rationale: 'cover the channel' }],
  zero,
  'Claude',
  new Set(['op-auto']),
  nameOf,
)
assert.deepEqual(
  happened.map((h) => [h.t_s, h.kind]),
  [
    [230, 'timeline'],
    [210, 'timeline'],
    [60, 'ai'],
  ],
  'merged by time, in plain words, each scripted decision once (the timeline says it)',
)
assert.equal(happened[1].text, 'the fleet is up: DRONE 1 ready')
assert.equal(happened[2].text, 'The AI (Claude) proposed orbit i4 for DRONE 1: approved by op-auto (scripted)')
assert.equal(happened[2].title, 'cover the channel', "the AI's rationale in the hover")
assert.equal(proposalState({ state: 'pending_approval', approval: null }, new Set()), 'waiting for a decision')
assert.equal(proposalState({ state: 'dispatched', approval: null }, new Set()), 'sent by policy')
assert.equal(proposalState({ state: 'queued', approval: { by: 'op-1' } }, new Set()), 'approved by op-1, held for its link')

console.log('situationText: ok')
