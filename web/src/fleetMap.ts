// The map organised by base (docs/design/fleet-organisation.md): every hub has one mark of its own,
// a house (the home its vehicles come back to: a shape nothing else on the map has), its roof the
// colour of its state and its name with one line under it ("2 need you · Marginal: gusts 12.5 m/s
// · 18 holding", hubSummary.ts). The vehicles parked at their own base (ready, or charging) collapse
// into the mark, their number on it, instead of a pile of overlapping icons; zoomed out further, the
// hubs of one site share one mark. From EXPAND_ZOOM in every vehicle is drawn on its own and the
// mark stays; a hub's details can open its vehicles at any zoom (the mark then hollow). Vehicles
// away from their base, in motion, in trouble, selected, followed or named by an order waiting for
// a person stay individual, with their routes.
//
// Pure (no browser, no deck.gl): fleetMap.check.ts runs it under node; fleetMapLayers.ts draws it.
import type { MapScreen } from './aloft'
import { textBox, type LabelCandidate } from './declutter.ts'
import { countsText, type Counts, type FleetDoc, type FleetStateId } from './fleetModel.ts'
import { hubView, siteView, type HubView } from './hubSummary.ts'
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
  /** the bases it stands for */
  bases: string[]
  /** the counts of the vehicles collapsed into it */
  counts: Counts
  /** the vehicles collapsed into it (0: none, each drawn on its own) */
  n: number
  /** in a line, for its tooltip: its name and its state */
  text: string
  /** an opened base: its vehicles are drawn one by one, the mark hollow */
  open: boolean
  /** its state (hubSummary.ts): the roof's tone, the line under its name, who needs the person */
  view: HubView
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

/** Which vehicles collapse into which mark, at this zoom, and every hub's mark (or its site's,
 *  zoomed out). `placed`: the uids with a position. */
export function collapseAtBases(doc: FleetDoc, placed: ReadonlySet<number>, o: CollapseOptions): Collapsed {
  const expandAll = o.zoom != null && o.zoom >= EXPAND_ZOOM
  const byBase = new Map<string, { uids: number[]; counts: Counts }>()
  if (!expandAll)
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
    const gathers = e != null && e.uids.length >= MIN_COLLAPSE
    const open = gathers && o.expanded.has(b.id)
    if (gathers && !open) for (const u of e.uids) collapsed.add(u)
    perBase.push({ id: `base:${b.id}`, name: b.name || b.id, at: b.at, bases: [b.id], counts: gathers && !open ? e.counts : {}, n: gathers && !open ? e.uids.length : 0, text: '', open, view: hubView(b) })
  }
  let markers = perBase
  if (o.zoom == null || o.zoom < SITE_ZOOM) {
    markers = []
    const done = new Set<string>()
    for (const m of perBase) {
      const hub = m.bases[0]
      const site = doc.sites.find((s) => s.hubs.includes(hub))
      // an opened base keeps its own mark
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
      const view = siteView(site.id, site.name, doc.bases.filter((b) => mine.some((x) => x.bases[0] === b.id)))
      markers.push({ id: `site:${site.id}`, name: site.name, at, bases: mine.map((x) => x.bases[0]), counts, n: mine.reduce((s, x) => s + x.n, 0), text: '', open: false, view })
    }
  }
  for (const m of markers) m.text = m.open ? `${m.name}: its vehicles shown one by one` : `${m.name}: ${m.view.line}`
  return { collapsed, markers }
}

/** What is collapsed into a mark, in words ("22 ready, 2 charging"), for its tooltip. */
export function parkedText(m: Pick<BaseMarker, 'counts'>, doc: Pick<FleetDoc, 'states'>): string {
  return countsText(m.counts, 3, doc.states)
}

/** A hub's mark, px: a house standing on a short stem over the hub's point, like a pin, so the
 *  vehicles parked at the point never cover it (they stay visible, and clickable, at its foot). The
 *  house's width and height, how far its roof's apex and its floor are from the middle of its walls
 *  (where the number of its parked vehicles is written), and how far that middle is above the
 *  hub's point. */
export const BADGE = { w: 28, h: 34, top: 20, bottom: 12, lift: 26 } as const

/** The mark's room, px from the hub's point: the house, its stem and its foot. */
export const MARK_BOX: [number, number, number, number] = [-BADGE.w / 2, -BADGE.lift - BADGE.top, BADGE.w / 2, 3]

/** Its needs-you count, px from the hub's point: on the roof's right eave. */
export const NEED_AT: [number, number] = [14, -BADGE.lift - 15]

/** Where a mark's label is drawn, px from the hub's point: right of the house, its name level with
 *  the roof and the line under it level with the walls. */
export function labelOffset(): [number, number] {
  return [BADGE.w / 2 + 9, -BADGE.lift - BADGE.top + 2]
}

/** The marks' labels for the one label budget (declutter.ts): the house takes its room whatever
 *  (an obstacle, as a cluster marker's), its name and line beside it as an aggregate's (priority
 *  2): the name alone where there is no room for both. */
export function baseLabelCandidates(markers: readonly BaseMarker[], screen: MapScreen): LabelCandidate[] {
  const out: LabelCandidate[] = []
  for (const m of markers) {
    const at = screen.at(m.at[0], m.at[1], 0)
    out.push({ key: `fbd:${m.id}`, at, short: MARK_BOX, full: MARK_BOX, priority: 2, obstacle: true, force: true })
    const full = textBox([m.name, labelLine(m)], 12, 'start', 'top', labelOffset(), [4, 2])
    const short = textBox([m.name], 12, 'start', 'top', labelOffset(), [4, 2])
    out.push({ key: `fb:${m.id}`, at, short, full, priority: 2 })
  }
  return out
}

/** The line under a mark's name. */
export function labelLine(m: Pick<BaseMarker, 'open' | 'view'>): string {
  return m.open ? 'vehicles shown one by one' : m.view.line
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
