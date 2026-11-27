import { useCallback, useEffect, useState } from 'react'
import type { Domain } from './types'

/** The launcher (`make start`, eval/launcher.py): FleetMesh's main menu. It serves this console and
 *  starts and stops one scenario at a time (a delivery scenario, what `make start SCENARIO=<name>`
 *  runs, or the open world, what `make up` runs with tidewater's conditions); the console asks it
 *  under /launcher, which Vite proxies only when the launcher runs it. Without one (a stack started
 *  with `make up` or `make start SCENARIO=`) the probe finds nothing and the console is what it
 *  always was. */

export type Stage = 'waiting' | 'starting' | 'bridge' | 'ready' | 'running' | 'ended' | 'stopping' | 'stopped' | 'failed'
/** the drop-zone procedure: standing approvals (sim/policy.toml) or the strict comparison one
 *  (sim/policy-strict.toml, a person confirms each zone every hour) */
export type PolicyId = 'standard' | 'strict'

/** A delivery scenario's fleet at a fraction of its size (SCALE), as sim/swarm.py plans it. */
export interface ScaleOption {
  scale: number
  vehicles: number
  domains: Record<Domain, number>
}

/** A scenario of the menu: a delivery scenario (sim/scenarios/<name>/), or the open world. The
 *  launcher lists only what runs here. */
export interface Scenario {
  id: string
  kind: 'scenario' | 'open_world'
  default: boolean
  title: string
  brief: string
  /** a headline and a few bullets for the brief (a scenario's `[summary]`), when it has one */
  summary?: { headline: string; bullets: string[] } | null
  runnable: boolean
  why_not: string | null
  /** at full size */
  vehicles?: number
  /** how many of them run in ArduPilot SITL (the rest are stand-ins) */
  sitl?: number
  platform_types?: number
  /** how many of them in each domain (air, ground, surface) */
  domains?: Record<Domain, number>
  /** how long vehicles usually take to be ready (s) */
  typical_s?: [number, number]
  /** the tidewater scenario, or "none" */
  tidewater?: string
  at?: string
  /** scenario time (s); null: the open world runs until it is ended */
  duration_s?: number | null
  /** a delivery scenario's options: its fleet at a fraction of its size, its clock rates (its
   *  own among them), its drop-zone procedures; and its own defaults */
  scales?: ScaleOption[]
  rate?: number
  rates?: number[]
  policy?: PolicyId
  policies?: PolicyId[]
  /** simulated air and sea traffic around the fleet */
  traffic?: boolean
  /** the same from a terminal */
  cli?: string
}

/** What a delivery scenario was started with. */
export interface LaunchOptions {
  scale?: number
  rate?: number
  policy?: PolicyId
}

/** One launch: its stages as the launcher saw them, and its end. */
export interface Launch {
  id: string
  scenario: string
  kind: 'scenario' | 'open_world'
  title: string
  options: LaunchOptions
  cli: string
  stage: Stage
  detail: string
  /** why it failed */
  error: string | null
  /** what was still held after it ended (ports, processes) */
  leftovers: string[]
  started_t: number
  stage_t: number
  history: { stage: Stage; t: number }[]
  ready: { ready: number; of: number } | null
  typical_s: [number, number]
  /** its command's output */
  log: string
  bridge_port: number
  /** a delivery scenario's clock as the bridge last said it (s) */
  clock: { elapsed_s: number; duration_s: number | null } | null
  /** the bridge answers: the console can show it */
  stack_up: boolean
}

/** How the launcher serves this console (eval/webconsole.py): the production build or the dev server. */
export type ConsoleMode = 'build' | 'dev'
export interface ConsoleState {
  /** the choice, kept across restarts */
  mode: ConsoleMode
  /** what the running server is (null while it is swapped) */
  serving: ConsoleMode | null
  /** a switch under way */
  working: 'building' | 'restarting' | null
  /** counts server starts: a page that sees it change reloads itself */
  generation: number
  /** the last build: how long it took (s), when it finished (s, epoch), why it failed */
  build: { state: 'none' | 'building' | 'done' | 'failed'; seconds: number | null; at: number | null; started: number | null; error: string | null }
  /** the sources changed since the build (null when no build matters) */
  stale: boolean | null
  error: string | null
}

export interface LauncherState {
  launcher: { console_port: number; api_port: number }
  console?: ConsoleState
  scenarios: Scenario[]
  current: Launch | null
  /** the launcher's clock (s) */
  now: number
}

export interface StartRequest {
  scenario: string
  scale?: number
  rate?: number
  policy?: PolicyId
  /** end the running scenario first */
  replace?: boolean
}

const ACTIVE: Stage[] = ['waiting', 'starting', 'bridge', 'ready', 'running', 'ended', 'stopping']
/** a launch still under way or still up (anything but stopped or failed) */
export const isActive = (l: Launch | null | undefined): l is Launch => !!l && ACTIVE.includes(l.stage)
/** a launch on its way up or down: the menu polls faster */
const moving = (l: Launch | null | undefined) => !!l && ['waiting', 'starting', 'bridge', 'ready', 'stopping'].includes(l.stage)

/** The launcher's state, or null when no launcher answers (without one, Vite serves its own
 *  index.html for the path, or nothing). */
export async function probeLauncher(timeoutMs = 2500): Promise<LauncherState | null> {
  const ctl = new AbortController()
  const timer = window.setTimeout(() => ctl.abort(), timeoutMs)
  try {
    const res = await fetch('/launcher/api/state', { signal: ctl.signal, cache: 'no-store' })
    if (!res.ok || !(res.headers.get('content-type') ?? '').includes('application/json')) return null
    const body = await res.json()
    return body && Array.isArray(body.scenarios) ? (body as LauncherState) : null
  } catch {
    return null
  } finally {
    window.clearTimeout(timer)
  }
}

async function post(path: 'start' | 'end', body: object): Promise<Launch | null> {
  const res = await fetch(`/launcher/api/${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  const out = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(out.detail ?? `the launcher answered ${res.status}`)
  return out.current ?? null
}

export const startScenario = (req: StartRequest) => post('start', req)
export const endScenario = () => post('end', {})

/** Choose how the console is served; the launcher refuses while a scenario is under way. */
export async function setConsoleMode(mode: ConsoleMode): Promise<ConsoleState> {
  const res = await fetch('/launcher/api/console', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ mode }),
  })
  const out = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(out.detail ?? `the launcher answered ${res.status}`)
  return out.console
}

/** The launcher's state, polled: every second while a launch comes up or goes down, every 3 s
 *  otherwise. `received` is this browser's clock (ms) when it came; `lost` says the launcher
 *  stopped answering (the last state stays). */
export function useLauncher(initial: LauncherState) {
  const [state, setState] = useState(initial)
  const [received, setReceived] = useState(() => Date.now())
  const [lost, setLost] = useState(false)
  // the console server this page came from (a different one later: reload)
  const [gen0] = useState(initial.console?.generation)
  // bumped to look again at once (after a start or an end)
  const [kick, setKick] = useState(0)
  useEffect(() => {
    let stop = false
    let timer: number | undefined
    const poll = async () => {
      const s = await probeLauncher()
      if (stop) return
      if (s) {
        // the console server was swapped (build or dev): this page came from the old one
        if (s.console && gen0 !== undefined && s.console.generation !== gen0 && !s.console.working) {
          window.location.reload()
          return
        }
        setState(s)
        setReceived(Date.now())
      }
      setLost(!s)
      timer = window.setTimeout(poll, s && !moving(s.current) && !s.console?.working ? 3000 : 1000)
    }
    timer = window.setTimeout(poll, kick ? 0 : 1000)
    return () => {
      stop = true
      window.clearTimeout(timer)
    }
  }, [kick, gen0])
  /** a launch the launcher just answered with, shown before the next look */
  const update = useCallback((current: Launch | null) => {
    if (current) setState((s) => ({ ...s, current }))
    setKick((n) => n + 1)
  }, [])
  return { state, received, lost, update }
}
