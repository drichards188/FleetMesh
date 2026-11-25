// The fleet by home base (`GET /api/fleet`, fleetModel.ts), polled every couple of seconds while a
// component holds it. A bridge without the endpoint (one started before it existed) answers 404:
// `missing`, and the console derives a coarser view itself (fleetModel.ts `fallbackFleet`).
// Read-only.
import { useSyncExternalStore } from 'react'
import type { FleetDoc } from './fleetModel'

export interface FleetFeed {
  doc: FleetDoc | null
  /** the bridge has no /api/fleet */
  missing: boolean
}

const EMPTY: FleetFeed = { doc: null, missing: false }
const POLL_MS = 2000
/** a bridge without the endpoint is asked again this seldom (a restarted bridge may have it) */
const MISSING_POLL_MS = 30_000

let feed: FleetFeed = EMPTY
const listeners = new Set<() => void>()
let timer = 0
let inFlight = false
let lastMissing = 0

async function poll() {
  if (inFlight || (feed.missing && Date.now() - lastMissing < MISSING_POLL_MS)) return
  inFlight = true
  try {
    const res = await fetch('/api/fleet')
    if (res.status === 404) {
      lastMissing = Date.now()
      if (!feed.missing) {
        feed = { doc: null, missing: true }
        for (const l of listeners) l()
      }
      return
    }
    if (!res.ok) return
    const doc = (await res.json()) as FleetDoc
    if (!Array.isArray(doc.vehicles) || !Array.isArray(doc.bases)) return
    feed = { doc: { ...doc, sites: doc.sites ?? [], states: doc.states ?? [] }, missing: false }
    for (const l of listeners) l()
  } catch {
    // unreachable for now: keep the last answer
  } finally {
    inFlight = false
  }
}

function subscribe(l: () => void) {
  listeners.add(l)
  if (listeners.size === 1) {
    void poll()
    timer = window.setInterval(() => void poll(), POLL_MS)
  }
  return () => {
    listeners.delete(l)
    if (listeners.size === 0) window.clearInterval(timer)
  }
}

/** The bridge's fleet view, refreshed every couple of seconds while any component holds it. */
export function useFleet(): FleetFeed {
  return useSyncExternalStore(subscribe, () => feed, () => EMPTY)
}
