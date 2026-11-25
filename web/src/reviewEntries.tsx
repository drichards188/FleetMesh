import type { ReactNode } from 'react'
import { RANK } from './inboxOrder'
import { NodeLossItem, NodeLostItem, type Props } from './ReviewSection'

/** Review items as Inbox entries (ledger 2 section 4; ReviewSection.tsx holds the cards). */
/** One Inbox item: the card, where it sorts (inboxOrder.ts) and by what within its rank. */
export interface ReviewEntry {
  key: string
  rank: number
  sort: number | null
  node: ReactNode
}

/** Every review item as an Inbox entry, in the order the engine sent them: one of ours presumed
 *  lost ranks with ours in trouble, a silent one flagged by hand is review; a loss already
 *  confirmed goes under For information. */
export function reviewEntries({ review, now, onDone, focus, nameOf }: Props, _operator: string): ReviewEntry[] {
  const out: ReviewEntry[] = []
  const focusN = (id: string) => (focus && focus.id === id ? focus.n : undefined)
  for (const n of review?.node_losses ?? []) {
    out.push({
      key: n.id,
      rank: n.state === 'presumed_lost' ? RANK.ours : RANK.review,
      sort: n.silent_s != null ? -n.silent_s : null,
      node: <NodeLossItem key={n.id} n={n} now={now} onDone={onDone} focusN={focusN(n.id)} nameOf={nameOf} />,
    })
  }
  for (const n of review?.nodes_lost ?? []) {
    out.push({ key: n.id, rank: RANK.settled, sort: null, node: <NodeLostItem key={n.id} n={n} now={now} onDone={onDone} focusN={focusN(n.id)} /> })
  }
  return out
}
