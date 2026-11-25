// Checks for topbar.ts: node web/src/topbar.check.ts, as make check does. The claims (bd
// murmur-iobd, ledger 1 section 1): the top bar counts ours in trouble with the fleet rows' own
// slot, every kind of it, its tone the worst among them; a click walks through them in fleet order;
// the glance line is the Sitrep's bottom line (ours); the Sitrep's
// button waits for group scale. Prints "topbar: ok", throws on the first failure.
import { glanceWords, GROUP_SCALE, groupScale, nextTrouble, runStanding, troubleOf } from './topbar.ts'
import type { Vehicle } from './types'
import { NO_EXCEPTIONS } from './vehicleException.ts'

function check(cond: unknown, what: string) {
  if (!cond) throw new Error(`topbar: ${what}`)
}

const v = (uid: number, o: { stale?: boolean; loss?: number } = {}): Vehicle =>
  ({
    uid, sysid: uid, kind: 'copter',
    link: { stale: o.stale ?? false, since_last_ms: o.stale ? 12_000 : 200, expected_interval_ms: 1000, loss_pct: o.loss ?? 0 },
    identity: { callsign: `DRONE ${uid}`, platform: 'delivery_drone', short: 'Drone', domain: 'air' },
  }) as unknown as Vehicle

// nothing wrong: nothing counted
const calm = troubleOf([v(1), v(2)], NO_EXCEPTIONS)
check(calm.n === 0 && calm.tone === null && calm.uids.length === 0, 'none in trouble')

// every kind counts, the worst tone wins, fleet order kept
const low = (uid: number): Vehicle => ({ ...v(uid), battery_pct: 5 }) as Vehicle
const t = troubleOf([v(1), v(2, { stale: true }), low(3), v(5, { loss: 30 })], NO_EXCEPTIONS)
check(t.n === 2 && t.uids.join() === '2,3', `silent and low battery count, a degraded link does not: ${t.uids.join()}`)
check(troubleOf([v(5, { loss: 30 })], NO_EXCEPTIONS).n === 0, 'a lossy link alone is the row warning, not the bar')
check(t.tone === 'bad', 'a silent one makes it red')
check(t.says.includes('DRONE 3: low battery 5%'), `the hover names each in its slot's words: ${t.says}`)
const amber = troubleOf([low(4)], NO_EXCEPTIONS)
check(amber.n === 1 && amber.tone === 'warn', 'a low battery alone is amber, as its slot')
const settled = troubleOf([v(6)], { ...NO_EXCEPTIONS, nodes: { on: true, review: new Map(), lost: new Map([[6, {} as never]]) } })
check(settled.n === 1 && settled.tone === 'settled', 'a loss an operator confirmed still counts, grey')

// a click walks through them
check(nextTrouble([2, 3, 5], null) === 2, 'the first')
check(nextTrouble([2, 3, 5], 2) === 3 && nextTrouble([2, 3, 5], 5) === 2, 'the next, and round again')
check(nextTrouble([2, 3, 5], 9) === 2, 'one no longer in trouble: from the first')
check(nextTrouble([], 2) === null, 'none')

// the glance in words: "N vehicles", then "N of ours in trouble"
const words = (p: ReturnType<typeof glanceWords>) => p.map((x) => (x.n != null ? `${x.n} ${x.text}` : x.text)).join('')
const big = { ours: 105 }
check(words(glanceWords(big, 3, false)) === '105 vehicles · 3 of ours in trouble', `full: ${words(glanceWords(big, 3, false))}`)
check(words(glanceWords(big, 3, true)) === '3 of ours in trouble', `folded into the problem: ${words(glanceWords(big, 3, true))}`)
check(words(glanceWords({ ours: 1 }, 0, false)) === '1 vehicle', 'singular, without trouble')
check(glanceWords(big, 0, true).length === 0, 'folded with nothing wrong: nothing')
check(glanceWords(big, 3, false).filter((x) => x.kind === 'trouble').every((x) => x.n! > 0), 'a problem shows only above 0')

// the run's standing beside its title
const run = { operator: 'autoplay' as const, phase: 'running', stale: false }
check(runStanding(run).label === 'Autoplay' && runStanding({ ...run, operator: 'live' }).label === 'You decide', 'who decides while it runs')
check(runStanding({ ...run, phase: 'ended' }).label === 'Ended' && runStanding({ ...run, phase: 'ended' }).tone === 'quiet', 'ended, quietly')
check(runStanding({ ...run, stale: true }).tone === 'warn' && runStanding({ ...run, phase: 'failed' }).tone === 'warn', 'a silent or failed runner in amber')
check([runStanding(run), runStanding({ ...run, phase: 'ended' }), runStanding({ ...run, stale: true })].every((s) => !(s.title + s.label).includes(String.fromCharCode(0x2014))), 'no em dash in the words')

// group scale
check(!groupScale(22) && !groupScale(GROUP_SCALE) && groupScale(GROUP_SCALE + 1), 'the Sitrep waits for more than GROUP_SCALE of ours')

console.log('topbar: ok')
