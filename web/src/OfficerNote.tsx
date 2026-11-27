import { useEffect, useState } from 'react'
import type { TriageNote } from './decisions'
import { fmtWorldZ } from './scenarioClock'
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

/** One item the officer triaged (`GET /api/triage`). */
interface TriageRecord {
  item: string
  class: 'held' | 'alert'
  kind: string
  about?: { intent?: number; uids?: number[]; alert?: string; uid?: number; callsign?: string | null; detail?: string; rationale?: { text?: string } | null } | null
  entered_t: number
  stage: 'with_officer' | 'bypassed' | 'handled' | 'escalated' | 'turned_down' | 'untriaged'
  note?: string
  recommendation?: string | null
  why?: string
  intents?: number[]
  at_t?: number
  ended?: string
}

interface TriageState {
  officer: string | null
  counts: { on_watch: boolean; with_officer: number; handled: number }
  open: TriageRecord[]
  history: TriageRecord[]
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
  const [state, setState] = useState<TriageState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [filter, setFilter] = useState<Filter>('settled')
  useEffect(() => {
    let stop = false
    let timer: number | undefined
    const load = async () => {
      try {
        const res = await fetch('/api/triage', { cache: 'no-store' })
        if (!res.ok) throw new Error(`the bridge answered ${res.status}`)
        const v = (await res.json()) as TriageState
        if (!stop) {
          setState(v)
          setError(null)
        }
      } catch (e) {
        if (!stop) setError((e as Error).message)
      }
      if (!stop) timer = window.setTimeout(load, 3000)
    }
    void load()
    return () => {
      stop = true
      window.clearTimeout(timer)
    }
  }, [])
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
