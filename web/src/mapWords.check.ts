// Checks for mapWords.ts (no test runner in web/): node web/src/mapWords.check.ts, as make check
// does. The map's labels say what a person reads, in words: no raw ids, no engine codes. Throws on
// the first failure, prints "mapWords: ok" otherwise.
import { estimateWords, mayBeRunningWords, queuedWords, resumedWords, spanWords } from './mapWords.ts'

function check(cond: unknown, what: string) {
  if (!cond) throw new Error(`mapWords: ${what}`)
}
const eq = (got: string, want: string) => check(got === want, `${JSON.stringify(got)} is not ${JSON.stringify(want)}`)

// spans: seconds under a minute and a half, then minutes, then hours
eq(spanWords(47), '47 s')
eq(spanWords(89.4), '89 s')
eq(spanWords(90), '1 min')
eq(spanWords(240), '4 min')
eq(spanWords(7500), '2 h 05 min')
eq(spanWords(-3), '0 s')

// a silent vehicle's estimate: what it is probably doing and how long it has been silent
eq(estimateWords({ failsafe: 'rtl' }, 47), 'probably returning (lost-link RTL); silent 47 s')
eq(estimateWords({ basis: 'orbit', second: false }, 47), 'probably on its orbit; silent 47 s')
eq(estimateWords({ basis: 'velocity', second: false }, 200), 'probably on its last course; silent 3 min')
// the task model once the failsafe leads: the other hypothesis, said as one
eq(estimateWords({ basis: 'patrol', second: true }, 47), 'if it kept its task: on its patrol route')
check(!/EST|RTL [A-Z]/.test(estimateWords({ basis: 'hold', second: false }, 5)), 'no engine codes in the task model')

// orders: what and why in words, never the raw intent id
eq(queuedWords('orbit', 'link down', 125), 'queued orbit: link down, 2 min left')
eq(queuedWords('patrol', 'silent for 12.3 s', null), 'queued patrol: silent for 12.3 s')
eq(resumedWords('patrol'), 'resumed patrol: waiting for the vehicle')
// a person decides: it keeps its capitals
eq(mayBeRunningWords('orbit', 'pending_approval'), 'MAY BE RUNNING orbit: its start went out unconfirmed (pending approval)')
for (const t of [queuedWords('orbit', 'link down', 60), resumedWords('hold'), mayBeRunningWords('rtl', 'voided')]) check(!/\bi\d+\b/.test(t), `no raw id in ${t}`)

console.log('mapWords: ok')
