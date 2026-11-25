// The situation report, the pure part: the whole picture assembled top-down
// from the feeds the console already has (vehicles, swarms, the operator's groups, the
// inbox, the situation panel's status), summarised, then expanded on demand. Read-only and display
// only: it decides nothing, sends nothing and re-derives nothing the bridge already says. Built from the ~1 Hz feeds, never per
// frame. Checked by sitrep.check.ts.
import { STATE_ORDER, STATE_WORDS, vehicleState, type LatestOp, type VehicleState } from './exceptions.ts'
import { callsign, staleMs, type Vehicle } from './types.ts'

/** Below this a vehicle counts as low on battery when the swarm summary does not say (it does:
 *  `low_battery_below_pct`). */
export const LOW_BATTERY_PCT = 25

// ---- by group and swarm ---------------------------------------------------------------------

export interface GroupRow {
  key: string
  /** a fleet swarm (fixed by the fleet file), one of the operator's own groups, or one the AI watch
   *  officer made for a tasking request (groups.ts `byOfficer`) */
  kind: 'swarm' | 'yours' | 'officer'
  name: string
  color: string | null
  total: number
  counts: Record<VehicleState, number>
  lowBattery: number
  /** the order it runs, in a line ("orbit i12 (95)"; swarms only) */
  task: string
  tone: 'ok' | 'warn' | 'bad'
  /** the uids, for the drill-down */
  members: number[]
}

export interface SitrepContext {
  latest: ReadonlyMap<number, LatestOp>
  lowBelowPct?: number
}

const emptyCounts = () => Object.fromEntries(STATE_ORDER.map((s) => [s, 0])) as Record<VehicleState, number>

/** How many of `members` are in each state, and how many are low on battery. */
export function tally(members: readonly number[], by: ReadonlyMap<number, Vehicle>, ctx: SitrepContext): { counts: Record<VehicleState, number>; lowBattery: number; here: number } {
  const counts = emptyCounts()
  let lowBattery = 0
  let here = 0
  const low = ctx.lowBelowPct ?? LOW_BATTERY_PCT
  for (const u of members) {
    const v = by.get(u)
    if (!v) continue
    here++
    counts[vehicleState(v, ctx.latest)]++
    if (v.battery_pct != null && v.battery_pct < low) lowBattery++
  }
  return { counts, lowBattery, here }
}




/** A strength line: "97 of 100: 94 on station, 3 returning; 2 lost link". */
export function strengthWords(r: Pick<GroupRow, 'total' | 'counts'>): string {
  const up = r.total - r.counts.lost
  const normal = (['station', 'moving', 'returning', 'idle'] as VehicleState[]).filter((s) => r.counts[s] > 0).map((s) => `${r.counts[s]} ${STATE_WORDS[s]}`)
  const bad = (['lost'] as VehicleState[]).filter((s) => r.counts[s] > 0).map((s) => `${r.counts[s]} ${STATE_WORDS[s]}`)
  return `${up} of ${r.total}${normal.length ? `: ${normal.join(', ')}` : ''}${bad.length ? `; ${bad.join(', ')}` : ''}`
}

// ---- the nodes ------------------------------------------------------------------------------

export interface NodeRow {
  uid: number
  name: string
  state: VehicleState
  /** link health as the fleet list reads it */
  link: 'ok' | 'warn' | 'bad'
  battery: number | null
  lowBattery: boolean
  /** its latest order ("orbit active"), empty when none */
  order: string
  /** the groups it is in, by name (the operator's first) */
  groups: string[]
}

export function linkHealth(v: Pick<Vehicle, 'link'>): 'ok' | 'warn' | 'bad' {
  const since = v.link.since_last_ms ?? (v.link.stale == null || v.link.stale ? Infinity : 0)
  const loss = v.link.loss_pct ?? 0
  if (since > staleMs(v)) return 'bad'
  if (since > (v.link.expected_interval_ms ?? 1000) || loss > 10) return 'warn'
  return 'ok'
}

export function nodeRows(vehicles: readonly Vehicle[], ctx: SitrepContext, groupsOf: ReadonlyMap<number, string[]>): NodeRow[] {
  const low = ctx.lowBelowPct ?? LOW_BATTERY_PCT
  return vehicles.map((v) => {
    const op = ctx.latest.get(v.uid)
    return {
      uid: v.uid,
      name: callsign(v),
      state: vehicleState(v, ctx.latest),
      link: linkHealth(v),
      battery: v.battery_pct,
      lowBattery: v.battery_pct != null && v.battery_pct < low,
      order: op ? `${op.task} ${op.state}` : '',
      groups: groupsOf.get(v.uid) ?? [],
    }
  })
}

export interface NodeFilter {
  state: VehicleState | 'all'
  /** only those not heard now */
  unheard: boolean
  /** only the low on battery */
  low: boolean
  /** names containing this, any case */
  text: string
  /** only the members of these uids (a group opened), or null for every node */
  within: ReadonlySet<number> | null
}

export const ANY: NodeFilter = { state: 'all', unheard: false, low: false, text: '', within: null }

export function filterNodes(rows: readonly NodeRow[], f: NodeFilter): NodeRow[] {
  const q = f.text.trim().toLowerCase()
  return rows.filter(
    (r) =>
      (f.state === 'all' || r.state === f.state) &&
      (!f.unheard || r.state === 'lost') &&
      (!f.low || r.lowBattery) &&
      (!q || r.name.toLowerCase().includes(q)) &&
      (!f.within || f.within.has(r.uid)),
  )
}

/** The worst first (lost, low battery), then by name: what a filtered list opens on. */
export function worstFirst(rows: readonly NodeRow[]): NodeRow[] {
  const rank = (r: NodeRow) => (r.state === 'lost' ? 0 : r.lowBattery ? 1 : r.link !== 'ok' ? 2 : 3)
  return [...rows].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name, 'en', { numeric: true }))
}

/** The rows in view of a scrolling list of fixed-height rows (virtualised: hundreds of nodes build
 *  only what can be seen). */
export function windowOf(count: number, scrollTop: number, viewH: number, rowH: number, overscan = 6): { from: number; to: number } {
  const from = Math.max(0, Math.floor(scrollTop / rowH) - overscan)
  const to = Math.min(count, Math.ceil((scrollTop + viewH) / rowH) + overscan)
  return { from, to: Math.max(from, to) }
}
