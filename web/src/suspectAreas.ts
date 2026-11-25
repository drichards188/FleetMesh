// Suspected interference and link-outage areas, drawn from what vehicles report (docs/design/
// link-and-gnss.md section 8): the console does not know where a jammer is, nor an event's name, and
// must not show it. It shows where vehicles are affected: those whose navigation is degraded or
// untrusted (the navigation monitor's own verdict) or whose link is down or degraded (the link
// emulator's state as the bridge reports it), joined into an area when they are near each other.
// An area is a symptom's extent (a circle around the affected vehicles), labelled as suspected.
// Display only. Pure: suspectAreas.check.ts.
import type { NavDoc } from './delivery.ts'
import type { LngLat } from './types.ts'

export type AreaKind = 'gnss' | 'link_down' | 'link_degraded'

export interface SuspectArea {
  kind: AreaKind
  /** centre, lon lat */
  at: LngLat
  radius_m: number
  uids: number[]
  /** how it reads on the map and in a hover */
  label: string
}

export interface AreaVehicle {
  uid: number
  position: LngLat | null
  link: { emu?: { state: string; cause: string | null } | null }
}

/** Vehicles closer than this join one area, m. */
export const JOIN_M = 1500
/** An area is at least this wide around one vehicle, m, and reaches this far past the outermost. */
export const MIN_RADIUS_M = 300
export const MARGIN_M = 250

const R = 6_371_000
function dist(a: LngLat, b: LngLat): number {
  const rad = Math.PI / 180
  const dLat = (b[1] - a[1]) * rad
  const dLon = (b[0] - a[0]) * rad
  const h = Math.sin(dLat / 2) ** 2 + Math.cos(a[1] * rad) * Math.cos(b[1] * rad) * Math.sin(dLon / 2) ** 2
  return 2 * R * Math.asin(Math.min(1, Math.sqrt(h)))
}

/** Single-linkage groups of points closer than `joinM`. */
function groups(items: { uid: number; at: LngLat }[], joinM: number): { uid: number; at: LngLat }[][] {
  const out: { uid: number; at: LngLat }[][] = []
  const left = [...items]
  while (left.length) {
    const g = [left.pop()!]
    for (let k = 0; k < g.length; k++)
      for (let j = left.length - 1; j >= 0; j--)
        if (dist(g[k].at, left[j].at) <= joinM) g.push(...left.splice(j, 1))
    out.push(g)
  }
  return out
}

function areaOf(kind: AreaKind, g: { uid: number; at: LngLat }[], word: string): SuspectArea {
  const lon = g.reduce((s, x) => s + x.at[0], 0) / g.length
  const lat = g.reduce((s, x) => s + x.at[1], 0) / g.length
  const at: LngLat = [lon, lat]
  const far = Math.max(0, ...g.map((x) => dist(at, x.at)))
  const uids = g.map((x) => x.uid).sort((a, b) => a - b)
  return { kind, at, radius_m: Math.max(MIN_RADIUS_M, far + MARGIN_M), uids, label: `${word}: ${g.length} vehicle${g.length === 1 ? '' : 's'}` }
}

/** The areas the fleet's symptoms suggest. A fault that touches most of a larger fleet at once
 *  (the first seconds of a run, before any vehicle has a fix) is not a place and gives no area. */
export function suspectAreas(vehicles: readonly AreaVehicle[], nav: NavDoc): SuspectArea[] {
  const gnss: { uid: number; at: LngLat }[] = []
  const down: { uid: number; at: LngLat }[] = []
  const slow: { uid: number; at: LngLat }[] = []
  for (const v of vehicles) {
    if (!v.position) continue
    const s = nav.vehicles[String(v.uid)]?.state
    if (s === 'degraded' || s === 'untrusted') gnss.push({ uid: v.uid, at: v.position })
    const e = v.link.emu?.state
    if (e === 'down') down.push({ uid: v.uid, at: v.position })
    else if (e === 'degraded') slow.push({ uid: v.uid, at: v.position })
  }
  const local = (xs: unknown[]) => !(vehicles.length >= 6 && xs.length > 0.8 * vehicles.length)
  const out: SuspectArea[] = []
  if (local(gnss)) for (const g of groups(gnss, JOIN_M)) out.push(areaOf('gnss', g, 'Suspected GNSS interference'))
  if (local(down)) for (const g of groups(down, JOIN_M)) out.push(areaOf('link_down', g, 'Link outage'))
  if (local(slow)) for (const g of groups(slow, JOIN_M)) out.push(areaOf('link_degraded', g, 'Link degraded'))
  return out
}
