import { loadTone, loadWords, operatorLoad, spanText } from './delivery'
import type { Decisions } from './decisions'
import './delivery.css'

/** The operator's load in the top bar (docs/design/attention-cost.md 4.1): items waiting for a
 *  person, how long the oldest has waited, and the decisions made in the last ten minutes. Read from
 *  what the bridge already sends; display only, and quiet when nothing waits. */
export function OperatorLoad({ decisions, now }: { decisions: Decisions | null; now: () => number }) {
  if (!decisions) return null
  const l = operatorLoad(decisions.inbox.pending, now(), decisions.inbox.counts.decided_10m, decisions.intents)
  const tone = loadTone(l)
  return (
    <span className={`load ${tone}`} role="status" aria-label={`Your load: ${loadWords(l)}`} title="Your load: items waiting for you, how long the oldest has waited, and the decisions made in the last 10 minutes (docs/design/attention-cost.md)">
      <span>
        <b className="w">{l.waiting}</b> waiting
      </span>
      <span>
        oldest <b>{l.oldestS == null ? 'none' : spanText(l.oldestS)}</b>
      </span>
      <span>
        <b>{l.decided}</b> decided / 10 min
      </span>
    </span>
  )
}

/** The Parcels button (a dock drawer), with the count of parcels waiting or late. */
export function ParcelsButton({ open, onClick, late, total }: { open: boolean; onClick: () => void; late: number; total: number }) {
  return (
    <button className={`ghost platforms-btn ix-btn ${open ? 'on' : ''}`} aria-pressed={open} onClick={onClick} title="Parcels: each parcel of a delivery run, its state, hub, destination, weight, due time and vehicle">
      Parcels
      {total > 0 && <span className="ix-badge" style={late > 0 ? { background: 'var(--bad)', color: '#fff' } : undefined}>{late > 0 ? late : total}</span>}
    </button>
  )
}
