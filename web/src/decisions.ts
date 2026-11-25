import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { getAuth, operatorFetch, operatorHeaders, useAuth } from './auth'
import type { LngLat } from './types'
import type { GroupSpec, Share } from './groupOrders'
import type { ReviewNodeLoss, ReviewNodeLost } from './nodeLoss'

/** The decision layer as the bridge serves it: GET /api/inbox, GET /api/intents and the
 *  WebSocket `{"type":"decisions"}` message. Intent times are wall-clock milliseconds on the
 *  bridge's clock; alert `since_t` is wall-clock seconds. */

export type IntentState =
  | 'proposed'
  | 'checked'
  | 'auto_approved'
  | 'pending_approval'
  | 'approved'
  | 'rejected'
  | 'expired'
  | 'voided'
  | 'dispatched'
  | 'queued' // rules 5: approved, held for a vehicle the link can't carry it to now
  | 'done'
  | 'failed'

/** `hold`: a delivery check found something marginal that a person must judge (delivery-rules.md) */
export type CheckOutcome = 'pass' | 'fail' | 'unverified' | 'not_evaluated' | 'hold'

export interface Check {
  rule: string
  uid: string | null
  outcome: CheckOutcome
  detail: string
  /** a delivery rule's stable reason codes behind a hold or a refusal (`ENERGY_MARGIN_THIN`) */
  codes?: string[]
  /** rules 34, a group order's failing check of one member: what it makes of that member
   *  (waiting: it joins if it passes in time; excluded: no member order) */
  member?: 'waiting' | 'excluded'
}

export interface PolicyDecision {
  tier: 'auto' | 'human'
  rule: string
  reason: string
  facts: {
    kind: string
    source: 'human' | 'agent'
    leaves_approved_area: boolean
    area_detail: string
    unverified: string[]
  }
  /** distinct approvers the tier needs (two-person: 2); absent means 1 */
  approvers?: number
  /** the proposer may not approve it */
  proposer_excluded?: boolean
  /** rules 8: each approval re-enters the approver's password (a step-up; any tier the policy
   *  marks consequential) */
  step_up?: boolean
}

export interface Approval {
  by: string
  t: number
  self_approved: boolean
  /** the approval was the submission itself (an operator's own task in a human tier) */
  at_submission?: boolean
  note: string | null
  /** rules 8: the approver re-entered their password to sign */
  step_up?: boolean
}

/** A human-tier task the operator authorized by sending it (POST /api/tasks, 200). */
export interface Authorization {
  by: string
  at_submission: true
  tier: string
  reason: string
  why: string[]
}

/** "outside-area" -> "outside area", for "authorized by you (outside area)". */
export function tierLabel(tier: string) {
  return tier.replace(/[()]/g, '').replace(/-/g, ' ')
}

export type IntentParams = {
  center?: LngLat
  radius_m?: number
  route?: LngLat[]
  [k: string]: unknown
}

export interface Intent {
  id: number
  created_t: number
  /** rules 29: `handover`, the previous watch's order the fleet file handed over at the start (id: that file) */
  source: { kind: 'human' | 'agent' | 'handover'; id: string }
  kind: string
  params: IntentParams
  uids: string[]
  constraints: { reserve_pct?: number; duration_s?: number }
  rationale: string | null
  state: IntentState
  checks: Check[]
  policy: PolicyDecision | null
  approval: Approval | null
  expires_t: number | null
  history: { state: IntentState; t: number; reason: string }[]
  tasking_id?: number
  /** inbox only: why a human is needed, as plain lines */
  why?: string[]
  /** signatures before the last one, when the tier needs more than one approver */
  concurrences?: Approval[]
  /** rules 5: held for an unreachable vehicle (or once was): since, until, why, and the last
   *  re-check's verdict when its vehicles could be reached again */
  queue?: QueueInfo
  /** rules 29: the previous watch's order, handed over at the start by the fleet file */
  handed_over?: { fleet: string; sha256: string }
  /** rules 5: carried over from the previous run after a bridge restart */
  resumed?: { from: { log: string; head: string; seq: number; was: IntentState }; ops?: Record<string, string> }
  /** rules 5: the vehicles that ever started it (followed, never sent it again) */
  committed?: string[]
  /** rules 5: dispatched before (a vehicle may hold it: its next delivery asks first) */
  dispatched_before?: boolean
  /** rules 5: resumed under way, waiting for its vehicles to connect before it is followed */
  follow_pending?: boolean
  /** queued intents in the inbox message: each vehicle's reach on the world now */
  reach_now?: Reach[]
  /** rules 34: a group order's spec (its spread, minimum, whether it re-forms) */
  group?: GroupSpec
  /** rules 34: a member order's group order, and its share of it */
  parent?: number
  share?: Share
}

/** Whether one vehicle can be reached for an order now, and why. */
export interface Reach {
  uid: string
  reachable: boolean
  detail: string
}

export interface QueueInfo {
  t: number
  expires_t: number
  /** at the deadline it goes back to the inbox (a human's approval is held only so long) */
  inbox?: boolean
  reach: Reach[]
  recheck?: { t: number; verdict: 'dispatch' | 'inbox' | 'void'; changes: string[] }
}

/** `info` asks nothing of the operator: never counted as a warning. */
export type Severity = 'high' | 'medium' | 'info'

export interface Alert {
  id: string // link:<uid>, op:<op id>, restore:<uid> or route:<op id>; stable while the condition lasts
  kind:
    | 'lost_link'
    | 'not_moving'
    | 'left_task'
    | 'task_failed'
    | 'ack_timeout'
    | 'may_be_running'
    | 'restore_pending'
    | 'path_crosses_land'
    // rules 30: one of ours confirmed lost (information), and one reported again after it
    | 'node_lost'
    | 'node_reported_again'
    // the navigation monitor proposed a safe order for a vehicle whose navigation is untrusted
    | 'nav_monitor'
  severity: Severity
  uid: number
  callsign: string | null
  detail: string
  since_t: number
  op_id?: number
  intent_id?: number
  /** folded into this swarm alert (`Inbox.swarm_alerts`): the Inbox shows it inside that card only */
  folded_into?: string
}

export interface Inbox {
  pending: Intent[]
  alerts: Alert[]
  /** rules 5: approved orders held for vehicles the link can't carry them to now */
  queued?: Intent[]
  /** rules 30: the review's loss items (presumed or flagged, and confirmed); absent before rules 30 */
  review?: { node_losses?: ReviewNodeLoss[]; nodes_lost?: ReviewNodeLost[] }
  counts: { pending: number; alerts: number; queued?: number; review?: number; /** decisions a person made in the last ten minutes (the bridge counts them) */ decided_10m?: number }
}

export interface Decisions {
  inbox: Inbox
  intents: Intent[] // newest first, the latest 50
  /** the operators the bridge accepts, and those with authority */
  operators?: { operators: string[]; consequential: string[] }
}

/** POST /api/intents as the console's operator. 201 with the intent; a refused or waiting
 *  intent is still a 201 (its state says so). Throws an Error with the bridge's reason. */
export async function submitIntent(body: Record<string, unknown>): Promise<{ intent: Intent; dispatched: boolean }> {
  const res = await operatorFetch('/api/intents', {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...operatorHeaders(getOperator()) },
    body: JSON.stringify(body),
  })
  const b = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(b.detail ?? `bridge returned ${res.status}`)
  return b
}

export const ALERT_LABEL: Record<Alert['kind'], string> = {
  lost_link: 'LOST LINK',
  not_moving: 'NOT MOVING',
  left_task: 'LEFT TASK',
  task_failed: 'TASK FAILED',
  ack_timeout: 'NO ACK',
  may_be_running: 'MAY BE RUNNING',
  restore_pending: 'FAILSAFE NOT RESTORED',
  path_crosses_land: 'PATH CROSSES LAND',
  node_lost: 'CONFIRMED LOST',
  node_reported_again: 'LOST, REPORTING AGAIN',
  nav_monitor: 'NAVIGATION MONITOR ACTED',
}

// ---- operator ------------------------------------------------------------------------------
// A credentials run: the operator signed in on this browser (./auth). A dev run (the
// default; FLEETMESH_AUTH unset): the operator the "Acting as" switcher names, sent as a header.

const OPERATORS_ENV = (import.meta.env.VITE_FLEETMESH_OPERATORS as string | undefined) ?? 'op-1'
/** Dev run: the ids the switcher offers until the bridge sends its own list (the decisions
 *  message carries it), from VITE_FLEETMESH_OPERATORS at build or dev-server start. */
export const OPERATORS = OPERATORS_ENV.split(',')
  .map((s) => s.trim())
  .filter(Boolean)

/** The bridge's own list once it sends one (the decisions message carries it); until then
 *  VITE_FLEETMESH_OPERATORS. */
let bridgeOperators: { operators: string[]; consequential: string[] } | null = null
const bridgeOperatorListeners = new Set<() => void>()

export function setBridgeOperators(ops: { operators: string[]; consequential: string[] } | undefined) {
  if (!ops) return
  const same = bridgeOperators && bridgeOperators.operators.join() === ops.operators.join() && bridgeOperators.consequential.join() === ops.consequential.join()
  if (same) return
  bridgeOperators = ops
  bridgeOperatorListeners.forEach((l) => l())
}

/** Every operator the console may act as, and who of them has authority. */
export function useOperators(): { operators: string[]; consequential: string[] } {
  return useSyncExternalStore(
    (cb) => {
      bridgeOperatorListeners.add(cb)
      return () => bridgeOperatorListeners.delete(cb)
    },
    () => bridgeOperators ?? FALLBACK_OPERATORS,
  )
}

const FALLBACK_OPERATORS = { operators: OPERATORS, consequential: [] as string[] }

const OPERATOR_KEY = 'fleetmesh.operator'
let operator = (() => {
  try {
    const saved = localStorage.getItem(OPERATOR_KEY)
    // checked against the bridge's list when it arrives (setOperator keeps any listed id)
    if (saved) return saved
  } catch {
    // blocked storage: the first listed operator
  }
  return OPERATORS[0] ?? 'op-1'
})()
const operatorListeners = new Set<() => void>()

/** Who this console acts for: the signed-in operator (credentials run), or the switcher's
 *  pick (dev run). */
export function getOperator() {
  const a = getAuth()
  return a.auth === 'credentials' ? (a.operator ?? '') : operator
}

export function setOperator(id: string) {
  operator = id
  try {
    localStorage.setItem(OPERATOR_KEY, id)
  } catch {
    // not persisted; still applies for this session
  }
  operatorListeners.forEach((l) => l())
}

export function useOperator(): [string, (id: string) => void] {
  const auth = useAuth()
  const op = useSyncExternalStore(
    (cb) => {
      operatorListeners.add(cb)
      return () => operatorListeners.delete(cb)
    },
    () => operator,
  )
  return [auth.auth === 'credentials' ? (auth.operator ?? '') : op, setOperator]
}

// ---- actions ------------------------------------------------------------------------------

export interface ActionResult {
  intent: Intent
  dispatched: boolean
}

/** What an approval adds beyond the note. */
export interface ApproveExtra {
  /** rules 8: the approver's password again, for a consequential signature (a step-up) */
  password?: string
  /** rules 10: or their passkey's assertion, made for this signature */
  passkey?: Record<string, unknown>
}

/** The bridge refused a signature for want of the signer's password (a step-up): absent or
 *  wrong. The card asks for it. */
export class StepUpNeeded extends Error {}

/** approve | reject | cancel. Throws an Error with the bridge's reason ([`StepUpNeeded`] when
 *  the password is missing or wrong). */
export async function intentAction(id: number, action: 'approve' | 'reject' | 'cancel', note: string, extra?: ApproveExtra): Promise<ActionResult> {
  const body: { note?: string } & ApproveExtra = { ...(extra ?? {}) }
  if (note.trim()) body.note = note.trim()
  const res = await operatorFetch(`/api/intents/${id}/${action}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...operatorHeaders(getOperator()) },
    body: JSON.stringify(body),
  })
  const reply = await res.json().catch(() => ({}))
  if (!res.ok) {
    const msg = reply.detail ?? `bridge returned ${res.status}`
    throw reply.step_up ? new StepUpNeeded(msg) : new Error(msg)
  }
  return reply as ActionResult
}

/** This run signs consequential approvals with the password again (credentials run). */
export function stepsUp(): boolean {
  return getAuth().auth === 'credentials'
}

// ---- time ---------------------------------------------------------------------------------

/** Seconds since the bridge's wall clock, for expiry countdowns: the browser's clock can
 *  differ from the bridge's, so the offset is taken from the bridge's own message times. */
export function useBridgeClock(bridgeT: number | undefined) {
  const offset = useRef(0)
  const [, tick] = useState(0)
  useEffect(() => {
    if (bridgeT != null) offset.current = bridgeT * 1000 - Date.now()
  }, [bridgeT])
  useEffect(() => {
    const id = window.setInterval(() => tick((n) => n + 1), 500)
    return () => window.clearInterval(id)
  }, [])
  return useCallback(() => Date.now() + offset.current, [])
}

export function fmtDuration(s: number) {
  const n = Math.max(0, Math.round(s))
  if (n < 60) return `${n}s`
  if (n < 3600) return `${Math.floor(n / 60)}m${String(n % 60).padStart(2, '0')}s`
  return `${Math.floor(n / 3600)}h${String(Math.floor((n % 3600) / 60)).padStart(2, '0')}m`
}

/** What happened to an intent and how fast, for the Decisions list. */
export interface Verdict {
  route: 'auto' | 'human' | 'rules'
  decided: IntentState | null // approved | rejected | expired | voided, when a human was asked
  wait_s: number | null // pending_approval to the decision
}

const HUMAN_ENDS: IntentState[] = ['approved', 'rejected', 'expired', 'voided']

export function verdict(i: Intent): Verdict {
  const pend = i.history.find((h) => h.state === 'pending_approval')
  if (pend) {
    const end = i.history.find((h) => h.t >= pend.t && HUMAN_ENDS.includes(h.state))
    return { route: 'human', decided: end?.state ?? null, wait_s: end ? (end.t - pend.t) / 1000 : null }
  }
  if (i.history.some((h) => h.state === 'auto_approved')) return { route: 'auto', decided: null, wait_s: null }
  return { route: 'rules', decided: null, wait_s: null }
}

// ---- alert acknowledge (client-side) ---------------------------------------------------------

/** Acknowledged alerts live only in this browser's localStorage: they are not shared between
 *  operators or consoles and the bridge doesn't know about them. An acknowledgement holds for
 *  one occurrence (the alert id plus when it started); the same condition starting again later
 *  is a new alert. */
interface AlertMemory {
  acked: Record<string, number> // alert id -> since_t acknowledged
}

const ALERT_KEY = 'fleetmesh.alerts'

function loadMemory(): AlertMemory {
  try {
    const raw = localStorage.getItem(ALERT_KEY)
    if (raw) {
      const m = JSON.parse(raw)
      return { acked: m.acked ?? {} }
    }
  } catch {
    // blocked or corrupt storage: start empty
  }
  return { acked: {} }
}

export function useAlertMemory(live: Alert[]) {
  const [mem, setMem] = useState<AlertMemory>(loadMemory)
  const save = (m: AlertMemory) => {
    setMem(m)
    try {
      localStorage.setItem(ALERT_KEY, JSON.stringify(m))
    } catch {
      // not persisted; still applies for this session
    }
  }
  // forget entries for conditions that have cleared, so storage stays small
  useEffect(() => {
    const ids = new Set(live.map((a) => a.id))
    const acked = Object.fromEntries(Object.entries(mem.acked).filter(([id]) => ids.has(id)))
    if (Object.keys(acked).length !== Object.keys(mem.acked).length) save({ acked })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [live])
  return {
    isAcked: (a: Alert) => mem.acked[a.id] === a.since_t,
    ack: (a: Alert) => save({ acked: { ...mem.acked, [a.id]: a.since_t } }),
    wake: (a: Alert) => {
      const { [a.id]: _a, ...acked } = mem.acked
      save({ acked })
    },
  }
}

/** Alert ids that are acknowledged here (for the badge and the panel). */
export function useHiddenAlerts(decisions: Decisions | null) {
  const mem = useAlertMemory(decisions?.inbox.alerts ?? [])
  const hidden = new Set<string>()
  for (const a of decisions?.inbox.alerts ?? []) if (mem.isAcked(a)) hidden.add(a.id)
  return { mem, hidden }
}
