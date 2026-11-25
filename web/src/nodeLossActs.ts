// Rules 30: the person's acts on one of ours that went silent, as the bridge takes them
// (`POST /api/review/nodes/{uid}/...`, crates/fleetmesh-bridge/src/decision/nodes.rs). Each is the
// operator's own act under their session; nothing here decides, and no agent can make them.
import { operatorFetch, operatorHeaders } from './auth'
import { getOperator, StepUpNeeded } from './decisions'

async function post(path: string, body: Record<string, unknown>) {
  const res = await operatorFetch(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...operatorHeaders(getOperator()) },
    body: JSON.stringify(body),
  })
  const reply = await res.json().catch(() => ({}))
  if (!res.ok) {
    const msg = reply.detail ?? `bridge returned ${res.status}`
    throw reply.step_up ? new StepUpNeeded(msg) : new Error(msg)
  }
  return reply
}

/** Confirm one of ours lost, on the silence the review showed (`since`: its last report, the
 *  engine's ms, as the item names it), saying why where nothing presumes it lost. Only an
 *  authorised operator (403 otherwise); in a credentials run `proof` carries the step-up (the
 *  password again, or a passkey made for this act). Throws [`StepUpNeeded`] when the step-up is
 *  missing or wrong. */
export function confirmNodeLost(uid: number, since: number | null, note: string, proof: { password?: string; passkey?: Record<string, unknown> } = {}) {
  return post(`/api/review/nodes/${uid}/lost`, { since, note: note.trim(), ...proof })
}

/** Revoke the loss of one of ours (any operator, saying why: it only restricts). */
export function revokeNodeLoss(uid: number, note: string) {
  return post(`/api/review/nodes/${uid}/revoke`, { note: note.trim() })
}

/** Put a silent vehicle on the review list by hand, saying why (any operator; it decides nothing). */
export function flagNode(uid: number, note: string) {
  return post(`/api/review/nodes/${uid}/flag`, { note: note.trim() })
}

/** Take a hand flag off the review list (any operator). */
export function dismissNodeFlag(uid: number, note?: string) {
  return post(`/api/review/nodes/${uid}/dismiss`, { note: note?.trim() ?? '' })
}
