// Checks for groups.ts: node web/src/groups.check.ts (prints "groups: ok").
import assert from 'node:assert/strict'
import { byOfficer, defaultName, groupsFor, nameProblem, officerNote, splitByMaker, type OperatorGroup } from './groups.ts'

const g = (id: string, members: number[], color: string | null = null, name = id): OperatorGroup => ({ id, name, color, members, created_t: 1, updated_t: 1 })

// names
assert.equal(nameProblem('  '), 'A group needs a name.')
assert.equal(nameProblem('x'.repeat(65)), 'A name is at most 64 characters.')
assert.equal(nameProblem('North picket'), null)
assert.equal(nameProblem('⚓'.repeat(64)), null, 'characters, not UTF-16 units, as the bridge counts')
assert.equal(nameProblem('🛥'.repeat(64)), null, 'one character each though two UTF-16 units')




// names for new groups
assert.equal(defaultName([]), 'Group 1')
assert.equal(defaultName([g('a', [], null, 'Group 1'), g('b', [], null, 'Group 3')]), 'Group 2', 'the lowest number free')

// who made a group: the operator's own, the officer's for a tasking item (an older bridge says
// nothing of either: the operator's)
const officers = { ...g('og-2', [1, 2], null, 'West picket'), by: 'agent-watch', agent: true, tasking: 'tk-3' }
const opsOwn = { ...g('og-3', [3]), by: 'op-1' }
const old = g('og-4', [4])
assert.ok(byOfficer(officers) && !byOfficer(opsOwn) && !byOfficer(old))
const split = splitByMaker([officers, opsOwn, old])
assert.deepEqual([split.yours.map((x) => x.id), split.officer.map((x) => x.id)], [['og-3', 'og-4'], ['og-2']])
assert.deepEqual(groupsFor([officers, opsOwn, { ...officers, id: 'og-5', tasking: 'tk-4' }], 'tk-3').map((x) => x.id), ['og-2'], "the officer's for that item only")
assert.match(officerNote(officers), /^Made by the AI watch officer \(agent-watch\) for tasking tk-3: organisation only, it orders nothing\. You may change or delete it/)
assert.match(officerNote({ ...officers, changed_by: 'agent-watch' }), /You may change or delete it/, 'its own change is still its own')
assert.match(officerNote({ ...officers, changed_by: 'op-1' }), /Changed by op-1 since, so the officer can no longer change it\.$/)
console.log('groups: ok')
