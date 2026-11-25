// An agent's rationale as a card shows it. The text shows as a BLUF (bluf.ts `parseRationale`): the
// headline the agent was asked to lead with, its bullets, and the rest behind More; an older
// free-text rationale shows its first sentence with the rest behind More. Nothing is dropped.
import { Bluf } from './Bluf'
import { parseRationale } from './bluf'

export function RationaleText({ rationale }: { rationale: string; source?: { kind: string; id: string } | null }) {
  return <Bluf parts={parseRationale(rationale)} />
}
