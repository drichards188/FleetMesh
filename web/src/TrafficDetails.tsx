import { useEffect } from 'react'
import { FloatGrip } from './FloatingPanel'
import { useFloatBar } from './floating'
import { ageS, isBearingOnly, type ShownTrack, type TrafficSensor, type TruthMatch } from './traffic'
import { ageText, altDetail, classWords, climbText, courseText, distText, flagLines, sourceLine, speedText, stateWords, staticRows, trackCode, trackName, trackTitle, uncertaintyText } from './trafficText'
import { trafficSvg } from './trafficSymbols'
import './traffic.css'

/** A selected traffic track's details: who it is (callsign or name, its address, or anonymous),
 *  what it is, how sure the picture is of it and from what, its flags in plain words, and what AIS
 *  or Remote ID said about it. Read-only: the fleet gives nothing to traffic it does not control.
 *  `truth`: in a dev run with the truth overlay on, the simulator's object it stands for. */
export function TrafficDetails({
  row,
  pictureT,
  sensors,
  nameOf,
  truth,
  onFly,
  onClose,
}: {
  row: ShownTrack
  pictureT: number
  sensors: readonly TrafficSensor[]
  nameOf: (uid: string) => string
  truth?: TruthMatch | null
  onFly: () => void
  onClose: () => void
}) {
  const t = row.track
  const bar = useFloatBar()
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (e.key !== 'Escape' || e.defaultPrevented || (el && ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName))) return
      onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  const name = trackName(t)
  const code = trackCode(t)
  const flags = flagLines(t)
  const statics = staticRows(t.static)
  const bearing = isBearingOnly(t)
  const near = row.rel
  const ours = near.nearest != null ? nameOf(String(near.nearest)) : null
  const speed = speedText(t)
  const course = courseText(t)
  const climb = climbText(t)
  return (
    <aside className="vdetails tdetails" aria-label={`Traffic: ${trackTitle(t)}`}>
      <header className="vd-head" {...bar}>
        <FloatGrip />
        <span className="vd-sym td-sym" dangerouslySetInnerHTML={{ __html: trafficSvg(t.class, t.domain, t.state === 'tentative', 30) }} />
        <div className="vd-title">
          <div className="vd-name">
            {trackTitle(t)} <span className="td-not-ours">not ours</span>
          </div>
          <div className="muted">
            {classWords(t.class)}
            {name && code ? ` · ${code}` : ''}
            {!t.cooperative ? ' · seen only by our sensors' : ''}
          </div>
        </div>
        <button className="ghost mini" onClick={onFly} title="Fly the map to it">
          fly to
        </button>
        <button className="ghost mini" onClick={onClose} aria-label="Close traffic details">
          ✕
        </button>
      </header>
      <div className="vd-body">
        {flags.some((f) => f.tone !== 'info') && (
          <ul className="td-flags">
            {flags
              .filter((f) => f.tone !== 'info')
              .map((f) => (
                <li key={f.flag} className={f.tone}>
                  {f.text}
                </li>
              ))}
          </ul>
        )}
        <section>
          <h4>Picture</h4>
          <dl>
            <dt>State</dt>
            <dd>{stateWords(t.state)}</dd>
            <dt>Position</dt>
            <dd>{uncertaintyText(t, nameOf)}</dd>
            {!bearing && (
              <>
                <dt>Altitude</dt>
                <dd>{altDetail(t)}</dd>
              </>
            )}
            {speed && (
              <>
                <dt>Speed</dt>
                <dd>
                  {speed}
                  {course ? `, course ${course}` : ''}
                </dd>
              </>
            )}
            {climb && (
              <>
                <dt>Vertical</dt>
                <dd>{climb}</dd>
              </>
            )}
            <dt>Last report</dt>
            <dd>{ageText(ageS(t, pictureT))} ago</dd>
            <dt>To the fleet</dt>
            <dd>
              {Number.isFinite(near.distM) ? (
                <>
                  {distText(near.distM)} from {ours ?? 'a route of ours'}
                  {near.nearest != null && near.dAltM != null ? `, ${distText(Math.abs(near.dAltM))} ${near.dAltM >= 0 ? 'above' : 'below'} it` : ''}
                </>
              ) : (
                'none of ours it could matter to'
              )}
              <span className="muted"> ({near.why})</span>
            </dd>
          </dl>
        </section>
        <section>
          <h4>Sources</h4>
          <ul className="td-sources">
            {t.sources.length === 0 && <li className="muted">none listed</li>}
            {[...t.sources]
              .sort((a, b) => b.last_t - a.last_t)
              .map((s) => (
                <li key={`${s.kind}:${s.rx}`}>{sourceLine(s, pictureT, nameOf, sensors)}</li>
              ))}
          </ul>
        </section>
        {(statics.length > 0 || flags.some((f) => f.tone === 'info')) && (
          <section>
            <h4>{t.domain === 'surface' ? 'AIS static data' : 'Broadcast data'}</h4>
            {statics.length > 0 && (
              <dl>
                {statics.map(([k, v]) => (
                  <div key={k} className="td-pair">
                    <dt>{k}</dt>
                    <dd>{v}</dd>
                  </div>
                ))}
              </dl>
            )}
            {flags
              .filter((f) => f.tone === 'info')
              .map((f) => (
                <p key={f.flag} className="muted">
                  {f.text}
                </p>
              ))}
          </section>
        )}
        {truth && (
          <section className="td-truth">
            <h4>Simulator truth, not available to a real operator</h4>
            <p>
              {truth.truth.ids?.callsign?.trim() || truth.truth.ids?.name?.trim() || truth.truth.id}: {classWords(truth.truth.class)}
              {truth.errM != null ? `, ${distText(truth.errM)} from this track` : ''}
              {truth.truth.faults?.length ? `; faults: ${truth.truth.faults.join(', ')}` : ''}
            </p>
          </section>
        )}
        <p className="muted td-id">
          {t.id} · {t.cooperative ? 'cooperative (it broadcasts)' : 'non-cooperative (no broadcast)'}
        </p>
      </div>
    </aside>
  )
}
