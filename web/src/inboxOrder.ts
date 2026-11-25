// The Inbox's two lists, "Needs you" and "For information" (ledger 2 section 2, D3, requirements
// R8 to R10, rule 4: no alarm styling without a response expected). One function says where each
// item goes and in what order, so the drawer's list, its tab and the Inbox button's badge count the
// same things. Display only: nothing here decides, resolves or hides anything in the bridge or the
// engine. Pure: inboxOrder.check.ts.

/** The order of "Needs you", most consequential first; items of one rank go by time left or, with
 *  none, by how long they have waited. */
export const RANK = {
/** one of ours silent or presumed lost */
  ours: 1,
  /** review items: presumed or flagged losses to confirm */
  review: 2,
  /** approvals, soonest expiry first */
  approval: 3,
  /** high and medium alerts, a group's that stops and asks included */
  alert: 4,
  /** for information, in this order */
  running: 5,
  note: 6,
  settled: 7,
  acknowledged: 8,
  lost: 9,
} as const

/** Where an item sits: in "Needs you" up to rank 4, in "For information" past it. */
export const NEEDS_UP_TO = RANK.alert

export interface Ranked {
  rank: number
  /** within a rank: smaller first (time left, or minus how long it has waited); none last */
  sort: number | null
}

export function isNeeds(r: Ranked): boolean {
  return r.rank <= NEEDS_UP_TO
}

/** Items in the order the drawer shows them: by rank, then by `sort` (none last), stable. */
export function inboxOrder<T extends Ranked>(items: readonly T[]): { needs: T[]; info: T[] } {
  const key = (x: T) => x.sort ?? Number.POSITIVE_INFINITY
  const sorted = items.map((x, k) => ({ x, k })).sort((a, b) => a.x.rank - b.x.rank || key(a.x) - key(b.x) || a.k - b.k).map((e) => e.x)
  return { needs: sorted.filter(isNeeds), info: sorted.filter((x) => !isNeeds(x)) }
}

/** An alert (a group's alert too, one card for its members): ours in trouble first (a lost link or one that cannot move), else by severity;
 *  an info alert asks nothing. */
const OURS_TROUBLE = new Set(['lost_link', 'not_moving'])

export function alertRank(kind: string, severity: string): number {
  if (severity === 'info') return RANK.note
  if (severity === 'high' && OURS_TROUBLE.has(kind)) return RANK.ours
  return RANK.alert
}

/** Secondary sort for alerts: high before medium, then the oldest first. */
export function alertSort(severity: string, sinceT: number): number {
  return (severity === 'high' ? 0 : severity === 'medium' ? 1 : 2) * 1e12 + sinceT
}
