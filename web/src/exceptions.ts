// Lists by exception (docs/research/saturation-display.md R6): the fleet list by group and the
// command log one card per order with its vehicles' states counted. In Saturation the fleet list
// was 3,602 identical five-line rows and the command log one line per vehicle per group order (P9
// to P11). What a fold holds is counted on it and one click away; exceptions are never folded. A
// fleet without groups is listed as before. Display only.
//
// Pure (no browser): exceptions.check.ts runs it under node.
import type { Op, Tasking, Vehicle } from './types'
import { isStale } from './types.ts'

// ---- the fleet list by group --------------------------------------------------------------------

/** A vehicle's state as its group's row counts it: the first that holds of lost link, returning (RTL, landing), on station (its order's op active, or
 *  done for an orbit, patrol or hold: an orbit is done once established, a patrol after its
 *  first lap, and both go on), moving, idle. */
export type VehicleState = 'lost' | 'returning' | 'station' | 'moving' | 'idle'

/** The row's order: the normal states, then the exceptions (drawn in the alert colour). */
export const STATE_ORDER: VehicleState[] = ['station', 'moving', 'returning', 'idle', 'lost']

export const STATE_WORDS: Record<VehicleState, string> = {
  station: 'on station',
  moving: 'moving',
  returning: 'returning',
  idle: 'idle',
  lost: 'lost link',
}

/** A mode that takes the vehicle home or down: RTL, SMART_RTL, QRTL, AUTOLAND, LAND. */
const RETURNING = /RTL|RETURN|LAND/

export function isReturning(mode: string): boolean {
  return RETURNING.test(mode.toUpperCase())
}

export interface LatestOp {
  state: string
  task: string
}

/** Each vehicle's latest op: of the newest tasking that names it. */
export function latestOps(taskings: readonly Pick<Tasking, 'id' | 'task' | 'ops'>[]): Map<number, LatestOp> {
  const out = new Map<number, LatestOp>()
  for (const t of [...taskings].sort((a, b) => a.id - b.id)) for (const o of t.ops) out.set(o.uid, { state: o.state, task: t.task })
  return out
}

/** On its order's station: the op running, or done for a task that goes on once done. */
function onStation(op: LatestOp | undefined): boolean {
  return op != null && (op.state === 'active' || (op.state === 'done' && op.task !== 'rtl'))
}

type V = Pick<Vehicle, 'uid' | 'link' | 'mode' | 'groundspeed'>

export function vehicleState(v: V, latest: ReadonlyMap<number, LatestOp>): VehicleState {
  if (isStale(v)) return 'lost'
  if (v.mode && isReturning(v.mode)) return 'returning'
  if (onStation(latest.get(v.uid))) return 'station'
  if ((v.groundspeed ?? 0) >= 1) return 'moving'
  return 'idle'
}

export interface GroupContext {
  latest: ReadonlyMap<number, LatestOp>
  /** vehicles with an alert of their own (not folded into their group's) */
  alerted: ReadonlySet<number>
  selected: ReadonlySet<number>
  /** its details open */
  open: number | null
  /** just revealed from outside the list (a map click) */
  flash: number | null
}

/** A folded group lists at most this many members on their own (the open and the revealed one
 *  always); the rest are counted. */
export const GROUP_SHOWN = 5

export interface GroupView<T> {
  counts: Record<VehicleState, number>
  /** listed on their own: everyone when open, else the exceptions, the most pressing first */
  shown: T[]
  /** members not listed */
  folded: number
  /** of the folded, how many are exceptions or selected (said on the "more" line) */
  foldedExceptions: number
  selected: number
}

/** Why a member is listed on its own in a folded group, the lower the more pressing; null: it
 *  is not an exception. */
function exceptionRank(v: V, state: VehicleState, ctx: GroupContext): number | null {
  if (v.uid === ctx.open) return 0
  if (v.uid === ctx.flash) return 1
  if (state === 'lost') return 3
  if (ctx.alerted.has(v.uid)) return 4
  if (ctx.selected.has(v.uid)) return 5
  return null
}

/** A group's row: its members counted by state, and who is listed on their own. */
export function groupView<T extends V>(members: readonly T[], ctx: GroupContext, expanded: boolean): GroupView<T> {
  const counts = Object.fromEntries(STATE_ORDER.map((s) => [s, 0])) as Record<VehicleState, number>
  const ranked: { v: T; rank: number }[] = []
  const rest: T[] = []
  let selected = 0
  for (const v of members) {
    const state = vehicleState(v, ctx.latest)
    counts[state]++
    if (ctx.selected.has(v.uid)) selected++
    const rank = exceptionRank(v, state, ctx)
    if (rank == null) rest.push(v)
    else ranked.push({ v, rank })
  }
  ranked.sort((a, b) => a.rank - b.rank)
  if (expanded) return { counts, shown: [...ranked.map((r) => r.v), ...rest], folded: 0, foldedExceptions: 0, selected }
  const shown = ranked.filter((r, k) => r.rank <= 1 || k < GROUP_SHOWN).map((r) => r.v)
  return { counts, shown, folded: members.length - shown.length, foldedExceptions: ranked.length - shown.length, selected }
}

// ---- the command log: one card per order, its vehicles counted -----------------------------------

/** An order to more vehicles than this is summarised (fewer are listed vehicle by vehicle, as before). */
export const OPS_FOLD = 5
/** An op waiting this long in a step before active (sent, accepted...) is stuck: listed, s. */
export const STUCK_S = 60

/** The op states as the summary counts them, in the lifecycle's order, then the endings. */
export const OP_ORDER = ['queued', 'sent', 'starting', 'acked', 'active', 'done', 'requeued', 'unconfirmed', 'superseded', 'failed']
const WAITING = new Set(['queued', 'sent', 'starting', 'acked'])
const ENDED_SHORT = new Set(['failed', 'superseded', 'unconfirmed', 'requeued'])

export interface OpException {
  /** the op state, or `stuck` */
  state: string
  detail: string
  uids: number[]
}

export interface OrderSummary {
  total: number
  /** [state, count] in OP_ORDER, the states present only */
  counts: [string, number][]
  /** every op that failed, was superseded, may be running, went back to the queue or is stuck,
   *  grouped by state and reason, the largest group first: always listed */
  exceptions: OpException[]
}

/** An order's ops counted by state, with its exceptions grouped (`nowS`: the bridge's clock, s). */
export function orderSummary(ops: readonly Pick<Op, 'uid' | 'state' | 'detail' | 'history'>[], nowS: number): OrderSummary {
  const counts = new Map<string, number>()
  const groups = new Map<string, OpException>()
  for (const o of ops) {
    counts.set(o.state, (counts.get(o.state) ?? 0) + 1)
    const since = o.history.length ? o.history[o.history.length - 1].t : nowS
    const state = ENDED_SHORT.has(o.state) ? o.state : WAITING.has(o.state) && nowS - since > STUCK_S ? 'stuck' : null
    if (!state) continue
    const key = `${state}|${o.detail}`
    const g = groups.get(key)
    if (g) g.uids.push(o.uid)
    else groups.set(key, { state, detail: o.detail, uids: [o.uid] })
  }
  return {
    total: ops.length,
    counts: OP_ORDER.flatMap((s) => (counts.get(s) ? [[s, counts.get(s)!] as [string, number]] : [])),
    exceptions: [...groups.values()].sort((a, b) => b.uids.length - a.uids.length || a.state.localeCompare(b.state)),
  }
}
