// The traffic layer on the map: the traffic the fleet does not control as the company's tracker
// sees it (traffic.ts), never as ground truth. deck.gl layers for MapView's `layers` prop; display
// only.
//
//   traffic-coverage        the company's receivers' nominal reach (the bar's Coverage): rings,
//                           shaded for the short-range ones, red where a receiver is down
//   traffic-wedges, -lines  a bearing-only track: the wedge it can be in (its range interval, two
//                           bearing sigmas either side) and its line of bearing, on the surface
//                           (where it can be, seen from above); never a point. Selected or
//                           hovered, traffic-sight adds the line of sight in 3D, rising with its
//                           elevation from the sensor on our vehicle
//   traffic-stems           an aircraft drawn at its height: a thin stem to the surface, as ours
//   traffic-leaders         the speed leader: where it will be in 30 s (aircraft) or 60 s (vessels)
//   traffic-predict(-sigma) the selected or hovered track's predicted path (constant velocity,
//                           the tracker's own model), ticks with its growing 1-sigma rings
//   traffic-disagree        a broadcast the company's sensors contradict, joined to where they see it
//   traffic-alarm           a red ring round a flagged track (sensor_disagree, position_jump,
//                           emergency); traffic-selected the selected one's ring
//   traffic-icons           the icon by class (trafficSymbols.ts), turned to its course, sized and
//                           faded by how much it matters and how fresh it is (traffic.ts declutter)
//   traffic-labels          its name, and in full its altitude or speed: only where the label budget
//                           placed it (declutter.ts), only for the near ones and the flagged
//   traffic-truth(-links, -names)  dev runs only, off by default: the simulator's truth beside it
//
// Heights: an aircraft below FLATTEN_ALT_M is drawn at its altitude (twice over, as ours,
// map3d.ts), at its foot while that is above the view (aloft.ts); everything else on the surface.
import { IconLayer, LineLayer, PathLayer, PolygonLayer, ScatterplotLayer, TextLayer } from '@deck.gl/layers'
import type { MapScreen } from './aloft'
import { type LabelCandidate, type LabelPlan, ALL_FULL, rightOf, textBox } from './declutter'
import { circle } from './geo'
import { HEIGHT_EXAGGERATION, NO_NEAR_GROWTH } from './map3d'
import { perfPush } from './perf'
import { bearingWedge, courseOf, drawAltM, isBearingOnly, leaderEnd, predictPath, type ShownTrack, type Track, type TrafficSensor, type TruthMatch } from './traffic'
import { classWords, distText, labelLines, sourceKindWords, trackTitle } from './trafficText'
import { AIR_TRAFFIC_RGB, ICON_PX, orients, SURFACE_TRAFFIC_RGB, trafficIcon } from './trafficSymbols'
import type { LngLat } from './types'

type RGBA = [number, number, number, number]
type XYZ = [number, number, number]

export const TRAFFIC_LAYER = {
  coverage: 'traffic-coverage',
  coverageNames: 'traffic-coverage-names',
  wedges: 'traffic-wedges',
  lines: 'traffic-bearing-lines',
  sight: 'traffic-sight',
  stems: 'traffic-stems',
  leaders: 'traffic-leaders',
  predict: 'traffic-predict',
  predictSigma: 'traffic-predict-sigma',
  disagree: 'traffic-disagree',
  alarm: 'traffic-alarm',
  selected: 'traffic-selected',
  icons: 'traffic-icons',
  labels: 'traffic-labels',
  truth: 'traffic-truth',
  truthLinks: 'traffic-truth-links',
  truthNames: 'traffic-truth-names',
} as const

/** layer ids whose object is a ShownTrack: a click selects it, the pointer over it hovers it */
export const TRAFFIC_PICK_LAYERS: string[] = [TRAFFIC_LAYER.icons, TRAFFIC_LAYER.wedges, TRAFFIC_LAYER.labels]

const OUTLINE: RGBA = [11, 15, 20, 255]
const BAD: RGBA = [255, 93, 93, 255]
const ACCENT: RGBA = [255, 179, 71, 255]
/** the simulator's truth: a colour nothing else on the map uses */
export const TRUTH_RGB: [number, number, number] = [255, 92, 214]
const FONT = 'IBM Plex Mono, ui-monospace, monospace'
const FONT_SETTINGS = { sdf: true, fontSize: 48, buffer: 6 }
const onTop = { depthCompare: 'always' as const, depthWriteEnabled: false }
/** coverage rings larger than this are not drawn: they reach past any view of the bay (the bar
 *  lists them) */
export const COVERAGE_MAX_M = 30_000

const rgb = (t: Pick<Track, 'domain'>) => (t.domain === 'air' ? AIR_TRAFFIC_RGB : SURFACE_TRAFFIC_RGB)
const withA = (c: [number, number, number], a: number): RGBA => [c[0], c[1], c[2], Math.round(Math.max(0, Math.min(1, a)) * 255)]

export interface TrafficLayerInput {
  shown: ShownTrack[]
  /** the picture's time, Unix ms */
  pictureT: number
  selected: string | null
  hovered: string | null
  pairs: { broadcast: Track; sensed: Track }[]
  /** the receivers to draw the reach of (the Coverage toggle), or null */
  coverage: TrafficSensor[] | null
  /** dev runs only, the overlay on: truth matched to the picture */
  truth: TruthMatch[] | null
}

// ---- where each track is drawn at this camera ----------------------------------------------------

interface Placement {
  /** rendered metres (the map's exaggeration applied): its height, or 0 at its foot */
  z: Map<string, number>
  /** the icon's turn, degrees counter-clockwise on the screen (deck.gl's getAngle) */
  angle: Map<string, number>
  /** bumped whenever a z or an angle changes: the layers' update trigger */
  version: number
}

let placed: { shown: ShownTrack[]; z: Map<string, number>; angle: Map<string, number>; version: number } | null = null
const byScreen = new WeakMap<MapScreen, Placement>()

/** The angle on the screen of a course `deg` true at (lon, lat, z): through the camera, so it holds
 *  in a tilted and turned view. */
function screenAngle(screen: MapScreen, at: LngLat, z: number, deg: number): number | null {
  const p0 = screen.at(at[0], at[1], z)
  const k = Math.cos((at[1] * Math.PI) / 180)
  const r = (deg * Math.PI) / 180
  const d = 50 // metres ahead
  const p1 = screen.at(at[0] + (d * Math.sin(r)) / (111_320 * k), at[1] + (d * Math.cos(r)) / 111_320, z)
  if (!p0 || !p1) return null
  const dx = p1[0] - p0[0]
  const dy = p1[1] - p0[1]
  if (Math.hypot(dx, dy) < 0.01) return null
  // clockwise from screen up, then deck.gl's counter-clockwise
  return -((Math.atan2(dx, -dy) * 180) / Math.PI)
}

function placement(shown: ShownTrack[], screen: MapScreen): Placement {
  const hit = byScreen.get(screen)
  if (hit && placed?.shown === shown) return hit
  const z = new Map<string, number>()
  const angle = new Map<string, number>()
  for (const { track: t } of shown) {
    const alt = drawAltM(t.domain, t.alt_m) * HEIGHT_EXAGGERATION
    const zz = alt > 0 && screen.aloft(t.lon, t.lat, alt) ? 0 : alt
    z.set(t.id, zz)
    const c = courseOf(t)
    if (c != null && orients(t.class, t.domain) && !isBearingOnly(t)) {
      const a = screenAngle(screen, [t.lon, t.lat], zz, c) ?? -c
      angle.set(t.id, Math.round(a / 3) * 3)
    }
  }
  const same = (a: Map<string, number>, b: Map<string, number>) => a.size === b.size && [...a].every(([k, v]) => b.get(k) === v)
  const version = placed && placed.shown === shown && same(z, placed.z) && same(angle, placed.angle) ? placed.version : (placed?.version ?? 0) + 1
  placed = { shown, z, angle, version }
  const p = { z, angle, version }
  byScreen.set(screen, p)
  return p
}

const zOf = (p: Placement, t: Track) => p.z.get(t.id) ?? 0
/** where a track is drawn: a bearing-only one at the middle of its interval, on the surface with
 *  its footprint */
const posOf = (p: Placement, t: Track): XYZ => (isBearingOnly(t) ? [t.lon, t.lat, 0] : [t.lon, t.lat, zOf(p, t)])

// ---- labels -----------------------------------------------------------------------------------

const LABEL_PX = 11
const labelKey = (t: Track) => `t:${t.id}`
/** a track gets a label candidate when it is near (or flagged, or selected): the far ones never,
 *  the mid ones only on hover (the tooltip) */
const labelled = (r: ShownTrack, selected: string | null) => r.track.id === selected || r.alarm || r.rel.tier === 'near'

/** The traffic's labels for the one label budget (declutter.ts): the selected one forced; the rest
 *  at the lowest priority, so our own vehicles' labels never give way to traffic (they come first
 *  at equal priority), the flagged first, then the nearest; and, with the truth overlay on, the
 *  truth objects the picture misses. */
export function trafficLabelCandidates(input: TrafficLayerInput, screen: MapScreen): LabelCandidate[] {
  const p = placement(input.shown, screen)
  const out: LabelCandidate[] = []
  const order = input.shown.filter((r) => labelled(r, input.selected)).sort((a, b) => Number(b.alarm) - Number(a.alarm) || a.rel.distM - b.rel.distM || (a.track.id < b.track.id ? -1 : 1))
  for (const r of order) {
    const t = r.track
    const [lon, lat, z] = posOf(p, t)
    const half = isBearingOnly(t) ? 4 : ICON_PX[r.rel.tier] / 2
    const short = labelLines(t, false)
    const full = labelLines(t, true)
    const mine = t.id === input.selected
    out.push({
      key: labelKey(t),
      at: screen.at(lon, lat, z),
      short: rightOf(half, short[0], LABEL_PX),
      full: textBox(full, LABEL_PX, 'start', 'top', [half + 4, -LABEL_PX * 0.75]),
      priority: mine ? 1 : 4,
      force: mine,
    })
  }
  for (const m of input.truth ?? []) {
    if (m.track) continue
    const o = m.truth
    out.push({ key: `tm:${o.id}`, at: screen.at(o.lon, o.lat, drawAltM(o.domain, o.alt_m) * HEIGHT_EXAGGERATION), short: rightOf(6, 'missed', 10), full: rightOf(6, `missed: ${classWords(o.class)}`, 10), priority: 4 })
  }
  return out
}

/** The full label's lines are drawn from the top of the block, just above the icon's middle. */
const labelOffset = (r: ShownTrack, full: boolean): [number, number] => [(isBearingOnly(r.track) ? 4 : ICON_PX[r.rel.tier] / 2) + 4, full ? -LABEL_PX * 0.75 : 0]

// ---- the layers -------------------------------------------------------------------------------

interface Parts {
  shown: ShownTrack[]
  points: ShownTrack[]
  wedges: (ShownTrack & { wedge: ReturnType<typeof bearingWedge> })[]
  stems: ShownTrack[]
  leaders: (ShownTrack & { end: LngLat })[]
  alarms: ShownTrack[]
}

let memo: { shown: ShownTrack[]; parts: Parts } | null = null

function partition(shown: ShownTrack[]): Parts {
  if (memo?.shown === shown) return memo.parts
  const parts: Parts = { shown, points: [], wedges: [], stems: [], leaders: [], alarms: [] }
  for (const r of shown) {
    const t = r.track
    if (isBearingOnly(t)) {
      parts.wedges.push({ ...r, wedge: bearingWedge(t.bearing_only) })
      continue
    }
    parts.points.push(r)
    if (r.alarm) parts.alarms.push(r)
    if (r.rel.tier === 'far') continue
    if (t.domain === 'air' && drawAltM(t.domain, t.alt_m) > 0) parts.stems.push(r)
    const end = leaderEnd(t)
    if (end) parts.leaders.push({ ...r, end })
  }
  memo = { shown, parts }
  return parts
}

const exaggerate = (pts: [number, number, number][]): XYZ[] => pts.map(([lon, lat, h]) => [lon, lat, h * HEIGHT_EXAGGERATION])
const flat = (pts: [number, number, number][]): XYZ[] => pts.map(([lon, lat]) => [lon, lat, 0])

/** The traffic layer, in drawing order (under the fleet: MapView puts its extension layers below
 *  our vehicles). */
export function buildTrafficLayers(input: TrafficLayerInput, plan: LabelPlan = ALL_FULL, screen: MapScreen | null = null) {
  const t0 = performance.now()
  const parts = partition(input.shown)
  const p: Placement = screen ? placement(input.shown, screen) : { z: new Map(), angle: new Map(), version: -1 }
  const pos = (r: ShownTrack) => posOf(p, r.track)
  const trig = { getPosition: p.version }
  const layers = []

  if (input.coverage?.length) {
    const sites = input.coverage.filter((s) => s.owner === 'company' && s.lat != null && s.lon != null && s.enabled !== false && s.nominal_range_m > 0 && s.nominal_range_m <= COVERAGE_MAX_M)
    layers.push(
      new PolygonLayer<TrafficSensor>({
        id: TRAFFIC_LAYER.coverage,
        data: sites,
        getPolygon: (s) => circle([s.lon!, s.lat!], s.nominal_range_m, 96),
        getFillColor: (s) => (s.up === false ? [255, 93, 93, 18] : s.nominal_range_m <= 6000 ? [150, 200, 255, 16] : [150, 200, 255, 0]),
        getLineColor: (s) => (s.up === false ? [255, 93, 93, 200] : [150, 200, 255, 120]),
        getLineWidth: 1.5,
        lineWidthUnits: 'pixels',
        stroked: true,
        filled: true,
        pickable: true,
        parameters: onTop,
      }),
      new TextLayer<TrafficSensor>({
        id: TRAFFIC_LAYER.coverageNames,
        data: sites,
        getPosition: (s) => circle([s.lon!, s.lat!], s.nominal_range_m, 4)[0],
        getText: (s) => `${s.id} ${sourceKindWords(s.kind === 'ais' ? 'ais_a' : s.kind === 'rid' ? 'remote_id' : s.kind).replace(' Class A', '')} ${distText(s.nominal_range_m)}${s.up === false ? ' DOWN' : ''}`,
        getSize: 10,
        getColor: (s) => (s.up === false ? BAD : [150, 200, 255, 220]),
        getAlignmentBaseline: 'bottom',
        getPixelOffset: [0, -3],
        fontFamily: FONT,
        fontWeight: 500,
        outlineWidth: 3,
        outlineColor: OUTLINE,
        fontSettings: FONT_SETTINGS,
        characterSet: 'auto',
        extensions: NO_NEAR_GROWTH,
        parameters: onTop,
      }),
    )
  }

  if (parts.wedges.length)
    layers.push(
      new PolygonLayer<Parts['wedges'][number]>({
        id: TRAFFIC_LAYER.wedges,
        data: parts.wedges,
        getPolygon: (w) => flat(w.wedge.polygon),
        getFillColor: (w) => withA(rgb(w.track), 0.07 * w.alpha + (w.track.id === input.selected ? 0.1 : 0)),
        getLineColor: (w) => (w.track.id === input.selected ? ACCENT : withA(rgb(w.track), 0.55 * w.alpha)),
        getLineWidth: (w) => (w.track.id === input.selected ? 2 : 1.2),
        lineWidthUnits: 'pixels',
        stroked: true,
        filled: true,
        pickable: true,
        updateTriggers: { getFillColor: input.selected, getLineColor: input.selected, getLineWidth: input.selected },
        parameters: onTop,
      }),
      new PathLayer<Parts['wedges'][number]>({
        id: TRAFFIC_LAYER.lines,
        data: parts.wedges,
        getPath: (w) => flat(w.wedge.centre),
        getColor: (w) => withA(rgb(w.track), 0.9 * w.alpha),
        getWidth: 1.5,
        widthUnits: 'pixels',
        parameters: onTop,
      }),
    )
  // the selected or hovered bearing-only track's line of sight in 3D: from the sensor on our vehicle,
  // up (or down) its elevation, the wedge's near and far ends at their heights
  const sights = parts.wedges.filter((w) => w.track.id === input.selected || w.track.id === input.hovered)
  if (sights.length)
    layers.push(
      new PathLayer<{ path: XYZ[]; c: RGBA }>({
        id: TRAFFIC_LAYER.sight,
        data: sights.flatMap((w) => {
          const c = withA(rgb(w.track), 0.9)
          const s = w.wedge.sensor
          const [near, far] = w.wedge.centre
          const outline = exaggerate(w.wedge.polygon)
          return [
            { path: exaggerate([[s[0], s[1], s[2]], near, far]), c },
            { path: outline, c: withA(rgb(w.track), 0.45) },
            // where the far end is above the surface: a stem, as an aircraft's
            { path: [[far[0], far[1], 0], [far[0], far[1], far[2] * HEIGHT_EXAGGERATION]], c: withA(rgb(w.track), 0.35) },
          ]
        }),
        getPath: (d) => d.path,
        getColor: (d) => d.c,
        getWidth: 1.5,
        widthUnits: 'pixels',
        parameters: onTop,
      }),
    )

  if (parts.stems.length)
    layers.push(
      new LineLayer<ShownTrack>({
        id: TRAFFIC_LAYER.stems,
        data: parts.stems,
        getSourcePosition: (r) => [r.track.lon, r.track.lat, 0],
        getTargetPosition: pos,
        getColor: (r) => withA(rgb(r.track), 0.35 * r.alpha),
        getWidth: 1,
        widthUnits: 'pixels',
        updateTriggers: { getTargetPosition: p.version },
      }),
    )

  if (parts.leaders.length)
    layers.push(
      new LineLayer<Parts['leaders'][number]>({
        id: TRAFFIC_LAYER.leaders,
        data: parts.leaders,
        getSourcePosition: pos,
        getTargetPosition: (r) => [r.end[0], r.end[1], zOf(p, r.track)],
        getColor: (r) => withA(rgb(r.track), 0.8 * r.alpha),
        getWidth: 1.5,
        widthUnits: 'pixels',
        updateTriggers: { getSourcePosition: p.version, getTargetPosition: p.version },
        parameters: onTop,
      }),
    )

  // the predicted path of the selected and the hovered track
  const focus = [input.selected, input.hovered].filter((id, k, a): id is string => id != null && a.indexOf(id) === k)
  const predicted = focus.flatMap((id) => {
    const r = input.shown.find((x) => x.track.id === id)
    if (!r) return []
    const path = predictPath(r.track)
    if (path.length < 2) return []
    const flat = drawAltM(r.track.domain, r.track.alt_m) === 0 || zOf(p, r.track) === 0
    const at = (q: (typeof path)[number]): XYZ => [q.at[0], q.at[1], flat ? 0 : drawAltM(r.track.domain, q.altM) * HEIGHT_EXAGGERATION]
    return [{ r, path, pts: path.map(at) }]
  })
  if (predicted.length)
    layers.push(
      new PathLayer<(typeof predicted)[number]>({
        id: TRAFFIC_LAYER.predict,
        data: predicted,
        getPath: (d) => d.pts,
        getColor: (d) => withA(rgb(d.r.track), 0.95),
        getWidth: 2.5,
        widthUnits: 'pixels',
        parameters: onTop,
      }),
      new ScatterplotLayer<{ at: XYZ; sigma: number; k: number; n: number; c: [number, number, number] }>({
        id: TRAFFIC_LAYER.predictSigma,
        data: predicted.flatMap((d) => d.path.slice(1).map((q, k) => ({ at: d.pts[k + 1], sigma: q.sigmaM, k, n: d.path.length - 1, c: rgb(d.r.track) }))),
        getPosition: (d) => d.at,
        getRadius: (d) => d.sigma,
        radiusUnits: 'meters',
        radiusMinPixels: 3,
        stroked: true,
        filled: false,
        getLineColor: (d) => withA(d.c, 0.7 - (0.4 * d.k) / Math.max(1, d.n)),
        getLineWidth: 1.2,
        lineWidthUnits: 'pixels',
        parameters: onTop,
      }),
    )

  if (input.pairs.length)
    layers.push(
      new LineLayer<{ broadcast: Track; sensed: Track }>({
        id: TRAFFIC_LAYER.disagree,
        data: input.pairs,
        getSourcePosition: (d) => posOf(p, d.broadcast),
        getTargetPosition: (d) => posOf(p, d.sensed),
        getColor: [255, 93, 93, 210],
        getWidth: 2,
        widthUnits: 'pixels',
        updateTriggers: { getSourcePosition: p.version, getTargetPosition: p.version },
        parameters: onTop,
      }),
    )

  if (parts.alarms.length)
    layers.push(
      new ScatterplotLayer<ShownTrack>({
        id: TRAFFIC_LAYER.alarm,
        data: parts.alarms,
        getPosition: pos,
        getRadius: (r) => ICON_PX[r.rel.tier] / 2 + 5,
        radiusUnits: 'pixels',
        billboard: true,
        stroked: true,
        filled: false,
        getLineColor: BAD,
        getLineWidth: 2,
        lineWidthUnits: 'pixels',
        updateTriggers: trig,
        extensions: NO_NEAR_GROWTH,
        parameters: onTop,
      }),
    )

  const sel = input.selected ? parts.points.find((r) => r.track.id === input.selected) : undefined
  if (sel)
    layers.push(
      new ScatterplotLayer<ShownTrack>({
        id: TRAFFIC_LAYER.selected,
        data: [sel],
        getPosition: pos,
        getRadius: ICON_PX.near / 2 + 8,
        radiusUnits: 'pixels',
        billboard: true,
        stroked: true,
        filled: false,
        getLineColor: ACCENT,
        getLineWidth: 2,
        lineWidthUnits: 'pixels',
        updateTriggers: trig,
        extensions: NO_NEAR_GROWTH,
        parameters: onTop,
      }),
    )

  layers.push(
    new IconLayer<ShownTrack>({
      id: TRAFFIC_LAYER.icons,
      data: parts.points,
      pickable: true,
      getPosition: pos,
      getIcon: (r) => trafficIcon(r.track.class, r.track.domain, r.provisional),
      getSize: (r) => (r.track.id === input.selected ? ICON_PX.near : ICON_PX[r.rel.tier]),
      sizeUnits: 'pixels',
      getAngle: (r) => p.angle.get(r.track.id) ?? 0,
      getColor: (r) => [255, 255, 255, Math.round(r.alpha * 255)],
      updateTriggers: { getPosition: p.version, getAngle: p.version, getSize: input.selected },
      extensions: NO_NEAR_GROWTH,
      parameters: onTop,
    }),
  )

  const withLabel = input.shown.filter((r) => labelled(r, input.selected) && plan.level(labelKey(r.track)) !== 'none')
  if (withLabel.length) {
    const full = (r: ShownTrack) => plan.level(labelKey(r.track)) === 'full'
    const key = withLabel.map((r) => `${r.track.id}:${plan.level(labelKey(r.track))}`).join()
    layers.push(
      new TextLayer<ShownTrack>({
        id: TRAFFIC_LAYER.labels,
        data: withLabel,
        pickable: true,
        getPosition: pos,
        getText: (r) => labelLines(r.track, full(r)).join('\n'),
        getSize: LABEL_PX,
        lineHeight: 1.25,
        getColor: (r) => (r.alarm ? BAD : withA(rgb(r.track), Math.max(0.55, r.alpha))),
        getTextAnchor: 'start',
        getAlignmentBaseline: (r) => (full(r) ? 'top' : 'center'),
        getPixelOffset: (r) => labelOffset(r, full(r)),
        fontFamily: FONT,
        fontWeight: 600,
        outlineWidth: 3,
        outlineColor: OUTLINE,
        fontSettings: FONT_SETTINGS,
        characterSet: 'auto',
        updateTriggers: { getPosition: p.version, getText: key, getAlignmentBaseline: key, getPixelOffset: key, getColor: key },
        extensions: NO_NEAR_GROWTH,
        parameters: onTop,
      }),
    )
  }

  if (input.truth?.length) {
    const tz = (m: TruthMatch): XYZ => [m.truth.lon, m.truth.lat, drawAltM(m.truth.domain, m.truth.alt_m) * HEIGHT_EXAGGERATION]
    const linked = input.truth.filter((m) => m.track && (m.errM ?? 0) > 25 && !isBearingOnly(m.track))
    const missed = input.truth.filter((m) => !m.track && plan.level(`tm:${m.truth.id}`) !== 'none')
    layers.push(
      new LineLayer<TruthMatch>({
        id: TRAFFIC_LAYER.truthLinks,
        data: linked,
        getSourcePosition: tz,
        getTargetPosition: (m) => posOf(p, m.track!),
        getColor: withA(TRUTH_RGB, 0.55),
        getWidth: 1,
        widthUnits: 'pixels',
        updateTriggers: { getTargetPosition: p.version },
        parameters: onTop,
      }),
      new ScatterplotLayer<TruthMatch>({
        id: TRAFFIC_LAYER.truth,
        data: input.truth,
        pickable: true,
        getPosition: tz,
        getRadius: (m) => (m.track ? 3 : 6),
        radiusUnits: 'pixels',
        billboard: true,
        stroked: true,
        filled: true,
        getFillColor: (m) => (m.track ? withA(TRUTH_RGB, 0.9) : [0, 0, 0, 0]),
        getLineColor: withA(TRUTH_RGB, 1),
        getLineWidth: (m) => (m.track ? 0 : 2),
        lineWidthUnits: 'pixels',
        extensions: NO_NEAR_GROWTH,
        parameters: onTop,
      }),
      new TextLayer<TruthMatch>({
        id: TRAFFIC_LAYER.truthNames,
        data: missed,
        getPosition: tz,
        getText: (m) => (plan.level(`tm:${m.truth.id}`) === 'full' ? `missed: ${classWords(m.truth.class)}` : 'missed'),
        getSize: 10,
        getColor: withA(TRUTH_RGB, 1),
        getTextAnchor: 'start',
        getAlignmentBaseline: 'center',
        getPixelOffset: [10, 0],
        fontFamily: FONT,
        fontWeight: 600,
        outlineWidth: 3,
        outlineColor: OUTLINE,
        fontSettings: FONT_SETTINGS,
        characterSet: 'auto',
        updateTriggers: { getText: missed.map((m) => plan.level(`tm:${m.truth.id}`)).join() },
        extensions: NO_NEAR_GROWTH,
        parameters: onTop,
      }),
    )
  }
  // ?perf: what building them cost
  perfPush('trafficLayers', performance.now() - t0)
  return layers
}

/** The hover text of a traffic layer's object (a track, a truth object, a receiver), keyed as its
 *  label is, or null for another layer. */
export function trafficTooltip(layer: string, object: unknown, text: (t: Track) => string): { key: string; text: string } | null {
  if (TRAFFIC_PICK_LAYERS.includes(layer)) {
    const t = (object as ShownTrack).track
    return { key: labelKey(t), text: text(t) }
  }
  if (layer === TRAFFIC_LAYER.truth) {
    const m = object as TruthMatch
    const o = m.truth
    const who = o.ids?.callsign?.trim() || o.ids?.name?.trim() || o.id
    const alt = o.domain === 'air' ? ` · ${Math.round(o.alt_m)} m` : ''
    const seen = m.track ? `the picture has it as ${trackTitle(m.track)}, ${distText(m.errM ?? 0)} off` : 'missed: the picture has nothing for it'
    return { key: `tm:${o.id}`, text: `Simulator truth (not available to a real operator): ${who}, ${classWords(o.class)}${alt}${o.faults?.length ? ` · faults: ${o.faults.join(', ')}` : ''} · ${seen}` }
  }
  if (layer === TRAFFIC_LAYER.coverage) {
    const s = object as TrafficSensor
    return { key: `tc:${s.id}:${s.kind}`, text: `${s.id}: ${s.kind} receiver, nominal reach ${distText(s.nominal_range_m)}${s.up === false ? ', DOWN' : ''} (a nominal range, not a coverage prediction)` }
  }
  return null
}
