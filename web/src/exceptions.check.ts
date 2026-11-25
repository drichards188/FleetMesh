// Checks for exceptions.ts (no test runner in web/): node web/src/exceptions.check.ts, as make check
// does. The claims (docs/research/saturation-display.md R6): a vehicle's state is the first that
// holds of lost link, returning, on station (its newest order's op active), moving, idle;
// a folded group counts every member by state and lists only its exceptions, the most pressing
// first, at most GROUP_SHOWN but always the open and the revealed one; an open group lists everyone,
// exceptions first; an order's
// summary counts every op and lists every failure, supersession, unconfirmed, requeued and stuck
// op grouped by reason. Throws on the first failure, prints "exceptions: ok" otherwise.
import type { Op, Tasking, Vehicle } from './types'
import { GROUP_SHOWN, groupView, latestOps, OPS_FOLD, orderSummary, STUCK_S, vehicleState, type GroupContext } from './exceptions.ts'

function check(cond: unknown, what: string) {
  if (!cond) throw new Error(`exceptions: ${what}`)
}

type V = Pick<Vehicle, 'uid' | 'link' | 'mode' | 'groundspeed'>
const veh = (uid: number, extra: Partial<V> = {}): V => ({ uid, link: { stale: false, since_last_ms: 100, loss_pct: 0, rtt_ms: 10 }, mode: 'AUTO', groundspeed: 0, ...extra })
const op = (uid: number, state: Op['state'], detail = '', t = 1000): Pick<Op, 'uid' | 'state' | 'detail' | 'history'> => ({ uid, state, detail, history: [{ state, t }] })

// the newest order naming a vehicle says its op state
const taskings = [
  { id: 2, task: 'orbit', ops: [op(1, 'active'), op(2, 'failed'), op(4, 'done')] },
  { id: 1, task: 'patrol', ops: [op(1, 'failed'), op(3, 'active')] },
  { id: 3, task: 'rtl', ops: [op(5, 'done')] },
] as unknown as Tasking[]
const latest = latestOps(taskings)
check(latest.get(1)?.state === 'active' && latest.get(2)?.state === 'failed' && latest.get(3)?.state === 'active', 'the newest tasking wins')

// a vehicle's state: the first that holds
check(vehicleState(veh(1, { link: { stale: true, since_last_ms: 9000, loss_pct: 0, rtt_ms: null } }), latest) === 'lost', 'lost link')
check(vehicleState(veh(1, { mode: 'RTL' }), latest) === 'returning' && vehicleState(veh(1, { mode: 'AUTOLAND' }), latest) === 'returning', 'returning by mode')
check(vehicleState(veh(1), latest) === 'station', 'on station: its op active')
check(vehicleState(veh(4), latest) === 'station', 'on station: its orbit done (established) goes on')
check(vehicleState(veh(5), latest) === 'idle' && vehicleState(veh(2), latest) === 'idle', 'an rtl done or a failed op is not on station')
check(vehicleState(veh(9, { groundspeed: 12 }), latest) === 'moving', 'moving')
check(vehicleState(veh(9), latest) === 'idle', 'idle')

// a folded group: everyone counted, exceptions listed, the most pressing first, capped
const members = Array.from({ length: 100 }, (_, i) => veh(100 + i, { groundspeed: 12 }))
members[3] = veh(103, { link: { stale: true, since_last_ms: 9000, loss_pct: 0, rtt_ms: null } })
members[50] = veh(150, { mode: 'RTL' })
const lostLots = members.map((v, i) => (i >= 90 ? veh(v.uid, { link: { stale: true, since_last_ms: 9000, loss_pct: 0, rtt_ms: null } }) : v))
const ctx: GroupContext = { latest: new Map(), alerted: new Set([130]), selected: new Set([140]), open: null, flash: null }
{
  const g = groupView(members, ctx, false)
  check(g.counts.moving === 98 && g.counts.lost === 1 && g.counts.returning === 1, `counts by state ${JSON.stringify(g.counts)}`)
  check(Object.values(g.counts).reduce((a, b) => a + b, 0) === 100, 'every member counted once')
  check(g.shown.map((v) => v.uid).join() === '103,130,140', `exceptions in order: lost, alerted, selected (${g.shown.map((v) => v.uid)})`)
  check(g.folded === 97 && g.foldedExceptions === 0 && g.selected === 1, `the rest folded (${g.folded}, ${g.foldedExceptions})`)
}
{
  const g = groupView(lostLots, { ...ctx, open: 170, flash: 160 }, false)
  check(g.shown[0].uid === 170 && g.shown[1].uid === 160, 'the open and the revealed one first')
  check(g.shown.length === GROUP_SHOWN, `at most GROUP_SHOWN listed (${g.shown.length})`)
  check(g.foldedExceptions > 0, 'the folded exceptions are counted on the more line')
  const many = groupView(lostLots, { ...ctx, open: 170, flash: 160 }, false)
  check(many.shown.some((v) => v.uid === 170) && many.shown.some((v) => v.uid === 160), 'the open and revealed one are listed whatever the cap')
}
{
  const g = groupView(members, ctx, true)
  check(g.shown.length === 100 && g.folded === 0 && g.shown[0].uid === 103, 'an open group lists everyone, exceptions first')
}

// an order's summary
{
  const now = 5000
  const ops = [
    ...Array.from({ length: 60 }, (_, i) => op(i, 'active', 'patrolling', now - 10)),
    ...Array.from({ length: 30 }, (_, i) => op(100 + i, 'failed', 'vehicle left the task, now in RTL: GCS Failsafe Cleared', now - 10)),
    ...Array.from({ length: 5 }, (_, i) => op(200 + i, 'failed', 'MAV_CMD rejected', now - 10)),
    op(300, 'superseded', 'replaced by i9', now - 10),
    op(301, 'sent', 'sent', now - STUCK_S - 1),
    op(302, 'sent', 'sent', now - 5),
    op(303, 'unconfirmed', 'link lost after its start went out', now - 10),
    op(304, 'requeued', 'link lost before it started', now - 10),
  ]
  const s = orderSummary(ops, now)
  check(s.total === ops.length && s.counts.reduce((n, [, c]) => n + c, 0) === ops.length, 'every op counted')
  check(s.counts.map(([st]) => st).join() === 'sent,active,requeued,unconfirmed,superseded,failed', `counts in lifecycle order (${s.counts.map(([st]) => st)})`)
  const ex = s.exceptions.map((e) => `${e.state}:${e.uids.length}`)
  check(ex.join() === 'failed:30,failed:5,requeued:1,stuck:1,superseded:1,unconfirmed:1', `exceptions grouped by reason, largest first (${ex})`)
  check(!s.exceptions.some((e) => e.uids.includes(302)), 'an op sent a moment ago is not stuck')
  check(s.exceptions.find((e) => e.state === 'stuck')?.uids[0] === 301, 'an op waiting past STUCK_S is')
  check(OPS_FOLD < 100, 'an order to a group is summarised')
}

console.log('exceptions: ok')
