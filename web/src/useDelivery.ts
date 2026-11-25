// The delivery feed: parcels, hubs, drop zones and each vehicle's navigation trust, read from the
// bridge (`/api/parcels`, `/api/places`, `/api/nav`) and shared by everything that shows them (the
// map's layers, the Parcels panel, the Inbox cards, the vehicle details): one poll for all of them,
// running only while something uses it. Read-only; a bridge without delivery gives empty lists.
import { useSyncExternalStore } from 'react'
import { EMPTY_NAV, EMPTY_PARCELS, EMPTY_PLACES, type NavDoc, type ParcelsDoc, type PlacesDoc } from './delivery'

export interface DeliveryFeed {
  parcels: ParcelsDoc
  places: PlacesDoc
  nav: NavDoc
  /** a delivery run: the bridge reported a scenario, parcels, hubs or drop zones */
  active: boolean
  /** this browser's clock (ms) when the parcels last answered */
  at: number
}

const EMPTY: DeliveryFeed = { parcels: EMPTY_PARCELS, places: EMPTY_PLACES, nav: EMPTY_NAV, active: false, at: 0 }
const POLL_MS = 2000

let feed: DeliveryFeed = EMPTY
const listeners = new Set<() => void>()
let timer = 0
let inFlight = false

async function get<T>(url: string): Promise<T | null> {
  try {
    const res = await fetch(url)
    return res.ok ? ((await res.json()) as T) : null
  } catch {
    return null
  }
}

async function poll() {
  if (inFlight) return
  inFlight = true
  try {
    const [parcels, places, nav] = await Promise.all([get<ParcelsDoc>('/api/parcels'), get<PlacesDoc>('/api/places'), get<NavDoc>('/api/nav')])
    if (!parcels && !places && !nav) return
    const p = parcels ?? feed.parcels
    const pl = places ?? feed.places
    feed = {
      parcels: p,
      places: pl,
      nav: nav ?? feed.nav,
      active: p.scenario != null || p.parcels.length > 0 || pl.hubs.length > 0 || pl.drop_zones.length > 0,
      at: Date.now(),
    }
    for (const l of listeners) l()
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

/** The delivery feed, refreshed every couple of seconds while any component holds it. */
export function useDelivery(): DeliveryFeed {
  return useSyncExternalStore(subscribe, () => feed, () => EMPTY)
}
