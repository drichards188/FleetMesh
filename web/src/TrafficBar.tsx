import { useState } from 'react'
import { symbolSvg } from './symbols'
import { feedRows, type Picture, type TrafficSensor } from './traffic'
import { ageText, distText, sourceKindWords } from './trafficText'
import { COVERAGE_MAX_M } from './trafficLayers'
import { classSvg, type Frame } from './trafficSymbols'
import './traffic.css'

export interface TrafficToggles {
  on: boolean
  air: boolean
  surface: boolean
  coverage: boolean
  truth: boolean
}

/** The map's traffic bar: the layer on or off, each domain with how many tracks the picture has
 *  in it, how many are near the fleet, the picture's health (ok, or degraded with its reasons,
 *  each feed's state a click away), the receivers' coverage, the key to its symbols, and, in a dev
 *  run only, the simulator's truth beside the picture. */
export function TrafficBar({
  available,
  toggles,
  set,
  totals,
  near,
  picture,
  pictureT,
  sensors,
  truthAllowed,
  truthAvailable,
  missed,
  open,
  setOpen,
}: {
  available: boolean | null
  toggles: TrafficToggles
  set: (t: Partial<TrafficToggles>) => void
  /** tracks in the picture by domain, shown or not */
  totals: { air: number; surface: number }
  near: number
  picture: Picture | null
  pictureT: number
  sensors: readonly TrafficSensor[] | null
  /** a dev run: the truth overlay may be offered */
  truthAllowed: boolean
  truthAvailable: boolean | null
  /** truth objects the picture has nothing for, with the overlay on */
  missed: number | null
  /** the feeds shown (App keeps it: a picture alert's link opens it) */
  open: boolean
  setOpen: (f: (o: boolean) => boolean) => void
}) {
  const [keyOpen, setKeyOpen] = useState(false)
  const degraded = picture?.state === 'degraded'
  const feeds = feedRows(picture, pictureT)
  const far = (sensors ?? []).filter((s) => s.owner === 'company' && s.nominal_range_m > COVERAGE_MAX_M)
  const farKinds = [...new Set(far.map((s) => `${sourceKindWords(s.kind === 'ais' ? 'ais_a' : s.kind).replace(' Class A', '')} ${distText(s.nominal_range_m)}`))]
  if (available === false)
    return (
      <aside className="traffic-bar" aria-label="Traffic layer">
        <div className="tb-row">
          <span className="tb-title">Traffic</span>
          <span className="tb-quiet">no traffic picture in this run</span>
        </div>
      </aside>
    )
  return (
    <aside className={`traffic-bar${degraded ? ' degraded' : ''}`} aria-label="Traffic layer">
      <div className="tb-row">
        <button className={`tb-title chip ${toggles.on ? 'on' : ''}`} aria-pressed={toggles.on} onClick={() => set({ on: !toggles.on })} title="Other traffic, as the company's receivers and sensors see it (never ground truth): on or off">
          Traffic
        </button>
        {toggles.on && (
          <>
            <span className="tb-toggles" role="group" aria-label="Traffic by domain">
              <button className={`chip air ${toggles.air ? 'on' : ''}`} aria-pressed={toggles.air} onClick={() => set({ air: !toggles.air })} title={`Aircraft, helicopters and other operators' drones: ${totals.air} in the picture`}>
                Air <b>{totals.air}</b>
              </button>
              <button className={`chip surface ${toggles.surface ? 'on' : ''}`} aria-pressed={toggles.surface} onClick={() => set({ surface: !toggles.surface })} title={`Ships, ferries, tugs and small craft: ${totals.surface} in the picture`}>
                Surface <b>{totals.surface}</b>
              </button>
            </span>
            <span className="tb-near" title="Drawn in full: near our vehicles or their routes, or flagged. The rest are smaller and fainter, airliners at altitude faintest.">
              <b>{near}</b> near
            </span>
          </>
        )}
        <button className={`tb-health ${degraded ? 'bad' : picture ? 'ok' : 'none'}`} aria-expanded={open} onClick={() => setOpen((o) => !o)} title="The traffic picture's health: each feed and receiver">
          {picture ? (degraded ? 'Picture degraded' : 'Picture ok') : 'No picture yet'}
        </button>
      </div>
      {degraded && picture && picture.reasons.length > 0 && !open && <p className="tb-reason">{picture.reasons.join('; ')}</p>}
      {open && (
        <div className="tb-panel">
          {degraded && picture && (
            <ul className="tb-reasons">
              {picture.reasons.length ? picture.reasons.map((r) => <li key={r}>{r}</li>) : <li>degraded (the bridge gave no reason)</li>}
            </ul>
          )}
          {feeds.length > 0 ? (
            <table className="tb-feeds">
              <thead>
                <tr>
                  <th>Feed</th>
                  <th>Lines/s</th>
                  <th>Last line</th>
                </tr>
              </thead>
              <tbody>
                {feeds.map((f) => (
                  <tr key={f.name} className={f.tone}>
                    <td>{f.name}</td>
                    <td>{f.connected ? f.rate.toFixed(f.rate < 10 ? 1 : 0) : 'not connected'}</td>
                    <td>{f.ageS == null ? 'never' : `${ageText(f.ageS)} ago`}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p className="tb-quiet">The bridge lists no feeds.</p>
          )}
          <p className="tb-quiet">What the company can receive or detect: other traffic it cannot hear or see is not here.</p>
        </div>
      )}
      {toggles.on && (
        <div className="tb-row tb-sub">
          <button className={`chip ${toggles.coverage ? 'on' : ''}`} aria-pressed={toggles.coverage} onClick={() => set({ coverage: !toggles.coverage })} title="The company's receivers and sensors: rings of their nominal range (not a coverage prediction)">
            Coverage
          </button>
          {truthAllowed && (
            <button
              className={`chip truth ${toggles.truth ? 'on' : ''}`}
              aria-pressed={toggles.truth}
              disabled={truthAvailable === false && !toggles.truth}
              onClick={() => set({ truth: !toggles.truth })}
              title="Dev runs only: the simulator's truth beside the picture, to see what the company misses. A real operator does not have it."
            >
              Simulator truth
            </button>
          )}
          <button className={`chip tb-key-chip ${keyOpen ? 'on' : ''}`} aria-expanded={keyOpen} onClick={() => setKeyOpen((o) => !o)} title="What the traffic symbols mean">
            Key
          </button>
        </div>
      )}
      {toggles.on && keyOpen && <TrafficKey />}
      {toggles.on && toggles.coverage && farKinds.length > 0 && <p className="tb-cov">Not drawn, they reach past any view of the bay: {farKinds.join(', ')}.</p>}
      {toggles.on && toggles.truth && truthAllowed && (
        <p className="tb-truth" role="note">
          <i aria-hidden="true" /> Simulator truth, not available to a real operator.{' '}
          {truthAvailable === false ? 'This run serves none.' : missed != null ? `${missed} missed by the picture (open rings).` : 'Loading.'}
        </p>
      )}
    </aside>
  )
}

// ---- the key ----------------------------------------------------------------------------------

const KEY_SIZE = 12
const NEUTRAL: Frame = { identity: 'neutral', dashed: false }
const UNKNOWN: Frame = { identity: 'unknown', dashed: false }
let keySvgs: Record<string, string> | null = null
/** the key's examples, drawn once */
function keySymbols(): Record<string, string> {
  keySvgs ??= {
    idAir: classSvg('ga', 'air', NEUTRAL, KEY_SIZE),
    idSea: classSvg('ferry', 'surface', NEUTRAL, KEY_SIZE),
    unAir: classSvg('unknown_air', 'air', UNKNOWN, KEY_SIZE),
    unSea: classSvg('unknown_surface', 'surface', UNKNOWN, KEY_SIZE),
    coasting: classSvg('tug', 'surface', { identity: 'neutral', dashed: true }, KEY_SIZE),
    tentative: classSvg('unknown_surface', 'surface', { identity: 'pending', dashed: false }, KEY_SIZE),
    heli: classSvg('helicopter', 'air', NEUTRAL, KEY_SIZE),
    drone: classSvg('drone', 'air', NEUTRAL, KEY_SIZE),
    cargo: classSvg('cargo', 'surface', NEUTRAL, KEY_SIZE),
    bearing: classSvg('unknown_air', 'air', UNKNOWN, 8, false),
    ours: symbolSvg({ kind: 'copter' }, { stale: false, size: 20 }).svg,
  }
  return keySvgs
}

const Sym = ({ svg, className = '' }: { svg: string; className?: string }) => <span className={`tb-key-sym ${className}`} aria-hidden="true" dangerouslySetInnerHTML={{ __html: svg }} />

/** What the traffic symbols mean, in plain words: the two frame colours, a dashed frame, the line
 *  ahead, the mark inside, the red ring, a wedge, and that the round icons are ours. */
function TrafficKey() {
  const k = keySymbols()
  return (
    <dl className="tb-key" aria-label="Traffic key">
      <dt>
        <Sym svg={k.idAir} />
        <Sym svg={k.idSea} />
      </dt>
      <dd>
        <b>Identified, broadcasting:</b> it sends its position and who it is.
      </dd>
      <dt>
        <Sym svg={k.unAir} />
        <Sym svg={k.unSea} />
      </dt>
      <dd>
        <b>Unidentified or not broadcasting:</b> seen only by our sensors, or no identity. A cloud in the air, four lobes on the water.
      </dd>
      <dt>
        <Sym svg={k.coasting} />
        <Sym svg={k.tentative} />
      </dt>
      <dd>
        <b>Dashed:</b> no fresh report, position extrapolated. <b>Hollow:</b> not yet confirmed.
      </dd>
      <dt>
        <Sym svg={k.idAir} className="tb-key-vec" />
      </dt>
      <dd>
        <b>The line:</b> where it will be in 30 s (aircraft) or 60 s (vessels).
      </dd>
      <dt>
        <Sym svg={k.heli} />
        <Sym svg={k.drone} />
        <Sym svg={k.cargo} />
      </dt>
      <dd>
        <b>Inside:</b> plane, rotor (helicopter), V (drone); a hull, lettered A cargo, O tanker, P passenger, F ferry, T tug. Far off, the frame alone.
      </dd>
      <dt>
        <Sym svg={k.idSea} className="tb-key-ring" />
      </dt>
      <dd>
        <b>Red ring:</b> something is wrong: a broadcast our sensors contradict, a jump, an emergency.
      </dd>
      <dt>
        <span className="tb-key-wedge" aria-hidden="true">
          <svg width="48" height="18" viewBox="0 0 52 18">
            <path d="M1 9 L51 1 L51 17 Z" />
            <line x1="1" y1="9" x2="51" y2="9" />
          </svg>
          <Sym svg={k.bearing} />
        </span>
      </dt>
      <dd>
        <b>Wedge:</b> seen along a line of sight only, somewhere in it.
      </dd>
      <dt>
        <Sym svg={k.ours} />
      </dt>
      <dd>
        <b>Round icons are ours.</b> Other traffic is never round.
      </dd>
    </dl>
  )
}
