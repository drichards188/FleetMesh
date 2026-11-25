/// <reference types="geojson" />
import type { Intent } from './decisions'
import { circle } from './geo'
import type { LngLat, Vehicle } from './types'

type FC = GeoJSON.FeatureCollection

export const EMPTY_PREVIEW: FC = { type: 'FeatureCollection', features: [] }

/** Where a pending intent would send its vehicles: the orbit circle or patrol loop, plus a
 *  transit line from each vehicle's current position to the first waypoint (the leg the
 *  geofence and area checks also look at). `sel` marks the one the operator is reading. */
export function previewFeatures(pending: Intent[], vehicles: Vehicle[], selectedId: number | null): FC {
  const byUid = new Map(vehicles.map((v) => [String(v.uid), v]))
  const features: GeoJSON.Feature[] = []
  for (const i of pending) {
    const sel = i.id === selectedId
    const props = { sel, label: `${i.kind.toUpperCase()} i${i.id}` }
    // `uid` on a transit line: it starts at that vehicle (MapView moves the start with the
    // vehicle's drawn position while display smoothing glides it)
    const line = (coordinates: LngLat[], part: string, uid?: number) =>
      features.push({ type: 'Feature', properties: uid == null ? { ...props, part } : { ...props, part, uid }, geometry: { type: 'LineString', coordinates } })
    const point = (p: LngLat, n: string) =>
      features.push({ type: 'Feature', properties: { ...props, part: 'point', n }, geometry: { type: 'Point', coordinates: p } })
    let first: LngLat | null = null
    const p = i.params
    if (i.kind === 'orbit' && p.center) {
      line(circle(p.center, p.radius_m ?? 200), 'path')
      point(p.center, '')
      first = p.center
    } else if (i.kind === 'patrol' && p.route && p.route.length) {
      line(p.route.length > 2 ? [...p.route, p.route[0]] : p.route, 'path')
      p.route.forEach((q, k) => point(q, String(k + 1)))
      first = p.route[0]
    } else if (i.kind === 'transit' && p.route && p.route.length) {
      // rules 34: flown once, held at its end
      if (p.route.length > 1) line(p.route, 'path')
      p.route.forEach((q, k) => point(q, k === p.route!.length - 1 ? 'H' : String(k + 1)))
      first = p.route[0]
    } else if (i.kind === 'rtl') {
      // the way home is the whole task
      for (const u of i.uids) {
        const v = byUid.get(u)
        if (v?.position && v.home) line([v.position, v.home], 'transit', v.uid)
      }
      continue
    }
    if (first) {
      for (const u of i.uids) {
        const v = byUid.get(u)
        if (v?.position) line([v.position, first], 'transit', v.uid)
      }
    }
  }
  return { type: 'FeatureCollection', features }
}

/** The points an operator should see to judge an intent: its vehicles and its task geometry. */
export function intentPoints(i: Intent, vehicles: Vehicle[]): LngLat[] {
  const byUid = new Map(vehicles.map((v) => [String(v.uid), v]))
  const pts: LngLat[] = []
  for (const u of i.uids) {
    const pos = byUid.get(u)?.position
    if (pos) pts.push(pos)
  }
  const p = i.params
  if (p.center) {
    // include the whole circle, not just its centre
    pts.push(...circle(p.center, p.radius_m ?? 200).filter((_, k) => k % 8 === 0))
  }
  if (p.route) pts.push(...p.route)
  return pts
}
