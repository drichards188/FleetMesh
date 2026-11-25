// The traffic picture from the bridge (docs/design/traffic.md section 5): the WebSocket's
// `{"type": "traffic"}` push at about 1 Hz (useBridge hands it here), GET /api/traffic once at
// the start and as a fallback while no push comes, the receivers and sensors (GET
// /api/traffic/sensors), and, in a dev run only, the simulator's truth (GET /api/traffic/truth).
// A bridge without the traffic picture answers 404: the layer then says so and asks no more.
import { useEffect, useState, useSyncExternalStore } from 'react'
import { perfPush } from './perf'
import type { Picture, Track, TrafficFrame, TrafficSensor, TruthObject } from './traffic'

/** What the tracker paired as a disagreement, when the bridge passes it on (optional). */
export interface Disagreement {
  track: string
  sensed: string
  offset_m?: number
  sensor?: string
}

export type TrafficPicture = TrafficFrame & { disagreements?: Disagreement[] }

const OK_PICTURE: Picture = { state: 'ok', reasons: [], feeds: {} }

/** A message or an answer as the console keeps it: every part defaulted. */
export function normalizeFrame(m: unknown): TrafficPicture | null {
  if (!m || typeof m !== 'object') return null
  const o = m as Partial<TrafficPicture>
  if (!Array.isArray(o.tracks)) return null
  const p = (o.picture ?? OK_PICTURE) as Partial<Picture>
  return {
    t: typeof o.t === 'number' ? o.t : Date.now(),
    tracks: o.tracks.filter((t): t is Track => !!t && typeof t.id === 'string' && Number.isFinite(t.lat) && Number.isFinite(t.lon)).map((t) => ({ ...t, flags: t.flags ?? [], sources: t.sources ?? [], static: t.static ?? {}, ids: t.ids ?? { anonymous: false } })),
    picture: { state: p.state === 'degraded' ? 'degraded' : 'ok', reasons: Array.isArray(p.reasons) ? p.reasons : [], feeds: p.feeds && typeof p.feeds === 'object' ? p.feeds : {} },
    disagreements: Array.isArray(o.disagreements) ? o.disagreements : undefined,
  }
}

/** The truth as the bridge serves it: a list, or under `truth` or `objects`. */
export function normalizeTruth(m: unknown): TruthObject[] | null {
  const list = Array.isArray(m) ? m : m && typeof m === 'object' ? ((m as { truth?: unknown; objects?: unknown }).truth ?? (m as { objects?: unknown }).objects) : null
  if (!Array.isArray(list)) return null
  return list.filter((o): o is TruthObject => !!o && Number.isFinite(o.lat) && Number.isFinite(o.lon))
}

// ---- the store: one picture for the console ------------------------------------------------------

let current: TrafficPicture | null = null
/** when the last push came (performance.now), so the fallback poll knows to stay quiet */
let pushedAt = -Infinity
const listeners = new Set<() => void>()
const emit = () => listeners.forEach((l) => l())

/** useBridge: a `traffic` message from the socket. */
export function pushTraffic(msg: unknown) {
  const t0 = performance.now()
  const f = normalizeFrame(msg)
  if (!f) return
  current = f
  pushedAt = performance.now()
  perfPush('trafficMerge', pushedAt - t0)
  emit()
}

function setFromFetch(f: TrafficPicture) {
  current = f
  emit()
}

/** The picture now (null before the first), and whether the bridge has one at all: null not
 *  known yet, false when it answered 404. */
export function useTraffic(enabled = true): { picture: TrafficPicture | null; available: boolean | null } {
  const picture = useSyncExternalStore(
    (cb) => {
      listeners.add(cb)
      return () => listeners.delete(cb)
    },
    () => current,
  )
  const [available, setAvailable] = useState<boolean | null>(null)
  useEffect(() => {
    if (!enabled) return
    let on = true
    // the bridge said it has no picture: no more asking (a push still shows one if it comes)
    let none = false
    const load = async () => {
      // the socket pushes it: no need to ask
      if (none || performance.now() - pushedAt < 3000) return
      try {
        const res = await fetch('/api/traffic')
        if (!on) return
        if (res.status === 404) {
          none = true
          setAvailable(false)
          return
        }
        if (!res.ok) return
        const f = normalizeFrame(await res.json())
        if (f && on && performance.now() - pushedAt >= 3000) setFromFetch(f)
        if (on) setAvailable(true)
      } catch {
        // unreachable for now: the connection pill says so; ask again
      }
    }
    void load()
    const id = window.setInterval(() => void load(), 2000)
    return () => {
      on = false
      window.clearInterval(id)
    }
  }, [enabled])
  return { picture, available: picture ? true : available }
}

/** The company's receivers and sensors: asked at the start and every 15 s (the onboard ones move
 *  with our vehicles). */
export function useTrafficSensors(enabled: boolean): TrafficSensor[] | null {
  const [sensors, setSensors] = useState<TrafficSensor[] | null>(null)
  useEffect(() => {
    if (!enabled) return
    let on = true
    const load = async () => {
      try {
        const res = await fetch('/api/traffic/sensors')
        if (!res.ok) return
        const body = (await res.json()) as unknown
        const list = Array.isArray(body) ? body : (body as { sensors?: unknown })?.sensors
        if (on && Array.isArray(list)) setSensors(list as TrafficSensor[])
      } catch {
        // keep the last list
      }
    }
    void load()
    const id = window.setInterval(() => void load(), 15_000)
    return () => {
      on = false
      window.clearInterval(id)
    }
  }, [enabled])
  return sensors
}

/** The simulator's truth, once a second while `on` (a dev run with the overlay on): never asked
 *  otherwise. `available` false once the bridge says it has none (404, 403). */
export function useTrafficTruth(on: boolean): { truth: TruthObject[] | null; available: boolean | null } {
  const [truth, setTruth] = useState<TruthObject[] | null>(null)
  const [available, setAvailable] = useState<boolean | null>(null)
  useEffect(() => {
    // off: nothing asked (App draws none while off, so the last answer need not be cleared)
    if (!on) return
    let alive = true
    const load = async () => {
      try {
        const res = await fetch('/api/traffic/truth')
        if (!alive) return
        if (res.status === 404 || res.status === 403) {
          setAvailable(false)
          setTruth(null)
          return
        }
        if (!res.ok) return
        const list = normalizeTruth(await res.json())
        if (alive && list) {
          setTruth(list)
          setAvailable(true)
        }
      } catch {
        // ask again in a second
      }
    }
    void load()
    const id = window.setInterval(() => void load(), 1000)
    return () => {
      alive = false
      window.clearInterval(id)
    }
  }, [on])
  return { truth, available }
}
