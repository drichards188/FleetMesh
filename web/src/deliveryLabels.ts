// The delivery layers' text for the one label budget (declutter.ts). Pure; deliveryLayers.ts draws it.
import type { MapScreen } from './aloft.ts'
import { textBox, textWidth, type LabelCandidate } from './declutter.ts'
import type { ParcelPlace } from './delivery.ts'
import type { SuspectArea } from './suspectAreas.ts'

export const areaText = (a: SuspectArea, full: boolean) => (full ? `${a.label} (suspected)` : a.label)
/** A ring's radius on the screen now, px: `radius_m` round `at`, at least `min`. */
function ringPx(at: [number, number], radius_m: number, min: number, screen: MapScreen | null): number {
  const a = screen?.at(at[0], at[1], 0)
  const dLon = radius_m / (111_320 * Math.cos((at[1] * Math.PI) / 180))
  const b = screen?.at(at[0] + dLon, at[1], 0)
  return a && b ? Math.max(min, Math.hypot(b[0] - a[0], b[1] - a[1])) : min
}
/** A place's parcel mark, px high, and the number beside it, px. */
export const PARCEL_MARK_PX = 16
export const PARCEL_COUNT_PX = 12
/** Where a place's parcel mark is drawn, px from the place's point: under a hub's point (its house
 *  stands above it, its name and state right of the house), right of a drop zone's dot (5 to 14 px), down and right of a
 *  vehicle's point (clear of its symbol on the ground). */
export function parcelOffset(pl: Pick<ParcelPlace, 'kind' | 'at' | 'radius_m'>, screen: MapScreen | null): [number, number] {
  if (pl.kind === 'vehicle') return [14, 14]
  if (pl.kind === 'zone') return [Math.round(Math.min(14, ringPx(pl.at, pl.radius_m, 5, screen)) + 10), 0]
  // under the hub's point (its house stands above it, its label beside the house)
  return [0, PARCEL_MARK_PX / 2 + 5]
}
/** The number beside a place's mark: how many parcels are there. None for a vehicle's one (a
 *  vehicle carries one; the mark says it). */
export const parcelCountText = (pl: Pick<ParcelPlace, 'kind' | 'parcels'>) => (pl.kind === 'vehicle' && pl.parcels.length === 1 ? '' : String(pl.parcels.length))
/** Where the number is drawn, px from the place's point: right of the mark. */
export function parcelCountOffset(off: [number, number]): [number, number] {
  return [off[0] + PARCEL_MARK_PX / 2 + 3, off[1]]
}
export const parcelKey = (pl: Pick<ParcelPlace, 'key'>) => `dp:${pl.key}`

export const areaKey = (a: SuspectArea) => `da:${a.kind}:${a.uids.join()}`

/** The delivery layers' text for the one label budget (declutter.ts). A place's parcel count (with
 *  its mark) is an aggregate like a hub's name: it keeps its room before the vehicles'. The area
 *  names, longer and many, come only from the usual label zoom. (A hub's name and state are its
 *  mark's: fleetMap.ts.) */
export function deliveryLabelCandidates(
  d: { areas: SuspectArea[]; places?: ParcelPlace[] },
  on: { areas: boolean; parcels?: boolean },
  screen: MapScreen,
): LabelCandidate[] {
  const out: LabelCandidate[] = []
  if (on.parcels)
    for (const pl of d.places ?? []) {
      const text = parcelCountText(pl)
      if (!text) continue
      const off = parcelOffset(pl, screen)
      const [tx] = parcelCountOffset(off)
      const half = PARCEL_MARK_PX / 2
      const box: [number, number, number, number] = [off[0] - half, off[1] - half - 1, tx + textWidth(text, PARCEL_COUNT_PX) + 2, off[1] + half + 1]
      out.push({ key: parcelKey(pl), at: screen.at(pl.at[0], pl.at[1], 0), short: box, full: box, priority: 2 })
    }
  if (on.areas)
    for (const a of d.areas) {
      const box = (full: boolean) => textBox([areaText(a, full)], 12, 'middle', 'top', [0, -6], [0, 0], 2)
      out.push({ key: areaKey(a), at: screen.at(a.at[0], a.at[1], 0), short: box(false), full: box(true), priority: 3 })
    }
  return out
}
