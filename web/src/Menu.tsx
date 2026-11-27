import { Bluf } from './Bluf'
import { type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, useEffect, useState } from 'react'
import { type ConsoleMode, type ConsoleState, endScenario, setConsoleMode, isActive, type Launch, type LaunchOptions, type LauncherState, type PolicyId, type Scenario, type Stage, startScenario } from './launcher'
import { DOMAIN_LABEL, type Domain } from './types'
import { FleetBackdrop, FleetLegend } from './FleetBackdrop'
import './menu.css'

/** The main menu (`make start`): the delivery scenarios first (the first is the default), then the
 *  open world when it runs here, each with what it has; a scenario's options (its fleet at a
 *  fraction of its size, its clock, its drop-zone procedure) with its own defaults chosen; the
 *  launch's progress, and the scenario running with Return and End. Over the console, which stays
 *  live behind it while a scenario runs. Behind the menu, a harbour city in cross-section with the
 *  fleet's vehicles going about their deliveries and the links they use, named in the top rail
 *  (FleetBackdrop): decoration, never a run's vehicles. */

const POLICY_LABEL: Record<PolicyId, string> = { standard: 'Standing approvals', strict: 'Strict' }
const POLICY_NOTE: Record<PolicyId, string> = {
  standard: 'Approved delivery points stand: routine deliveries need no person (sim/policy.toml).',
  strict: 'The comparison procedure: a person confirms each drop zone, valid for an hour, before deliveries go there (sim/policy-strict.toml).',
}
const SCALE_LABEL: Record<string, string> = { '1': 'Full', '0.5': 'Half', '0.25': 'Quarter' }
const scaleLabel = (k: number) => SCALE_LABEL[String(k)] ?? `${k}x`

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']
const DOMAINS: Domain[] = ['air', 'ground', 'surface']
const pad2 = (n: number) => String(n).padStart(2, '0')

/** m:ss */
function mmss(s: number) {
  const n = Math.max(0, Math.round(s))
  return `${Math.floor(n / 60)}:${pad2(n % 60)}`
}

/** "14 Jun 2026, 10:00Z" */
function fmtAt(at: string | undefined) {
  const d = at ? new Date(at) : null
  if (!d || Number.isNaN(d.getTime())) return at ?? ''
  const hm = `${pad2(d.getUTCHours())}:${pad2(d.getUTCMinutes())}Z`
  return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()}, ${hm}`
}

const plural = (n: number | undefined, one: string, many = `${one}s`) => `${n ?? 0} ${n === 1 ? one : many}`

const realConditions = (s: Scenario) => !!s.tidewater && s.tidewater !== 'none'

/** a card's facts, one short line each */
function cardFacts(s: Scenario): string[] {
  if (!s.runnable) return [s.why_not ?? '']
  const wx = realConditions(s) ? ['tide, current, wind'] : []
  if (s.kind === 'open_world') return [plural(s.vehicles, 'vehicle'), 'ArduPilot SITL', ...wx]
  return [plural(s.vehicles, 'vehicle'), `${Math.round((s.duration_s ?? 0) / 60)} min`, ...wx]
}

/** the options a launch was started with, as the menu names them */
function optionWords(kind: Launch['kind'], o: LaunchOptions): string[] {
  if (kind === 'open_world') return ['free command, nothing scripted']
  return [`${scaleLabel(o.scale ?? 1).toLowerCase()} size`, `${o.rate ?? 1}x clock`, `${POLICY_LABEL[o.policy ?? 'standard'].toLowerCase()}`]
}

/** the same start from a terminal (eval/launcher.py's cli_text) */
function cliFor(s: Scenario, o: LaunchOptions): string {
  if (s.kind === 'open_world') return s.cli ?? ''
  const out = [`make start SCENARIO=${s.id}`]
  if (o.scale !== undefined && o.scale !== 1) out.push(`SCALE=${o.scale}`)
  if (o.rate !== undefined && o.rate !== s.rate) out.push(`FLEETMESH_RATE=${o.rate}`)
  if (o.policy && o.policy !== s.policy) out.push('POLICY=sim/policy-strict.toml')
  return out.join(' ')
}

/** how long vehicles usually take, as the menu says while it waits */
function typical([lo, hi]: [number, number]) {
  return hi >= 60 ? `Vehicles usually take ${lo} to ${hi} s to be ready (ArduPilot SITL).` : `Vehicles are usually ready in ${lo} to ${hi} s (stand-ins).`
}

/** a fleet's vehicles in each domain, as a sentence */
const mixText = (mix: Record<Domain, number>) => DOMAINS.map((d) => `${mix[d]} ${DOMAIN_LABEL[d].toLowerCase()}`).join(', ')

/** A fleet by domain, the menu's strata (air on top, the water below): a pip a vehicle, every row
 *  on one scale, squeezed into a bar when a row would not fit. With labels and counts in the
 *  detail, which a screen reader reads; on a card, the shape alone (the card's facts say it). */
function Strata({ mix, labelled = false }: { mix: Record<Domain, number>; labelled?: boolean }) {
  const width = labelled ? 168 : 56
  const most = Math.max(1, ...DOMAINS.map((d) => mix[d]))
  const pitch = Math.min(labelled ? 10 : 6, width / most)
  const pip = pitch >= 3 ? pitch - 2 - (labelled ? 1 : 0) : pitch
  const a11y = labelled ? { role: 'img', 'aria-label': `By domain: ${mixText(mix)}` } : { 'aria-hidden': true }
  return (
    <span className={`mm-strata${labelled ? ' labelled' : ''}`} {...a11y}>
      {DOMAINS.map((d) => (
        <span key={d} className={`mm-stratum d-${d}${mix[d] ? '' : ' none'}`}>
          {labelled && <span className="mm-stratum-k">{DOMAIN_LABEL[d]}</span>}
          <span className="mm-stratum-track" style={{ width }}>
            <span className="mm-stratum-pips" style={{ width: mix[d] * pitch, '--pip': `${pip}px`, '--pitch': `${pitch}px` } as CSSProperties} />
          </span>
          {labelled && <b>{mix[d]}</b>}
        </span>
      ))}
    </span>
  )
}

/** the launch's word on a card: starting, running or time up */
const cardStage = (stage: Stage) => (stage === 'ended' ? 'time up' : stage === 'running' ? 'running' : 'starting')

interface MenuProps {
  state: LauncherState
  /** this browser's clock (ms) when the state came */
  received: number
  lost: boolean
  onUpdate: (current: Launch | null) => void
  /** back to the console, when a scenario is up */
  onReturn?: () => void
}

export function Menu({ state, received, lost, onUpdate, onReturn }: MenuProps) {
  const cur = state.current
  const scenarios = state.scenarios
  const startable = scenarios.filter((s) => s.runnable)
  const delivery = startable.filter((s) => s.kind === 'scenario')
  const openWorld = startable.filter((s) => s.kind === 'open_world')
  // what the backdrop shows now, named faintly in the top rail
  const [scene, setScene] = useState('')
  // chosen: the scenario running, else the default (the first delivery scenario)
  const [selId, setSelId] = useState(() => (isActive(cur) ? cur.scenario : (scenarios.find((s) => s.default)?.id ?? scenarios[0]?.id)))
  const sel = scenarios.find((s) => s.id === selId) ?? scenarios[0]
  // the launcher's clock, ticking here between its answers
  const [nowMs, setNowMs] = useState(() => Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [])
  const now = state.now + (nowMs - received) / 1000
  const utc = new Date(now * 1000)
  // Escape goes back to the console, when one is up
  useEffect(() => {
    if (!onReturn) return
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onReturn()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onReturn])

  // the arrow keys walk the list, choosing as they go
  const onListKey = (e: ReactKeyboardEvent<HTMLElement>) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
    const cards = [...e.currentTarget.querySelectorAll<HTMLButtonElement>('.mm-card')]
    const at = cards.indexOf(document.activeElement as HTMLButtonElement)
    const to = e.key === 'Home' ? 0 : e.key === 'End' ? cards.length - 1 : Math.min(cards.length - 1, Math.max(0, at + (e.key === 'ArrowDown' ? 1 : -1)))
    if (at < 0 && e.key !== 'Home' && e.key !== 'End') return
    e.preventDefault()
    cards[to]?.focus()
    cards[to]?.click()
  }

  // a click chooses; in one column (menu.css), where the detail is under the list, it brings the
  // detail up too (a pointer's click only: the arrow keys stay in the list)
  const choose = (id: string, e: ReactMouseEvent) => {
    setSelId(id)
    if (e.detail > 0 && window.matchMedia('(max-width: 1000px)').matches) {
      const behavior = window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth'
      window.requestAnimationFrame(() => document.getElementById('mm-detail')?.scrollIntoView({ block: 'start', behavior }))
    }
  }

  let order = 0
  const card = (s: Scenario) => {
    const live = isActive(cur) && cur.scenario === s.id ? cur : null
    const tone = live ? (live.stage === 'running' ? 'live' : live.stage === 'ended' ? 'ended' : 'busy') : ''
    const facts = cardFacts(s)
    return (
      <button
        key={s.id}
        className={`mm-card${s.id === sel?.id ? ' sel' : ''}${s.runnable ? '' : ' off'}${s.kind === 'open_world' ? ' world' : ''}${live ? ` now ${tone}` : ''}`}
        style={{ '--i': order++ } as CSSProperties}
        aria-pressed={s.id === sel?.id}
        onClick={(e) => choose(s.id, e)}
      >
        {s.runnable && s.domains && <Strata mix={s.domains} />}
        <span className="mm-card-body">
          <span className="mm-card-top">
            <span className="mm-card-title">{s.title}</span>
            {s.default && <span className="mm-tag">Default</span>}
            {live && (
              <span className={`mm-card-state ${tone}`}>
                <i className="mm-dot" />
                {cardStage(live.stage)}
              </span>
            )}
          </span>
          <span className="mm-card-facts">
            {facts.map((f, k) => (
              <span key={k}>{f}</span>
            ))}
          </span>
        </span>
      </button>
    )
  }

  return (
    <div className="mm" role="dialog" aria-modal="true" aria-label="Main menu">
      <FleetBackdrop onScene={setScene} />
      <div className="mm-frame" aria-hidden="true" />
      <header className="mm-top">
        <span className="mm-where">Main menu</span>
        <FleetLegend />
        {scene && (
          <span key={scene} className="mm-scene" aria-hidden="true" title="What the backdrop shows now: decoration, not a run">
            {scene}
          </span>
        )}
        <span className="spacer" />
        {state.console && <ConsoleServe c={state.console} busy={isActive(cur)} now={now} onChanged={() => onUpdate(null)} />}
        {lost && !state.console?.working && (
          <span className="mm-lost" role="status">
            The launcher is not answering: is <code>make start</code> still running?
          </span>
        )}
        <span className="mm-utc" title="The launcher's clock, UTC">
          <span>
            {utc.getUTCDate()} {MONTHS[utc.getUTCMonth()]} {utc.getUTCFullYear()}
          </span>
          <b>
            {pad2(utc.getUTCHours())}:{pad2(utc.getUTCMinutes())}:{pad2(utc.getUTCSeconds())}Z
          </b>
        </span>
        {onReturn && (
          <button className="ghost mm-return" onClick={onReturn} title="Back to the console (Esc)">
            Return to console
          </button>
        )}
      </header>
      <div className="mm-scroll">
        <div className="mm-grid">
          <div className="mm-left mm-hud">
            <div className="mm-left-scroll">
              <div className="mm-ident">
                <h1 className="mm-word" aria-label="FleetMesh">
                  {'FLEETMESH'.split('').map((c, k) => (
                    <span key={k} aria-hidden="true" style={{ '--i': k } as CSSProperties}>
                      {c}
                    </span>
                  ))}
                </h1>
                <p className="mm-sub">Delivery fleet supervision</p>
                <p className="mm-pitch">One operator over autonomous vehicles in the air, on the ground and on the water: a place for the public to develop informed opinions on how automation at scale can and should be controlled by people.</p>
              </div>
              <nav className="mm-list" aria-label="Scenarios" onKeyDown={onListKey}>
                <h2 className="mm-h">
                  Scenarios <span>{delivery.length}</span>
                </h2>
                <div className="mm-cards">{delivery.map(card)}</div>
                {openWorld.length > 0 && (
                  <>
                    <h2 className="mm-h">Open world</h2>
                    <div className="mm-cards">{openWorld.map(card)}</div>
                  </>
                )}
                {startable.length === 0 && <p className="mm-why">No scenario runs here: see the launcher's output for why.</p>}
              </nav>
            </div>
          </div>
          <div className="mm-right">
            {cur && <Now key={cur.id} launch={cur} now={now} onUpdate={onUpdate} onReturn={onReturn} />}
            {sel && <Detail key={sel.id} s={sel} cur={cur} onUpdate={onUpdate} onReturn={onReturn} />}
          </div>
        </div>
      </div>
    </div>
  )
}

// ---- how the console is served -----------------------------------------------------------------

/** Built (production: the default, much faster at scale) or development (hot reload, for people
 *  changing the console). A switch builds first when the sources changed, then restarts the server
 *  this page is on, and the page reloads itself; refused while a scenario is under way. */
function ConsoleServe({ c, busy, now, onChanged }: { c: ConsoleState; busy: boolean; now: number; onChanged: () => void }) {
  const [error, setError] = useState<string | null>(null)
  const choose = async (mode: ConsoleMode) => {
    setError(null)
    try {
      await setConsoleMode(mode)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
    onChanged()
  }
  const working = c.working
  const rebuild = c.mode === 'build' && c.stale && !working
  let status = ''
  if (working === 'building') status = c.build.started ? `building the console, ${Math.max(0, Math.round(now - c.build.started))} s` : 'building the console'
  else if (working === 'restarting') status = 'restarting the server'
  else if (c.serving === 'build') {
    status = c.build.seconds != null ? `built in ${c.build.seconds} s` : 'built'
    if (c.stale) status += ', sources changed since'
  } else if (c.serving === 'dev') status = 'hot reload'
  const why = busy ? 'A scenario is starting or running: end it to change how the console is served.' : undefined
  return (
    <span className="mm-serve" role="group" aria-label="How the console is served" title={why ?? 'Built: the production console (fast, rebuilt when the sources change). Development: hot reload, for changing the console.'}>
      <span className="mm-serve-k">Console</span>
      {(['build', 'dev'] as const).map((m) => (
        <button key={m} className={`ghost mm-serve-b${c.serving === m ? ' on' : ''}`} aria-pressed={c.serving === m} disabled={busy || !!working || (c.serving === m && !(m === 'build' && c.stale))} onClick={() => choose(m)}>
          {m === 'build' ? (rebuild ? 'Rebuild' : 'Built') : 'Development'}
        </button>
      ))}
      <span className="mm-serve-s" role="status">
        {error ?? c.error ?? status}
      </span>
    </span>
  )
}

// ---- the launch: progress, running, ended -------------------------------------------------------

const STEPS: { stage: Stage; label: string }[] = [
  { stage: 'starting', label: 'Stack starting' },
  { stage: 'bridge', label: 'Bridge up' },
  { stage: 'ready', label: 'Fleet ready' },
  { stage: 'running', label: 'Running' },
]
const LAUNCHING: Stage[] = ['waiting', 'starting', 'bridge', 'ready']
const HEADLINE: Record<Stage, string> = {
  waiting: 'Starting',
  starting: 'Starting',
  bridge: 'Starting',
  ready: 'Starting',
  running: 'Running',
  ended: 'Time up',
  stopping: 'Ending',
  stopped: 'Ended',
  failed: 'Failed',
}

function Now({ launch: l, now, onUpdate, onReturn }: { launch: Launch; now: number; onUpdate: (c: Launch | null) => void; onReturn?: () => void }) {
  const [confirm, setConfirm] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const launching = LAUNCHING.includes(l.stage)
  const up = l.stage === 'running' || l.stage === 'ended'
  const at = STEPS.findIndex((s) => s.stage === l.stage)
  const tone = l.stage === 'failed' ? 'bad' : l.stage === 'stopped' ? 'quiet' : l.stage === 'ended' ? 'ended' : up ? 'live' : 'busy'
  const modes = optionWords(l.kind, l.options)
  const clock = l.clock && (up || l.stage === 'stopping') ? l.clock : null
  const steps = l.stage === 'waiting' ? [{ stage: 'waiting' as Stage, label: 'Ending the one before' }, ...STEPS] : STEPS

  const end = async () => {
    setBusy(true)
    setError(null)
    try {
      onUpdate(await endScenario())
      setConfirm(false)
    } catch (e) {
      setError((e as Error).message)
    }
    setBusy(false)
  }

  return (
    <section className={`mm-now mm-hud ${tone}`} aria-live="polite" aria-label="The scenario now">
      <div className="mm-now-head">
        <span className="mm-now-label">
          <i className="mm-dot" />
          {HEADLINE[l.stage]}
        </span>
        <span className="spacer" />
        {(launching || up) && (
          <span className="mm-now-clock" title="Since it was started">
            T+{mmss(now - l.started_t)}
          </span>
        )}
      </div>
      <h2>{l.title}</h2>
      <span className="mm-now-modes">
        {modes.map((m) => (
          <span key={m}>{m}</span>
        ))}
      </span>
      {launching && (
        <ol className="mm-steps" style={{ '--n': steps.length } as CSSProperties}>
          {steps.map((s, k) => {
            const state = l.stage === 'waiting' ? (k === 0 ? 'cur' : '') : at > k ? 'done' : at === k ? 'cur' : ''
            const count = s.stage === 'ready' && l.ready
            return (
              <li key={s.stage} className={state}>
                <span className="mm-step-mark" aria-hidden="true" />
                <span className="mm-step-label">
                  {s.label}
                  {count ? ` ${l.ready?.ready}/${l.ready?.of}` : ''}
                </span>
                {count && l.ready && (
                  <span className="mm-step-bar" aria-hidden="true">
                    <span style={{ transform: `scaleX(${l.ready.ready / Math.max(1, l.ready.of)})` }} />
                  </span>
                )}
              </li>
            )
          })}
        </ol>
      )}
      {clock && (
        <p className="mm-line">
          Scenario time {mmss(clock.elapsed_s)}
          {clock.duration_s ? ` of ${mmss(clock.duration_s)}` : ''}
        </p>
      )}
      {l.stage !== 'failed' && l.detail && <p className="mm-line">{l.detail}</p>}
      {launching && <p className="mm-note">{typical(l.typical_s)}</p>}
      {l.stage === 'failed' && (
        <>
          <p className="mm-fail">{l.error ?? l.detail}</p>
          <p className="mm-note">
            Its output: <code>{l.log}</code>
          </p>
        </>
      )}
      {l.leftovers.length > 0 && (
        <p className="mm-warn">
          Still held after it ended: {l.leftovers.join('; ')}. <code>make down</code> stops a stack.
        </p>
      )}
      {(launching || up) &&
        (confirm ? (
          <div className="mm-confirm" role="alertdialog" aria-label="End the scenario">
            <span>
              End {l.title}? Everything it started stops: the vehicles, the bridge, the link emulator, the traffic simulator and tidewater.
            </span>
            <button className="mm-danger" disabled={busy} onClick={end}>
              {busy ? 'Ending…' : 'End it'}
            </button>
            <button className="ghost" disabled={busy} onClick={() => setConfirm(false)}>
              Keep it running
            </button>
          </div>
        ) : (
          <div className="mm-actions">
            {onReturn && l.stack_up && (
              <button className="primary" onClick={onReturn}>
                {launching ? 'Open the console now' : 'Return to console'}
              </button>
            )}
            <button className="mm-danger" onClick={() => setConfirm(true)}>
              {launching ? 'Cancel' : 'End scenario'}
            </button>
          </div>
        ))}
      {error && <p className="mm-fail">{error}</p>}
    </section>
  )
}

// ---- a scenario's detail, its options and Start ---------------------------------------------------

function Fact({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <>
      <dt>{k}</dt>
      <dd>{children}</dd>
    </>
  )
}

function Detail({ s, cur, onUpdate, onReturn }: { s: Scenario; cur: Launch | null; onUpdate: (c: Launch | null) => void; onReturn?: () => void }) {
  const running = isActive(cur) ? cur : null
  // the scenario's own defaults, or what the run of it now was started with
  const mine = running?.scenario === s.id ? running.options : null
  const [scale, setScale] = useState<number>(mine?.scale ?? 1)
  const [rate, setRate] = useState<number | undefined>(mine?.rate ?? s.rate)
  const [policy, setPolicy] = useState<PolicyId | undefined>(mine?.policy ?? s.policy)
  const [confirm, setConfirm] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const scenario = s.kind === 'scenario'
  // the fleet at the chosen size (the full fleet for the open world)
  const sized = s.scales?.find((k) => k.scale === scale)
  const vehicles = sized?.vehicles ?? s.vehicles
  const domains = sized?.domains ?? s.domains
  const sitlNote = s.sitl ? (s.sitl === vehicles ? 'all in ArduPilot SITL' : `${s.sitl} in ArduPilot SITL, ${(vehicles ?? 0) - s.sitl} stand-ins`) : 'all stand-ins'
  const options: LaunchOptions = scenario ? { scale, rate, policy } : {}

  const start = async (replace: boolean) => {
    setBusy(true)
    setError(null)
    try {
      onUpdate(await startScenario({ scenario: s.id, ...options, replace }))
      setConfirm(false)
    } catch (e) {
      setError((e as Error).message)
    }
    setBusy(false)
  }

  return (
    <section id="mm-detail" className={`mm-detail mm-hud${s.runnable ? '' : ' off'}`} aria-label={s.title}>
      <div className="mm-kicker">
        <span>{scenario ? 'Scenario' : 'Open world'}</span>
        {s.default && <span className="mm-tag">Default</span>}
        {s.cli && <code title="The same from a terminal">{cliFor(s, options)}</code>}
      </div>
      <h2 className="mm-title">{s.title}</h2>
      <div className="mm-sheet">
        <div className="mm-spec">
          {s.runnable && domains && <Strata mix={domains} labelled />}
          {s.runnable && (
            <dl className="mm-facts">
              <Fact k="Fleet">
                <b>{plural(vehicles, 'vehicle')}</b> of {plural(s.platform_types, 'platform type')} ({sitlNote})
              </Fact>
              {scenario ? (
                <>
                  <Fact k="Duration">
                    <b>{Math.round((s.duration_s ?? 0) / 60)} min</b> of scenario time{rate && rate !== 1 ? ` at ${rate}x` : ''}, from {fmtAt(s.at)}; the stack stays up until you end it
                  </Fact>
                  <Fact k="Conditions">{realConditions(s) ? `Real tide, current and wind (tidewater ${s.tidewater})` : 'No tidewater data here: the archive weather only'}</Fact>
                  <Fact k="Scripted">Demand, hubs and drop zones, timed link, GNSS and weather events{s.traffic ? ', other air and sea traffic' : ''}</Fact>
                </>
              ) : (
                <>
                  <Fact k="Conditions">{realConditions(s) ? `Real tide, current and wind from tidewater (${s.tidewater}), from ${fmtAt(s.at)}` : 'No tidewater data here'}</Fact>
                  <Fact k="Duration">No limit: it runs until you end it</Fact>
                  <Fact k="Scripted">Nothing: no parcels, no timeline, no AI on watch</Fact>
                </>
              )}
              <Fact k="Scored">Nothing: a run is recorded (the audit log, which replay re-derives), not scored</Fact>
              {s.typical_s && <Fact k="Ready in">{typical(s.typical_s)}</Fact>}
            </dl>
          )}
          {!s.runnable && <p className="mm-why">Not started from the menu: {s.why_not}</p>}
        </div>
        <div className="mm-plan">
          {s.runnable && scenario && (
            <div className="mm-opts">
              {s.scales && s.scales.length > 1 && (
                <fieldset className="mm-opt">
                  <legend>Fleet and demand</legend>
                  <div className="seg" role="radiogroup" aria-label="Fleet and demand">
                    {s.scales.map((k) => (
                      <button key={k.scale} role="radio" aria-checked={scale === k.scale} className={scale === k.scale ? 'on' : ''} onClick={() => setScale(k.scale)}>
                        {scaleLabel(k.scale)}
                        <small>{k.vehicles}</small>
                      </button>
                    ))}
                  </div>
                  <p className="mm-note">{scale === 1 ? 'The scenario at its own size.' : `${scaleLabel(scale)} of its vehicles and of its parcels; every unit keeps at least one vehicle.`}</p>
                </fieldset>
              )}
              {s.rates && s.rates.length > 1 && (
                <fieldset className="mm-opt">
                  <legend>Scenario clock</legend>
                  <div className="seg" role="radiogroup" aria-label="Scenario clock">
                    {s.rates.map((r) => (
                      <button key={r} role="radio" aria-checked={rate === r} className={rate === r ? 'on' : ''} onClick={() => setRate(r)}>
                        {r}x{r === s.rate && <small>default</small>}
                      </button>
                    ))}
                  </div>
                  <p className="mm-note">{rate === 1 ? 'Real time: an hour of the scenario takes an hour.' : `${rate} times real time: the scenario's ${Math.round((s.duration_s ?? 0) / 60)} min take ${Math.round((s.duration_s ?? 0) / 60 / (rate ?? 1))}.`}</p>
                </fieldset>
              )}
              {s.policies && s.policies.length > 1 && (
                <fieldset className="mm-opt">
                  <legend>Drop zones</legend>
                  <div className="seg" role="radiogroup" aria-label="Drop zones">
                    {s.policies.map((k) => (
                      <button key={k} role="radio" aria-checked={policy === k} className={policy === k ? 'on' : ''} onClick={() => setPolicy(k)}>
                        {POLICY_LABEL[k]}
                        {k === s.policy && <small>default</small>}
                      </button>
                    ))}
                  </div>
                  {policy && <p className="mm-note">{POLICY_NOTE[policy]}</p>}
                </fieldset>
              )}
            </div>
          )}
          {s.runnable && (
            <div className="mm-go">
              {confirm && running ? (
                <div className="mm-confirm" role="alertdialog" aria-label="End the running scenario first">
                  <span>
                    {running.title} is {running.stage === 'ended' ? 'still up' : 'running'}. End it and start {s.title}?
                  </span>
                  <button className="primary" disabled={busy} onClick={() => start(true)}>
                    {busy ? 'Starting…' : 'End it and start'}
                  </button>
                  <button className="ghost" disabled={busy} onClick={() => setConfirm(false)}>
                    Cancel
                  </button>
                </div>
              ) : running?.scenario === s.id ? (
                <>
                  {running.stack_up && onReturn ? (
                    <button className="primary mm-start" onClick={onReturn}>
                      Return to console
                    </button>
                  ) : (
                    <span className="mm-note">{running.stage === 'stopping' ? 'Ending: see above.' : 'Starting: its progress is above.'}</span>
                  )}
                  {running.stage !== 'stopping' && (
                    <button className="ghost mm-again" onClick={() => setConfirm(true)}>
                      Start it again
                    </button>
                  )}
                </>
              ) : (
                <button className="primary mm-start" disabled={busy || running?.stage === 'stopping'} onClick={() => (running ? setConfirm(true) : start(false))}>
                  {busy ? 'Starting…' : scenario ? 'Start this scenario' : 'Start the open world'}
                </button>
              )}
              {!confirm && running && running.scenario !== s.id && <span className="mm-note">{running.title} is up: starting this one ends it first (you confirm).</span>}
            </div>
          )}
          {error && <p className="mm-fail">{error}</p>}
          <div className="mm-brief">
            <h3 className="mm-h">{scenario ? 'Brief' : 'What it is'}</h3>
            {s.summary ? (
              <Bluf
                parts={{ headline: s.summary.headline, bullets: s.summary.bullets, more: s.brief }}
                detail={s.brief.split(/\n\s*\n/).map((p, k) => (
                  <p key={k}>{p}</p>
                ))}
              />
            ) : (
              s.brief.split(/\n\s*\n/).map((p, k) => <p key={k}>{p}</p>)
            )}
          </div>
        </div>
      </div>
    </section>
  )
}
