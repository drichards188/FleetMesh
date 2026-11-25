// What in the Inbox needs the person, counted once (the Inbox button and its Needs you tab show the
// same number, and their titles say what it counts). Display only: nothing here resolves or hides
// anything in the bridge or the engine. Pure: needsYou.check.ts.
import type { Alert } from './decisions'

interface InboxLike {
  pending: readonly unknown[]
  alerts: readonly Alert[]
  review?: {
    node_losses?: readonly unknown[]
  }
}

export interface NeedsYou {
  /** everything below that needs the person, each item once */
  total: number
  /** approvals waiting for a signature */
  approvals: number
  /** review items: presumed losses of ours to confirm */
  review: number
  high: number
  medium: number
  /** not counted: informational alerts, alerts acknowledged in this browser */
  info: number
  muted: number
  /** what the total counts, and what it leaves out, in a sentence for a title */
  says: string
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

/** The one count of what needs the person. `hidden`: alerts acknowledged here. An alert folded
 *  into a swarm's counts once, as the swarm's. */
export function needsYou(inbox: InboxLike | null | undefined, hidden: ReadonlySet<string>): NeedsYou {
  const approvals = inbox?.pending.length ?? 0
  const review = inbox?.review?.node_losses?.length ?? 0
  const all = (inbox?.alerts ?? []).filter((a) => !a.folded_into)
  const live = all.filter((a) => !hidden.has(a.id))
  const sev = [...live.map((a) => a.severity)]
  const high = sev.filter((s) => s === 'high').length
  const medium = sev.filter((s) => s === 'medium').length
  const info = sev.length - high - medium
  const muted = all.length - live.length
  const total = approvals + review + high + medium
  const counted = [
    approvals ? plural(approvals, 'approval') : '',
    review ? `${plural(review, 'item')} for review` : '',
    high ? plural(high, 'high alert') : '',
    medium ? plural(medium, 'medium alert') : '',
  ].filter(Boolean)
  const left = [info ? `${plural(info, 'alert')} for information` : '', muted ? `${plural(muted, 'alert')} acknowledged here` : ''].filter(Boolean)
  const says = `${total ? `${total} need${total === 1 ? 's' : ''} you: ${counted.join(', ')}` : 'Nothing needs you'}${left.length ? `. Not counted: ${left.join(', ')}` : ''}.`
  return { total, approvals, review, high, medium, info, muted, says }
}
