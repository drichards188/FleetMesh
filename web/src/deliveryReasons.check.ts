// Checks for deliveryReasons.ts: node web/src/deliveryReasons.check.ts (prints "deliveryReasons: ok").
// The claims: every reason code the delivery rules give has a plain-language label and sentence (read
// from the engine's source, so a new code without words fails here); an unknown code still reads;
// the inputs a rule used are read out of its detail with their limits; a confirm order's evidence and
// look are read.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { evidenceOf, heldChecks, inputRows, lookInRationale, reasonLabel, reasonOf, REASONS } from './deliveryReasons.ts'

// every code in the rules' source has words (a hold's `_MARGINAL` suffix is built from its base)
const src = readFileSync(new URL('../../crates/fleetmesh-decide/src/delivery_rules.rs', import.meta.url), 'utf8')
const codes = new Set([...src.matchAll(/"([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)"/g)].map((m) => m[1]))
// strings that are not codes (a rule's id, a note's key)
const notCodes = new Set(['LINK_ON_BACKUP'])
const missing = [...codes].filter((c) => !notCodes.has(c) && !(c in REASONS) && !(`${c}_MARGINAL` in REASONS))
assert.deepEqual(missing, [], `reason codes without plain words: ${missing.join(', ')}`)
for (const [code, r] of Object.entries(REASONS)) {
  assert.ok(r.title.length > 3 && r.plain.endsWith('.'), `${code} has a title and a sentence`)
  assert.ok(!r.plain.includes('—'), `${code}: no em dash`)
}
assert.equal(reasonLabel('DZ_UNCONFIRMED'), 'Drop zone not confirmed lately')
assert.equal(reasonOf('SOME_NEW_CODE').title, 'Some new code', 'a code from a newer bridge reads from itself')

// the inputs a rule used
const gust = inputRows('weather', 'gust 11.4 m/s is above the soft limit 11.0')
assert.deepEqual(gust, [{ label: 'Gust', value: '11.4 m/s', against: 'soft limit 11.0 m/s', tone: 'over' }])
const wind = inputRows('weather', 'sustained wind 8.6 m/s is above the soft limit 8.0; gust 12.0 m/s is above the soft limit 11.0')
assert.deepEqual(wind.map((r) => r.label), ['Wind', 'Gust'], 'wind and gust, each against its limit')
const pass = inputRows('weather', 'wind 2.8 m/s, gust 4.7 m/s, visibility 12941 m, precipitation 0.0 mm/h are within limits (19 samples)')
assert.deepEqual(pass.map((r) => `${r.label} ${r.value}`), ['Wind 2.8 m/s', 'Gust 4.7 m/s'])
const energy = inputRows('energy', 'battery 57%, need 52.9% (route 17.2 plus return 7.1, times 1.15 for wind margin, equals 27.9, plus reserve 25), margin 4.1 points, hold band 5')
assert.equal(energy[0].value, '57%')
assert.equal(energy[0].against, 'needs 52.9%')
assert.equal(energy[0].tone, 'near', 'inside the hold band')
assert.equal(energy.find((r) => r.label === 'Reserve')?.value, '25%')
assert.equal(energy.find((r) => r.label === 'Margin')?.against, 'hold band 5 points')
const low = inputRows('energy', 'battery 40%, need 52.9% (route 17.2 plus return 7.1, times 1.15 for wind margin, equals 27.9, plus reserve 25), margin -12.9 points, hold band 5')
assert.equal(low[0].tone, 'over')
assert.deepEqual(inputRows('lost_link', 'the link is predicted down for 45 s on the route (limit 20 s) and no backup covers it'), [{ label: 'Predicted link gap', value: '45 s', against: 'limit 20 s', tone: 'over' }])
assert.equal(inputRows('lost_link', 'the backup radio carries a low data rate: a person judges a delivery on it')[0].label, 'Backup radio')
assert.equal(inputRows('nav_integrity', 'horizontal accuracy 7.5 m is above 5 m')[0].against, 'trusted up to 5 m')
assert.equal(inputRows('nav_integrity', '9 satellites tracked, fewer than 10')[0].label, 'Satellites')
assert.equal(inputRows('separation', "10's position is 92 s old and it is within 5 km of the route")[0].value, '92 s old')
assert.equal(inputRows('dropzone', 'drop zone dz-city-02 was not confirmed within 3600 s')[0].value, 'none in the last 60 min')
assert.equal(inputRows('dropzone', 'gust 9.0 m/s at the zone is above the descent limit 8.0')[0].label, 'Gust at the zone')
assert.deepEqual(inputRows('weather', 'something the patterns do not know'), [], 'nothing is made up')

// which checks put an order in front of a person
const held = heldChecks([
  { rule: 'energy', uid: '1', outcome: 'pass', detail: 'x' },
  { rule: 'dropzone', uid: '1', outcome: 'hold', detail: 'y', codes: ['DZ_UNCONFIRMED'] },
  { rule: 'geofence', uid: '1', outcome: 'fail', detail: 'z', codes: ['GEOFENCE_NO_GO'] },
  { rule: 'separation', uid: '1', outcome: 'hold', detail: 'w' },
])
assert.deepEqual(held.map((h) => h.rule), ['dropzone', 'separation', 'geofence'], 'holds first, then refusals; passes left out')
assert.deepEqual(held[1].codes, [])

// confirm_drop_zone's evidence and look
assert.deepEqual(evidenceOf(['obs:dz-city-02:1795590613605', 'other']), [{ zone: 'dz-city-02', atMs: 1795590613605 }])
assert.deepEqual(evidenceOf(undefined), [])
assert.deepEqual(lookInRationale('dispatcher: dz-city-02 has not been confirmed for p-0001; the latest look: Clear (0 seen) at 2026-11-25T07:10:13Z'), { state: 'clear', count: 0, at: '2026-11-25T07:10:13Z' })
assert.equal(lookInRationale('no look here'), null)

console.log('deliveryReasons: ok')
