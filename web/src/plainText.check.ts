// Checks for plainText.ts (no test runner in web/): node web/src/plainText.check.ts, as make check
// does. Throws on the first failure, prints "plainText: ok" otherwise. Not part of the bundle.
import assert from 'node:assert/strict'
import { agoWhen, clockText, epochMs, plainText, situationWhen } from './plainText.ts'

const names: Record<string, string> = { '1': 'DRONE 1', '2': 'CAR 1' }
const nameOf = (uid: string) => names[uid] ?? `vehicle ${uid}`
const now = 1795533619054 + 121_000

// ---- epoch stamps --------------------------------------------------------------------------------
assert.equal(epochMs(1795533627.2), 1795533627200, 'the bridge writes seconds')
assert.equal(epochMs(1795533619054), 1795533619054, 'the engine writes ms')
assert.equal(clockText(225), '3:45')
assert.equal(clockText(3725), '1:02:05')

// ---- a reason as the bridge logs it ---------------------------------------------------------------
const reason = 'no report from our vehicle 1 since t=1795533619054'
assert.equal(plainText(reason, { nameOf, when: agoWhen(now) }), 'no report from DRONE 1 2 min ago')
assert.equal(plainText('no report from our vehicle 2 at t=1795533619054', { nameOf, when: situationWhen(1795533619.054 - 210) }), 'no report from CAR 1 at T+3:30')
for (const s of ['reserve 8 min (20%)', 'at 0.88', 't=42 s', 'vehicle 1 of ours']) assert.equal(plainText(s, { nameOf, when: agoWhen(now) }), s)

console.log('plainText: ok')
