// The cockpit layout's top bar (Cockpit.tsx; bd epic murmur-iobd, direction 1): one line, its pieces
// grouped by job with space between the groups and no rules between them. On the left what is
// happening: the menu and the brand; the run (who decides, the scenario, the newest thing to watch;
// a click opens the Situation panel); the glance (ours, ours in trouble, the problem count a link
// to what it counts). On the right where to act and who acts:
// the Inbox, the one primary button; the other drawers; Views; the operator; the time, the quietest.
// Short of room (1366 px and a situation) the run's newest beat truncates first, then the glance
// folds into its problem counts, then a beat too short to read hides (the Situation panel has it),
// then the run's title truncates: the bar never takes a second row (useBarFold).
//
// Display only: every control is the console's own part, doing what it always did; the two links
// this adds (the trouble count) only show a vehicle, as a click on its row does.
import { useRef } from 'react'
import { glanceWords, type GlancePiece } from '../topbar'
import type { BarFigures, ConsoleParts } from './parts'



export function CockpitBar({ parts }: { parts: ConsoleParts }) {
  const { bar, facts } = parts
  const ref = useRef<HTMLElement>(null)
  const f = bar.figures
  return (
    <header ref={ref} className="topbar cp-bar" >
      <div className="cp-home">
        {bar.menu}
        {bar.brand}
      </div>
      <div className={`cp-status${facts.live ? '' : ' stale'}`}>
        {bar.connection}
        {f ? (
          <>
            <Glance f={f} folded={false} />
            {/* the whole line, unseen, for the width it would take (useGlanceFold) */}
            <span className="cp-measure-box" aria-hidden="true">
              <Glance f={f} folded={false} measure />
            </span>
          </>
        ) : (
          bar.glance
        )}
      </div>
      <nav className="cp-work" aria-label="Drawers over the map's right edge, one at a time">
        <span className="cp-inbox">{bar.inbox}</span>
        {bar.queue}
        {bar.sitrep}
        {bar.officer}
        {bar.parcels}
      </nav>
      {bar.load}
      <div className="cp-ref">{bar.views}</div>
      <div className="cp-who">
        {bar.operator}
        {bar.clock}
      </div>
    </header>
  )
}

/** The glance line (topbar.ts `glanceWords`): "105 vehicles · 3 of ours in trouble", or folded
 *  "3 of ours in trouble". The problem count is a link: the next of ours in trouble in the Fleet
 *  list. `measure`: the same words as plain text, for the width alone. */
function Glance({ f, folded, measure = false }: { f: BarFigures; folded: boolean; measure?: boolean }) {
  const pieces = glanceWords(f.glance, f.trouble.n, folded)
  return (
    <span className={`cp-glance${measure ? ' cp-measure' : ''}`}>
      {pieces.map((p, k) => (
        <Piece key={k} p={p} f={f} measure={measure} />
      ))}
    </span>
  )
}

function Piece({ p, f, measure }: { p: GlancePiece; f: BarFigures; measure: boolean }) {
  if (p.kind === 'sep') return <span className="cp-sep">{p.text}</span>
  const words = (
    <>
      <b>{p.n}</b> {p.text}
    </>
  )
  if (p.kind === 'ours') {
    return (
      <span className="cp-total" title={measure ? undefined : `${p.n} of ours in the fleet`}>
        {words}
      </span>
    )
  }
  const tone = f.trouble.tone ?? ''
  if (measure) return <span className={`cp-link ${tone}`}>{words}</span>
  return (
    <button type="button" className={`cp-link ${tone}`} onClick={f.showTrouble} title={f.trouble.says} aria-label={f.trouble.says}>
      {words}
    </button>
  )
}

