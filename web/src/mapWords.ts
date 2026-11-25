// The map's labels in a person's words (ledger 1 of the console cutdown, bd murmur-iobd): what a
// label says, never a raw id where a name exists, nothing ticking that no decision reads. Display
// only. Pure, no imports: mapWords.check.ts runs it under node.

/** "47 s", "4 min", "2 h 05 min": how long, in words for a label. */
export function spanWords(s: number): string {
  const n = Math.max(0, Math.round(s))
  if (n < 90) return `${n} s`
  const m = Math.floor(n / 60)
  return m < 120 ? `${m} min` : `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')} min`
}

// ---- our silent vehicles' estimates ----------------------------------------------------------

/** What a silent vehicle is probably doing, by the bridge's motion model (`Estimate.basis`). */
export const PROBABLY: Record<string, string> = {
  orbit: 'on its orbit',
  patrol: 'on its patrol route',
  hold: 'holding',
  rtl: 'returning home',
  velocity: 'on its last course',
  stationary: 'where it was',
}

/** What its lost-link failsafe does once its timer has fired (`FailsafeBranch.action`). */
export const FAILSAFE_DOES: Record<string, string> = {
  rtl: 'returning (lost-link RTL)',
  disarm: 'disarmed (lost-link failsafe)',
}

/** A silent vehicle's estimate label. The leading hypothesis says what it is probably doing and how
 *  long it has been silent ("probably returning (lost-link RTL); silent 47 s"); the task model,
 *  once the failsafe leads, what it would be doing had it kept its task. */
export function estimateWords(h: { failsafe: string } | { basis: string; second: boolean }, silentS: number): string {
  if ('failsafe' in h) return `probably ${FAILSAFE_DOES[h.failsafe] ?? h.failsafe}; silent ${spanWords(silentS)}`
  const doing = PROBABLY[h.basis] ?? h.basis
  return h.second ? `if it kept its task: ${doing}` : `probably ${doing}; silent ${spanWords(silentS)}`
}

// ---- orders ----------------------------------------------------------------------------------

/** An order held in the queue: "queued orbit: link down, 2 min left". */
export function queuedWords(kind: string, why: string, leftS: number | null): string {
  return `queued ${kind}: ${why}${leftS == null ? '' : `, ${spanWords(leftS)} left`}`
}

/** An order carried over a restart, waiting for its vehicle: "resumed patrol: waiting for the
 *  vehicle". */
export function resumedWords(kind: string): string {
  return `resumed ${kind}: waiting for the vehicle`
}

/** An order whose start went out unconfirmed and is no longer in force: a person decides.
 *  "MAY BE RUNNING orbit: its start went out unconfirmed (voided)". */
export function mayBeRunningWords(kind: string, state: string): string {
  return `MAY BE RUNNING ${kind}: its start went out unconfirmed (${state.replace(/_/g, ' ')})`
}
