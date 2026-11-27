import { useEffect, useState } from 'react'
import { PlayInForce } from './IncidentCard'
import type { Play } from './playWords'
import { FloatGrip } from './FloatingPanel'
import { useFloatBar } from './floating'
import type { FleetVehicle } from './fleetModel'
import { type HubDetails as Details, type HubTone, type HubWind, stateWords } from './hubSummary'
import { spanText } from './delivery'
import './hub.css'

interface Props {
  d: Details
  /** the wind there, as the bridge sends it (the gauge's figures) */
  weather: HubWind | null
  closed: boolean
  /** its own vehicles that need the person: needing attention, or silent */
  needing: readonly FleetVehicle[]
  nameOf: (uid: string) => string
  /** its vehicles are drawn one by one on the map */
  open: boolean
  /** none parked to gather: the map draws each already */
  canOpen: boolean
  onToggleOpen: () => void
  onVehicle: (uid: number) => void
  /** the Parcels list at this hub */
  onParcels: () => void
  /** what the person can do now for a vehicle that needs them: its card in the Inbox (an order
   *  to decide, an alert), or nothing yet (its details are a click on its name) */
  actFor: (uid: number) => { label: string; run: () => void } | null
  /** rules 8: the plays in force at it */
  plays: Play[]
  onClose: () => void
}

const NEED_SHOWN = 5

/** The house of the map's mark, in the hub's tone. */
export function HouseGlyph({ tone, size = 26 }: { tone: HubTone; size?: number }) {
  return (
    <svg className={`hd-house ${tone}`} width={size} height={(size * 36) / 32} viewBox="0 0 32 36" aria-hidden="true">
      <path className="walls" d="M3 14 L16 3 L29 14 L29 34 L3 34 Z" />
      <path className="roof" d="M1 15.5 L16 2 L31 15.5 Z" />
    </svg>
  )
}

/** The wind against the limits: a scale from calm past the hard limit, the bands it holds and
 *  refuses in, and where the gust and the mean wind are now. */
function WindGauge({ w }: { w: HubWind }) {
  const soft = w.gust_soft_mps
  const hard = w.gust_hard_mps
  const gust = w.gust_mps ?? 0
  const top = Math.max((hard ?? soft ?? gust) * 1.25, gust * 1.08, 1)
  const at = (v: number) => `${Math.min(100, (v / top) * 100)}%`
  return (
    <div className="hd-gauge" role="img" aria-label={`Gusts ${gust.toFixed(1)} m/s; held from ${soft ?? '–'}, refused from ${hard ?? '–'}`}>
      <div className="hd-gauge-track">
        {soft != null && <span className="band warn" style={{ left: at(soft), width: `calc(${at(hard ?? top)} - ${at(soft)})` }} />}
        {hard != null && <span className="band bad" style={{ left: at(hard), right: 0 }} />}
        {w.wind_mps != null && <span className="mark wind" style={{ left: at(w.wind_mps) }} title={`mean wind ${w.wind_mps.toFixed(1)} m/s`} />}
        <span className="mark gust" style={{ left: at(gust) }} title={`gusts ${gust.toFixed(1)} m/s`} />
      </div>
      <div className="hd-gauge-ticks">
        {soft != null && <span style={{ left: at(soft) }}>{soft}</span>}
        {hard != null && <span style={{ left: at(hard) }}>{hard}</span>}
      </div>
    </div>
  )
}

/** A hub, one click from its mark on the map: a few plain sentences first (whether it can fly, what
 *  needs you, what its vehicles and parcels are doing), then the figures behind them. Read-only:
 *  its links show vehicles, the parcels list or its vehicles on the map, and send nothing. */
export function HubDetails({ d, weather, closed, needing, nameOf, open, canOpen, onToggleOpen, onVehicle, onParcels, actFor, plays, onClose }: Props) {
  const [said, setSaid] = useState<{ text: string; tone: 'ok' | 'bad' } | null>(null)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  const bar = useFloatBar()
  const total = d.fleet.reduce((s, x) => s + x.n, 0)
  const state = stateWords(d, closed)
  // the first sentence (whether it can fly) in the colour of the answer
  const lead = closed ? 'closed' : d.wind?.band === 'over' ? 'bad' : d.wind?.band === 'marginal' ? 'warn' : undefined
  const p = d.parcels
  return (
    <aside className="vdetails hdetails" aria-label={`Details for ${d.name}`}>
      <header className="vd-head" {...bar}>
        <FloatGrip />
        <HouseGlyph tone={d.tone} />
        <div className="vd-title">
          <div className="hd-name">{d.name}</div>
          <span className={`hd-state ${state.tone}`}>{state.words}</span>
        </div>
        <button className="ghost mini" onClick={onClose} aria-label="Close hub details">
          ✕
        </button>
      </header>
      <div className="vd-body">
        <div className="hd-summary">
          {d.summary.map((s, i) => (
            <p key={i} className={i === 0 ? lead : i === 1 && d.needYou ? 'bad' : undefined}>
              {s}
            </p>
          ))}
        </div>

        {plays.length > 0 && (
          <section>
            <h4>Play in force</h4>
            {plays.map((p) => (
              <PlayInForce key={p.id} play={p} onDone={(text, tone) => setSaid({ text, tone })} />
            ))}
            {said && <p className={said.tone === 'bad' ? 'bad' : 'muted'}>{said.text}</p>}
          </section>
        )}

        {needing.length > 0 && (
          <section>
            <h4>Needs you</h4>
            <ul className="hd-need">
              {needing.slice(0, NEED_SHOWN).map((v) => {
                const act = actFor(v.uid)
                return (
                  <li key={v.uid}>
                    <button className="link" onClick={() => onVehicle(v.uid)} title="Its details">
                      {nameOf(String(v.uid))}
                    </button>
                    <span className="muted" title={v.why}>
                      {v.why || (v.state === 'offline' ? 'no telemetry' : 'needs attention')}
                    </span>
                    {act && (
                      <button className="ghost mini hd-do" onClick={act.run}>
                        {act.label}
                      </button>
                    )}
                  </li>
                )
              })}
            </ul>
            {needing.length > NEED_SHOWN && <p className="muted">and {needing.length - NEED_SHOWN} more in the Fleet list</p>}
          </section>
        )}

        {weather?.gust_mps != null && (weather.gust_soft_mps != null || weather.gust_hard_mps != null) && (
          <section>
            <h4>Wind at the hub</h4>
            <WindGauge w={weather} />
            <p className="muted">
              Gusts <b className="num">{weather.gust_mps.toFixed(1)}</b> m/s{weather.wind_mps != null && <>, mean wind <b className="num">{weather.wind_mps.toFixed(1)}</b> m/s</>}. The {d.wind?.who ?? 'vehicles'}' limits: held
              from {weather.gust_soft_mps ?? '–'}, refused from {weather.gust_hard_mps ?? '–'} m/s.
            </p>
          </section>
        )}

        <section>
          <h4>Its vehicles</h4>
          {total > 0 ? (
            <>
              <div className="hd-bar" role="img" aria-label={d.fleet.map((s) => `${s.n} ${s.label.toLowerCase()}`).join(', ')}>
                {d.fleet.map((s) => (
                  <span key={s.state} className={`fs-${s.state}`} style={{ flexGrow: s.n }} title={`${s.n} ${s.label.toLowerCase()}`} />
                ))}
              </div>
              <ul className="hd-legend">
                {d.fleet.map((s) => (
                  <li key={s.state} className={`fs-${s.state}`}>
                    <b className="num">{s.n}</b> <span>{s.label.toLowerCase()}</span>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p className="muted">None based here.</p>
          )}
          <dl>
            <dt>Based here</dt>
            <dd className="num">{d.based}</dd>
            <dt>Here now</dt>
            <dd>
              <span className="num">{d.here}</span>
              {d.pads != null && <span className="muted"> of {d.pads} pads taken</span>}
              {d.visitors > 0 && <span className="muted"> ({d.visitors} visiting)</span>}
            </dd>
            {d.packs != null && (
              <>
                <dt>Charged packs</dt>
                <dd className="num">{d.packs}</dd>
              </>
            )}
          </dl>
          {canOpen && (
            <button className="ghost mini hd-act" onClick={onToggleOpen}>
              {open ? 'Gather the parked ones again' : 'Show the parked ones one by one'}
            </button>
          )}
        </section>

        <section>
          <h4>Its parcels</h4>
          <dl>
            <dt>Waiting here</dt>
            <dd>
              <span className="num">{p.waiting}</span>
              {p.late > 0 && <span className="bad"> ({p.late} late)</span>}
            </dd>
            {p.nextDueS != null && (
              <>
                <dt>Next due</dt>
                <dd>in {spanText(p.nextDueS)}</dd>
              </>
            )}
            <dt>In transit</dt>
            <dd className="num">{p.inTransit}</dd>
            <dt>Delivered</dt>
            <dd>
              <span className="num">{p.delivered}</span>
              {p.failed > 0 && <span className="bad"> ({p.failed} failed)</span>}
            </dd>
            {p.later > 0 && (
              <>
                <dt>Ordered later</dt>
                <dd className="muted">
                  <span className="num">{p.later}</span> not placed yet
                </dd>
              </>
            )}
          </dl>
          <button className="ghost mini hd-act" onClick={onParcels}>
            List its parcels
          </button>
        </section>
      </div>
    </aside>
  )
}
