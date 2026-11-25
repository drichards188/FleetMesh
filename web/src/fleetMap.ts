// The map organised by base (docs/design/fleet-organisation.md): the vehicles parked at their own
// base (ready, or charging) collapse into one marker per base with its counts ("Marina Green hub:
// 22 ready, 2 charging") instead of a pile of overlapping icons; zoomed out further, the bases of
// one site share one marker ("Marina: 30 ready, 2 charging"). A marker expands on a click, and every
// marker does from EXPAND_ZOOM in; an opened base keeps a hollow marker that closes it again on a
// click. Vehicles away from their base, in motion, in trouble, selected, followed or named by an
// order waiting for a person stay individual, with their routes.
//
// Pure (no browser, no deck.gl): fleetMap.check.ts runs it under node; fleetMapLayers.ts draws it.
import type { MapScreen } from './aloft'
import { textBox, type LabelCandidate } from './declutter.ts'
import { countsText, type Counts, type FleetDoc, type FleetStateId } from './fleetModel.ts'
import type { LngLat } from './types'

/** Below this zoom the bases of a site share one marker. */
export const SITE_ZOOM = 15
/** From this zoom every vehicle is drawn on its own (its pads are metres apart on the screen). */
export const EXPAND_ZOOM = 17.5
/** The states a vehicle is parked in: collapsed into its base's marker while at its own base. */
export const PARKED: ReadonlySet<FleetStateId> = new Set<FleetStateId>(['ready', 'charging'])
/** A base collapses only this many or more: one parked vehicle is drawn as itself. */
export const MIN_COLLAPSE = 2

export interface BaseMarker {
  /** `base:<hub id>` or `site:<site id>` */
  id: string
  name: string
  at: LngLat
  /** the bases it stands for (a click expands them) */
  bases: string[]
  counts: Counts
  /** the vehicles in it */
  n: number
  text: string
  /** an opened base: its vehicles are drawn one by one, the marker (hollow) closes it */
  open: boolean
}

export interface CollapseOptions {
  /** drawn on their own whatever their state: selected, followed, named by a waiting order */
  keep: ReadonlySet<number>
  /** bases the person opened on the map */
  expanded: ReadonlySet<string>
  /** the map's zoom (none yet: as zoomed out) */
  zoom: number | null
}

export interface Collapsed {
  /** the vehicles drawn inside a marker, not on their own */
  collapsed: Set<number>
  markers: BaseMarker[]
}

export const NOTHING_COLLAPSED: Collapsed = { collapsed: new Set(), markers: [] }

/** Which vehicles collapse into which marker, at this zoom. `placed`: the uids with a position. */
export function collapseAtBases(doc: FleetDoc, placed: ReadonlySet<number>, o: CollapseOptions): Collapsed {
  if (o.zoom != null && o.zoom >= EXPAND_ZOOM) return NOTHING_COLLAPSED
  const byBase = new Map<string, { uids: number[]; counts: Counts }>()
  for (const f of doc.vehicles) {
    if (!PARKED.has(f.state) || f.at_hub == null || f.at_hub !== f.base) continue
    if (o.keep.has(f.uid) || !placed.has(f.uid)) continue
    const e = byBase.get(f.at_hub) ?? { uids: [], counts: {} }
    e.uids.push(f.uid)
    e.counts[f.state] = (e.counts[f.state] ?? 0) + 1
    byBase.set(f.at_hub, e)
  }
  const collapsed = new Set<number>()
  const perBase: BaseMarker[] = []
  for (const b of doc.bases) {
    const e = byBase.get(b.id)
    if (!e || e.uids.length < MIN_COLLAPSE) continue
    const open = o.expanded.has(b.id)
    if (!open) for (const u of e.uids) collapsed.add(u)
    perBase.push({ id: `base:${b.id}`, name: b.name || b.id, at: b.at, bases: [b.id], counts: e.counts, n: e.uids.length, text: '', open })
  }
  let markers = perBase
  if (o.zoom == null || o.zoom < SITE_ZOOM) {
    markers = []
    const done = new Set<string>()
    for (const m of perBase) {
      const hub = m.bases[0]
      const site = doc.sites.find((s) => s.hubs.includes(hub))
      // an opened base keeps its own marker (the one that closes it)
      if (!site || m.open) {
        markers.push(m)
        continue
      }
      if (done.has(site.id)) continue
      done.add(site.id)
      const mine = perBase.filter((x) => site.hubs.includes(x.bases[0]) && !x.open)
      const counts: Counts = {}
      for (const x of mine) for (const [k, n] of Object.entries(x.counts)) counts[k as FleetStateId] = (counts[k as FleetStateId] ?? 0) + (n ?? 0)
      const at: LngLat = [mine.reduce((s, x) => s + x.at[0], 0) / mine.length, mine.reduce((s, x) => s + x.at[1], 0) / mine.length]
      markers.push({ id: `site:${site.id}`, name: site.name, at, bases: mine.map((x) => x.bases[0]), counts, n: mine.reduce((s, x) => s + x.n, 0), text: '', open: false })
    }
  }
  for (const m of markers) m.text = m.open ? `${m.name}: shown one by one` : `${m.name}: ${countsText(m.counts, 3, doc.states)}`
  return { collapsed, markers }
}

/** A base marker's disc radius, px: 14 for a few, about 22 for a hundred. */
export function markerRadius(m: Pick<BaseMarker, 'n'>): number {
  return 14 + 4 * Math.log10(Math.max(1, m.n))
}

/** Where a marker's label is drawn, px from its point: right of its disc, clear of the disc's own
 *  room in the label budget (each box keeps 2 px round it). */
export function labelOffset(m: Pick<BaseMarker, 'n'>): [number, number] {
  return [Math.ceil(markerRadius(m)) + 10, -8]
}

/** The markers' labels for the one label budget (declutter.ts): the disc takes its room whatever
 *  (an obstacle, as a cluster marker's), its counts beside it as an aggregate's (priority 2). */
export function baseLabelCandidates(markers: readonly BaseMarker[], screen: MapScreen): LabelCandidate[] {
  const out: LabelCandidate[] = []
  for (const m of markers) {
    const at = screen.at(m.at[0], m.at[1], 0)
    const r = markerRadius(m)
    const disc: [number, number, number, number] = [-r, -r, r, r]
    out.push({ key: `fbd:${m.id}`, at, short: disc, full: disc, priority: 2, obstacle: true, force: true })
    const box = textBox([m.text], 12, 'start', 'top', labelOffset(m), [4, 2])
    const short = textBox([m.name], 12, 'start', 'top', labelOffset(m), [4, 2])
    out.push({ key: `fb:${m.id}`, at, short, full: box, priority: 2 })
  }
  return out
}

/** A click on a marker: its bases open (or, all open already, closed again). */
export function toggleExpanded(expanded: ReadonlySet<string>, m: Pick<BaseMarker, 'bases'>): Set<string> {
  const next = new Set(expanded)
  const allOpen = m.bases.every((b) => expanded.has(b))
  for (const b of m.bases) {
    if (allOpen) next.delete(b)
    else next.add(b)
  }
  return next
}
