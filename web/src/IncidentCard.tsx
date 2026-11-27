import { useState, type ReactNode } from 'react'
import { type HubWind, judgeWind } from './hubSummary'
import { DEFAULT_LIMITS, type Incident, incidentCount, incidentStand, incidentTitle, liftWords, PLAY_CODES, type Play, type PlayAsk, type PlayRec, playWords } from './playWords'
import { adoptPlay, liftPlay } from './plays'
import './incident.css'

interface Props {
  incident: Incident
  hubName: string
  /** the wind at its hub against the limits of what is based there */
  weather: HubWind | null
  /** the plays in force at its hub */
  plays: Play[]
  /** the watch officer's recommendation for it */
  rec: PlayRec | null
  /** its items' cards, as the Inbox would show each */
  members: ReactNode[]
  /** the other items at its hub, of another cause, in words */
  others: string[]
  onDone: (text: string, tone: 'ok' | 'bad') => void
}

/** One card for the items of one cause at one hub (docs/design/incidents.md): what the pattern
 *  is, what it is doing to the hub, the watch officer's recommendation, and one decision for it:
 *  a play the person adopts (a ground stop until the wind drops, or limits within which orders
 *  go), or each item decided as before. What is of another cause at the hub is said, never folded
 *  in. A play in force says how its end stands and can be lifted. */
export function IncidentCard({ incident, hubName, weather, plays, rec, members, others, onDone }: Props) {
  const [open, setOpen] = useState(false)
  const [limitsOpen, setLimitsOpen] = useState(false)
  const [limits, setLimits] = useState(DEFAULT_LIMITS)
  const [busy, setBusy] = useState(false)
  const w = judgeWind(weather)
  const codes = incident.codes.filter((c) => PLAY_CODES.includes(c))
  const act = async (what: string, f: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await f()
      onDone(what, 'ok')
    } catch (e) {
      onDone(`${what} refused: ${(e as Error).message}`, 'bad')
    } finally {
      setBusy(false)
    }
  }
  const adopt = (ask: PlayAsk, what: string) => act(what, () => adoptPlay(ask))
  const windWords =
    w && weather?.gust_mps != null
      ? w.band === 'over'
        ? `Gusts ${weather.gust_mps.toFixed(1)} m/s, over the ${w.who}' ${w.hard} m/s limit: nothing leaves.`
        : w.band === 'marginal'
          ? `Gusts ${weather.gust_mps.toFixed(1)} m/s, over the ${w.who}' ${w.soft} m/s soft limit: every departure is held or refused.`
          : `Gusts ${weather.gust_mps.toFixed(1)} m/s, under the ${w.who}' ${w.soft} m/s limit.`
      : null
  return (
    <article className="incident" aria-label={incidentTitle(incident, hubName)}>
      <header>
        <span className="inc-k">Pattern</span>
        <h3>{incidentTitle(incident, hubName)}</h3>
        <span className="inc-n">{incidentCount(incident)}</span>
      </header>
      {incidentStand(incident) && <p className="inc-stand">{incidentStand(incident)}</p>}
      {windWords && <p className={`inc-wind ${w?.band ?? ''}`}>{windWords}</p>}

      {plays.map((p) => (
        <PlayInForce key={p.id} play={p} onDone={onDone} />
      ))}

      {rec && plays.length === 0 && (
        <div className="inc-rec">
          <span className="inc-k">AI watch officer recommends</span>
          <p>
            <b>{playWords(rec)}</b>
          </p>
          <Why text={rec.why} />
        </div>
      )}

      {plays.length === 0 && (
        <div className="inc-acts">
          <p className="inc-ask">Answer the pattern once:</p>
          <button className="inc-play-btn" disabled={busy} onClick={() => adopt({ kind: 'ground_stop', hub: incident.hub, lift: weather ? 'wind' : undefined, except_priority: 3 }, 'Ground stop adopted')}>
            Ground stop{weather ? ' until the wind drops' : ''}
            <small>No departure without you; urgent parcels still ask.</small>
          </button>
          {codes.length > 0 && (
            <button className="inc-play-btn" disabled={busy} aria-expanded={limitsOpen} onClick={() => setLimitsOpen((o) => !o)}>
              Fly within limits…
              <small>Short, light, well-charged trips go; the rest wait.</small>
            </button>
          )}
          {limitsOpen && (
            <form
              className="inc-limits"
              onSubmit={(e) => {
                e.preventDefault()
                void adopt({ kind: 'limits', hub: incident.hub, codes, limits, lift: weather ? 'wind' : undefined, except_priority: 3 }, 'Limits adopted')
              }}
            >
              <label>
                Trip up to <input type="number" min={100} step={100} value={limits.max_trip_m} onChange={(e) => setLimits({ ...limits, max_trip_m: Number(e.target.value) })} /> m
              </label>
              <label>
                Parcel up to <input type="number" min={0.1} step={0.1} value={limits.max_mass_kg} onChange={(e) => setLimits({ ...limits, max_mass_kg: Number(e.target.value) })} /> kg
              </label>
              <label>
                Battery at least <input type="number" min={0} max={100} step={5} value={limits.min_battery_pct} onChange={(e) => setLimits({ ...limits, min_battery_pct: Number(e.target.value) })} /> %
              </label>
              <button className="inc-play-btn primary" disabled={busy} type="submit">
                Adopt these limits
              </button>
            </form>
          )}
        </div>
      )}

      {others.length > 0 && (
        <p className="inc-others">
          Not part of this: {others.join('; ')}. {others.length === 1 ? 'It is' : 'They are'} below, on {others.length === 1 ? 'its' : 'their'} own.
        </p>
      )}

      {members.length > 0 && (
        <>
          <button className="link inc-each" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
            {open ? 'Hide the items' : `Decide each of your ${members.length} instead`}
          </button>
          {open && <div className="inc-members">{members}</div>}
        </>
      )}
    </article>
  )
}

/** A play in force (rules 8): what it does, who adopted it, how its end stands, and the way to lift
 *  it. In its incident's card, or on its own at the head of the Inbox once its items have gone. */
export function PlayInForce({ play: p, hubName, onDone }: { play: Play; hubName?: string; onDone: (text: string, tone: 'ok' | 'bad') => void }) {
  const [busy, setBusy] = useState(false)
  const lift = async () => {
    setBusy(true)
    try {
      await liftPlay(p.id)
      onDone(`Play ${p.id} lifted`, 'ok')
    } catch (e) {
      onDone(`Lifting play ${p.id} refused: ${(e as Error).message}`, 'bad')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="inc-play" role="status">
      <p>
        <b>{playWords(p)}</b>
        {hubName ? ` at ${hubName}` : ''}, play {p.id} by {p.by}
      </p>
      <p className="muted">{liftWords(p)}. Urgent parcels still come to you.</p>
      <button className="ghost mini" disabled={busy} onClick={() => void lift()}>
        Lift it now
      </button>
    </div>
  )
}

/** The officer's why: its first line, then its bullets as a list (it writes "- " lines). */
function Why({ text }: { text: string }) {
  const lines = text.split('\n').map((l) => l.trim()).filter(Boolean)
  const [first, ...rest] = lines
  const bullets = rest.filter((l) => l.startsWith('- ')).map((l) => l.slice(2))
  const plain = rest.filter((l) => !l.startsWith('- '))
  return (
    <>
      {first && <p className="muted">{first}</p>}
      {bullets.length > 0 && (
        <ul className="inc-why">
          {bullets.map((b, k) => (
            <li key={k}>{b}</li>
          ))}
        </ul>
      )}
      {plain.map((l, k) => (
        <p key={k} className="muted">
          {l}
        </p>
      ))}
    </>
  )
}

