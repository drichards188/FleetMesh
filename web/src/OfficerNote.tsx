import { useState } from 'react'
import type { TriageNote } from './decisions'
import { fmtWorldZ } from './scenarioClock'
import { type Desk, officerDesk, STALL_RUN, type TriageRecord } from './officerDesk'
import { useTriage } from './useTriage'
import { spanText } from './delivery'
import './officer.css'

/** The AI watch officer on an Inbox card (rules 7, docs/design/watch-officer.md): its
 *  recommendation, bottom line first, on an item it passed on to the person; or the mark of one
 *  that reached the person without its verdict. Nothing for an item that went straight to the
 *  person or with no officer on the run. */
export function OfficerNote({ triage }: { triage?: TriageNote | null }) {
  if (!triage) return null
  if (triage.stage === 'escalated') {
    const lines = (triage.recommendation ?? '').split('\n').map((l) => l.trim()).filter(Boolean)
    const [bottom, ...rest] = lines
    const bullets = rest.filter((l) => l.startsWith('- ')).map((l) => l.slice(2))
    return (
      <div className="of-note" role="note">
        <span className="of-k">AI watch officer recommends</span>
        {bottom && <p className="of-bottom">{bottom}</p>}
        {bullets.length > 0 && (
          <ul>
            {bullets.map((b, k) => (
              <li key={k}>{b}</li>
            ))}
          </ul>
        )}
        {triage.note && <p className="of-why">Its note: {triage.note}</p>}
      </div>
    )
  }
  if (triage.stage === 'untriaged') {
    const why = triage.why === 'officer_absent' ? 'the officer went quiet' : 'it did not decide it before its deadline'
    return (
      <div className="of-note untriaged" role="note">
        <span className="of-k">Untriaged</span>
        <p className="of-why">The AI watch officer held this first, but {why}: you have it as the engine raised it.</p>
      </div>
    )
  }
  return null
}

const STAGE_LABEL: Record<TriageRecord['stage'], string> = {
  with_officer: 'With the officer',
  bypassed: 'Straight to you',
  handled: 'Handled',
  escalated: 'Escalated',
  turned_down: 'Turned down',
  untriaged: 'Untriaged',
}

type Filter = 'settled' | 'escalated' | 'untriaged' | 'all'
const FILTERS: { id: Filter; label: string; stages: TriageRecord['stage'][] | null }[] = [
  { id: 'settled', label: 'Handled by the officer', stages: ['handled', 'turned_down'] },
  { id: 'escalated', label: 'Passed to you', stages: ['escalated'] },
  { id: 'untriaged', label: 'Untriaged', stages: ['untriaged'] },
  { id: 'all', label: 'All', stages: null },
]

function what(r: TriageRecord): string {
  const a = r.about ?? {}
  if (r.class === 'held') return `${r.kind} i${a.intent ?? r.item.replace('intent:', '')}${a.uids?.length ? ` on ${a.uids.join(', ')}` : ''}`
  return `${r.kind.replaceAll('_', ' ')}${a.callsign ? ` ${a.callsign}` : a.uid != null ? ` vehicle ${a.uid}` : ''}`
}

/** The Officer log: every Inbox item the AI watch officer triaged this run, for the person's own
 *  audit of what it settled without them (the maintainer's choice: nothing it handled is sampled
 *  into the Inbox). The audit log's `triage.*` events are the record. */
export function OfficerLog() {
  const { state, error } = useTriage()
  const [filter, setFilter] = useState<Filter>('settled')
  if (!state) return <p className="empty">{error ?? 'Reading the officer log'}</p>
  const all = [...state.open, ...state.history]
  const f = FILTERS.find((x) => x.id === filter) ?? FILTERS[0]
  const shown = f.stages ? all.filter((r) => f.stages?.includes(r.stage)) : all
  return (
    <div className="of-log">
      <p className="of-head">
        {state.officer ? `${state.officer} is ${state.counts.on_watch ? 'on watch' : 'off watch'}` : 'No watch officer in this run'}: {state.counts.handled} settled
        without you, {state.counts.with_officer} with it now.
      </p>
      <div className="of-filters seg" role="radiogroup" aria-label="Show">
        {FILTERS.map((x) => (
          <button key={x.id} role="radio" aria-checked={filter === x.id} className={filter === x.id ? 'on' : ''} onClick={() => setFilter(x.id)}>
            {x.label} ({x.stages ? all.filter((r) => x.stages?.includes(r.stage)).length : all.length})
          </button>
        ))}
      </div>
      {shown.length === 0 && <p className="empty">Nothing here yet</p>}
      {shown.map((r) => (
        <article key={`${r.item}@${r.entered_t}`} className="of-row">
          <header>
            <span className={`of-stage ${r.stage}`}>{STAGE_LABEL[r.stage]}</span>
            <span className="of-what">{what(r)}</span>
            <span className="of-when">{fmtWorldZ((r.at_t ?? r.entered_t) * 1000)}</span>
          </header>
          {r.note && <p>{r.note}</p>}
          {r.recommendation && <p className="of-detail">Recommended: {r.recommendation.split('\n')[0]}</p>}
          {r.intents && r.intents.length > 0 && <p className="of-detail">Orders: {r.intents.map((i) => `i${i}`).join(', ')}</p>}
          {r.about?.detail && <p className="of-detail">{r.about.detail}</p>}
          {r.ended && <p className="of-detail">Left the Inbox: {r.ended}</p>}
        </article>
      ))}
    </div>
  )
}

/** The AI watch officer at work, at the head of the person's Needs you (officerDesk.ts): on watch,
 *  what it is deciding now and the time it has left (it reaches the person then anyway), what it
 *  just did and why, and its counts for ten minutes; plainly, when its decisions are not landing.
 *  `full` while nothing needs the person (the desk is then what the Inbox has to say); otherwise
 *  its lines alone, under the cards' room. Read-only: the Officer log has every item. */
export function OfficerDesk({ now, nameOf, full, onLog }: { now: () => number; nameOf: (uid: number | string) => string; full: boolean; onLog: () => void }) {
  const { state } = useTriage()
  if (!state?.officer) return null
  const d: Desk = officerDesk(state, now(), nameOf)
  return (
    <section className={`of-desk${d.stalled ? ' stalled' : ''}${d.onWatch ? '' : ' off'}`} aria-label="The AI watch officer" aria-live="polite">
      <header>
        <span className={`of-live${d.onWatch ? ' on' : ''}`} aria-hidden="true" />
        <span className="of-k">AI watch officer</span>
        <span className="of-state">{d.onWatch ? 'on watch' : 'off watch'}</span>
        <button className="link of-open" onClick={onLog}>
          Officer log
        </button>
      </header>
      <p className="of-headline">{d.headline}</p>
      {d.stalled && (
        <p className="of-stall" role="alert">
          Its last {STALL_RUN} items reached you at their deadlines with no decision: its verdicts are not landing. Every item still reaches
          you; if this goes on, its own log says why (logs/officer.log).
        </p>
      )}
      {d.working.length > 0 && (
        <ul className="of-working" aria-label="Deciding now">
          {d.working.slice(0, full ? 4 : 2).map((w) => (
            <li key={w.item}>
              <span className="of-what">{w.what}</span>
              <span className="of-left" title="It reaches you then anyway">
                {spanText(w.leftS)}
              </span>
              <span className="of-clock" aria-hidden="true">
                <span style={{ transform: `scaleX(${w.left})` }} />
              </span>
            </li>
          ))}
          {d.working.length > (full ? 4 : 2) && <li className="of-more">and {d.working.length - (full ? 4 : 2)} more</li>}
        </ul>
      )}
      {full && d.recent.length > 0 && (
        <ul className="of-recent" aria-label="Just done">
          {d.recent.slice(0, 4).map((x) => (
            <li key={`${x.item}@${x.atMs}`}>
              <span className={`of-stage ${x.stage}`}>{x.verb}</span> <span className="of-what">{x.what}</span>
              {x.why && <p>{x.why}</p>}
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

