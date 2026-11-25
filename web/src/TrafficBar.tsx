import { feedRows, type Picture, type TrafficSensor } from './traffic'
import { ageText, distText, sourceKindWords } from './trafficText'
import { COVERAGE_MAX_M } from './trafficLayers'
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
 *  each feed's state a click away), the receivers' coverage, and, in a dev run only, the
 *  simulator's truth beside the picture. */
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
        </div>
      )}
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
