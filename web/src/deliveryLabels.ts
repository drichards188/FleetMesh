// The delivery layers' text for the one label budget (declutter.ts). Pure; deliveryLayers.ts draws it.
import type { MapScreen } from './aloft.ts'
import { textBox, type LabelCandidate } from './declutter.ts'
import type { Hub } from './delivery.ts'
import type { SuspectArea } from './suspectAreas.ts'

export const hubText = (h: Hub, full: boolean) => (full ? `${h.name}  ${h.packs} pack${h.packs === 1 ? '' : 's'}` : h.name)
export const areaText = (a: SuspectArea, full: boolean) => (full ? `${a.label} (suspected)` : a.label)
/** Where a hub's name is drawn, px from its point: up and right of its ring (the ring is
 *  `radius_m` across, at least 9 px), so the name clears the vehicles on its pads. */
export function hubOffset(h: Hub, screen: MapScreen | null): [number, number] {
  let r = 9
  const a = screen?.at(h.at[0], h.at[1], 0)
  const dLon = h.radius_m / (111_320 * Math.cos((h.at[1] * Math.PI) / 180))
  const b = screen?.at(h.at[0] + dLon, h.at[1], 0)
  if (a && b) r = Math.max(9, Math.hypot(b[0] - a[0], b[1] - a[1]))
  return [Math.round(r * 0.7 + 6), -Math.round(r * 0.7 + 4)]
}
export const areaKey = (a: SuspectArea) => `da:${a.kind}:${a.uids.join()}`

/** The delivery layers' text for the one label budget (declutter.ts): a hub's name (with its packs
 *  when there is room) comes before the vehicles' labels around it, so the hub's neighbours give
 *  way and two labels never print on top of each other. Hub names hold their place down to a wide
 *  view; the area names, longer and many, only from the usual label zoom. */
export function deliveryLabelCandidates(d: { hubs: Hub[]; areas: SuspectArea[] }, on: { hubs: boolean; areas: boolean }, screen: MapScreen): LabelCandidate[] {
  const out: LabelCandidate[] = []
  if (on.hubs)
    for (const h of d.hubs) {
      const box = (full: boolean) => (() => { const [dx, dy] = hubOffset(h, screen); return textBox([hubText(h, full)], 13, 'start', 'top', [dx, dy - 7], [0, 0], 2) })()
      out.push({ key: `dh:${h.id}`, at: screen.at(h.at[0], h.at[1], 0), short: box(false), full: box(true), priority: 2 })
    }
  if (on.areas)
    for (const a of d.areas) {
      const box = (full: boolean) => textBox([areaText(a, full)], 12, 'middle', 'top', [0, -6], [0, 0], 2)
      out.push({ key: areaKey(a), at: screen.at(a.at[0], a.at[1], 0), short: box(false), full: box(true), priority: 3 })
    }
  return out
}

