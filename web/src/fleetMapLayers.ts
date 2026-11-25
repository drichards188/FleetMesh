// The base markers on the map (fleetMap.ts): one disc per base (or per site, zoomed out) standing
// for the vehicles parked at it, the count in it, and its counts by state beside it where the label
// budget has room; an opened base's marker is hollow, its vehicles drawn one by one. Our fleet's
// own blue, ringed as our vehicles' symbols are, so a marker never reads as other traffic. deck.gl
// layers for MapView's `layers` prop; display only.
import { ScatterplotLayer, TextLayer } from '@deck.gl/layers'
import { ALL_FULL, type LabelPlan } from './declutter'
import { type BaseMarker, labelOffset, markerRadius } from './fleetMap'

/** layer ids that stand for a base marker under the pointer */
export const BASE_PICK_LAYERS = ['fleet-bases']

type RGBA = [number, number, number, number]
const FILL: RGBA = [18, 34, 54, 235]
const FILL_OPEN: RGBA = [18, 34, 54, 70]
const RING: RGBA = [127, 184, 255, 235]
const TEXT: RGBA = [215, 221, 228, 255]
const COUNT: RGBA = [235, 242, 250, 255]
const BG: RGBA = [11, 15, 20, 215]
const FONT = 'IBM Plex Mono, ui-monospace, monospace'
const FONT_SETTINGS = { sdf: true, fontSize: 48, buffer: 6 }

export function buildBaseLayers(markers: readonly BaseMarker[], plan: LabelPlan = ALL_FULL) {
  if (!markers.length) return []
  const onTop = { depthCompare: 'always' as const, depthWriteEnabled: false }
  const at = (m: BaseMarker): [number, number, number] => [m.at[0], m.at[1], 0]
  const labelled = markers.filter((m) => plan.level(`fb:${m.id}`) !== 'none')
  const key = markers.map((m) => `${m.id}:${m.n}:${m.text}`).join('|')
  const levels = labelled.map((m) => plan.level(`fb:${m.id}`)).join()
  return [
    new ScatterplotLayer<BaseMarker>({
      id: 'fleet-bases',
      data: markers,
      pickable: true,
      getPosition: at,
      getRadius: markerRadius,
      radiusUnits: 'pixels',
      billboard: true,
      stroked: true,
      filled: true,
      getFillColor: (m) => (m.open ? FILL_OPEN : FILL),
      getLineColor: RING,
      getLineWidth: 2.5,
      lineWidthUnits: 'pixels',
      updateTriggers: { getRadius: key, getFillColor: key },
      parameters: onTop,
    }),
    new TextLayer<BaseMarker>({
      id: 'fleet-base-counts',
      data: markers.filter((m) => !m.open),
      getPosition: at,
      getText: (m) => String(m.n),
      getSize: 13,
      getColor: COUNT,
      getTextAnchor: 'middle',
      getAlignmentBaseline: 'center',
      fontFamily: FONT,
      fontWeight: 700,
      fontSettings: FONT_SETTINGS,
      characterSet: 'auto',
      updateTriggers: { getText: key },
      parameters: onTop,
    }),
    new TextLayer<BaseMarker>({
      id: 'fleet-base-labels',
      data: labelled,
      getPosition: at,
      getText: (m) => (plan.level(`fb:${m.id}`) === 'full' ? m.text : m.name),
      getPixelOffset: labelOffset,
      getSize: 12,
      getColor: TEXT,
      getTextAnchor: 'start',
      getAlignmentBaseline: 'top',
      background: true,
      getBackgroundColor: BG,
      backgroundPadding: [4, 2],
      fontFamily: FONT,
      fontWeight: 600,
      fontSettings: FONT_SETTINGS,
      characterSet: 'auto',
      updateTriggers: { getText: `${key}#${levels}`, getPixelOffset: key },
      parameters: onTop,
    }),
  ]
}
