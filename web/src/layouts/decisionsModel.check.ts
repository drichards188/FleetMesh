// Checks for decisionsModel.ts: node web/src/layouts/decisionsModel.check.ts (prints "decisionsModel: ok").
import assert from 'node:assert/strict'
import { oursLine, overMap, REF_IDS, REF_LABEL, REF_TITLE, toggleRef } from './decisionsModel.ts'

// the reference buttons, one at a time
assert.equal(toggleRef(null, 'fleet'), 'fleet', 'a closed list opens')
assert.equal(toggleRef('fleet', 'fleet'), null, 'the list shown closes')
assert.equal(toggleRef('fleet', 'log'), 'log', 'another list takes its place')
assert.deepEqual([...REF_IDS], ['fleet', 'log', 'parcels'], 'in the bar the order is Fleet, Log, Parcels')
for (const id of REF_IDS) {
  assert.ok(REF_LABEL[id], `${id} has a label`)
  assert.ok(REF_TITLE[id].startsWith(`${REF_LABEL[id]}: `), `${id}'s title names it first`)
  assert.ok(!/\u2014/.test(REF_TITLE[id]), `${id}'s title has no em dash`)
}

// over the map's right edge: one drawer at a time; the Inbox is the column, never over the map
assert.equal(overMap(null, null), null, 'nothing open: nothing over the map')
assert.equal(overMap('inbox', null), null, 'the Inbox is the column')
assert.equal(overMap('inbox', 'fleet'), 'ref', 'the dock resting on the Inbox: the list shown')
assert.equal(overMap(null, 'log'), 'ref', 'the dock closed: the list shown')
assert.equal(overMap('queue', null), 'dock', "a drawer of the dock's")
assert.equal(overMap('officer', 'fleet'), 'dock', "a drawer of the dock's wins over a list, which the layout then closes")

// the quiet state's glance
assert.deepEqual(oursLine(22, 0), { n: 22, words: 'vehicles, none in trouble' })
assert.deepEqual(oursLine(1, 0), { n: 1, words: 'vehicle, none in trouble' }, 'one vehicle, singular')
assert.deepEqual(oursLine(105, 5), { n: 105, words: 'vehicles, 5 in trouble' })
assert.deepEqual(oursLine(0, 0), { n: null, words: 'None in the fleet yet' }, 'no number before the fleet reports')

console.log('decisionsModel: ok')
