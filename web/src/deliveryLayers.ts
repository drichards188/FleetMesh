// The delivery layers on the map (the Layers menu): hubs with their battery packs, drop zones
// coloured by their latest look, parcels by place (one mark for a hub, a vehicle or a drop zone,
// with how many are there), each running delivery's route, and the
// areas where vehicles show GNSS interference or a link outage (suspected, from symptoms:
// suspectAreas.ts). deck.gl layers for MapView's `layers` prop; display only.
import { IconLayer, PathLayer, PolygonLayer, ScatterplotLayer, TextLayer } from '@deck.gl/layers'
import { type DropZone, type Hub, type ParcelPlace, observationText, observationTone, placeParcelsText } from './delivery'
import { type RouteDraw } from './deliveryRoutes'
import { ALL_FULL, type LabelPlan } from './declutter'
import type { MapScreen } from './aloft'
import { areaKey, areaText, hubOffset, hubText, PARCEL_COUNT_PX, PARCEL_MARK_PX, parcelCountOffset, parcelCountText, parcelKey, parcelOffset } from './deliveryLabels'
import { circle } from './geo'
import type { SuspectArea } from './suspectAreas'

type RGBA = [number, number, number, number]
const TEXT: RGBA = [235, 239, 244, 255]
const OUTLINE: RGBA = [11, 15, 20, 255]
const FONT = 'IBM Plex Mono, ui-monospace, monospace'
const FONT_SETTINGS = { sdf: true, fontSize: 48, buffer: 6 }

export const LAYER_IDS = {
  hubs: 'delivery-hubs',
  hubNames: 'delivery-hub-names',
  zones: 'delivery-zones',
  parcels: 'delivery-parcels',
  parcelCounts: 'delivery-parcel-counts',
  routes: 'delivery-routes',
  areas: 'delivery-areas',
  areaNames: 'delivery-area-names',
} as const

const ZONE: Record<'ok' | 'bad' | 'unknown', RGBA> = { ok: [96, 200, 130, 235], bad: [236, 94, 84, 240], unknown: [150, 160, 172, 220] }
/** A place's mark: a parcel (a box with its lid and tape), filled with the colour of what its
 *  parcels are doing (the most pressing: waiting for a vehicle, then assigned, in transit,
 *  delivered), edged red while one there is late. */
const PARCEL_FILL: Record<string, string> = { ready: 'rgb(242,193,78)', assigned: 'rgb(127,184,255)', loaded: 'rgb(196,155,255)', delivered: 'rgba(96,200,130,0.65)' }
const LEAD = ['ready', 'assigned', 'loaded', 'delivered']
export const leadState = (pl: Pick<ParcelPlace, 'parcels'>) => LEAD.find((s) => pl.parcels.some((p) => p.state === s)) ?? 'ready'
const MARK_ATLAS = 2 * PARCEL_MARK_PX
const marks = new Map<string, { id: string; url: string; width: number; height: number; anchorX: number; anchorY: number }>()
function parcelMark(state: string, late: boolean) {
  const id = `parcel-${state}${late ? '-late' : ''}`
  const hit = marks.get(id)
  if (hit) return hit
  const edge = late ? 'stroke="rgb(236,94,84)" stroke-width="3.4"' : 'stroke="rgb(11,15,20)" stroke-width="2.2"'
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${MARK_ATLAS}" height="${MARK_ATLAS}" viewBox="0 0 32 32">` +
    `<rect x="3" y="6" width="26" height="21" rx="2.5" fill="${PARCEL_FILL[state] ?? PARCEL_FILL.ready}" ${edge}/>` +
    `<path d="M4.5 12.5h23M16 7.5v5" stroke="rgb(11,15,20)" stroke-width="2" fill="none"/></svg>`
  const mark = { id, url: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`, width: MARK_ATLAS, height: MARK_ATLAS, anchorX: MARK_ATLAS / 2, anchorY: MARK_ATLAS / 2 }
  marks.set(id, mark)
  return mark
}
/** The parcel layers a click opens the Parcels list from (at that place). */
export const PARCEL_PICK_LAYERS: string[] = [LAYER_IDS.parcels, LAYER_IDS.parcelCounts]
const ROUTE: Record<RouteDraw['action'], RGBA> = { pickup: [127, 184, 255, 220], deliver: [96, 200, 130, 235], home: [150, 160, 172, 200], other: [215, 221, 228, 200] }
const AREA: Record<SuspectArea['kind'], { fill: RGBA; line: RGBA }> = {
  gnss: { fill: [236, 94, 84, 46], line: [236, 94, 84, 220] },
  link_down: { fill: [242, 193, 78, 46], line: [242, 193, 78, 230] },
  link_degraded: { fill: [242, 193, 78, 22], line: [242, 193, 78, 150] },
}

export interface DeliveryDraw {
  hubs: boolean
  zones: boolean
  parcels: boolean
  routes: boolean
  areas: boolean
}

export interface DeliveryData {
  hubs: Hub[]
  zones: DropZone[]
  /** where the parcels are now, one place each (delivery.ts parcelPlaces) */
  places: ParcelPlace[]
  routes: RouteDraw[]
  areas: SuspectArea[]
}

export function buildDeliveryLayers(d: DeliveryData, on: DeliveryDraw, plan: LabelPlan = ALL_FULL, screen: MapScreen | null = null) {
  const layers = []
  if (on.areas && d.areas.length) {
    layers.push(
      new PolygonLayer<SuspectArea>({
        id: LAYER_IDS.areas,
        data: d.areas,
        getPolygon: (a) => circle(a.at, a.radius_m, 48),
        getFillColor: (a) => AREA[a.kind].fill,
        getLineColor: (a) => AREA[a.kind].line,
        getLineWidth: 2,
        lineWidthUnits: 'pixels',
        stroked: true,
        filled: true,
        pickable: true,
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
      new TextLayer<SuspectArea>({
        id: LAYER_IDS.areaNames,
        data: d.areas.filter((a) => plan.level(areaKey(a)) !== 'none'),
        getPosition: (a) => a.at,
        getText: (a) => areaText(a, plan.level(areaKey(a)) === 'full'),
        updateTriggers: { getText: d.areas.map((a) => plan.level(areaKey(a))).join() },
        getSize: 12,
        getColor: (a) => AREA[a.kind].line,
        fontFamily: FONT,
        fontWeight: 600,
        outlineWidth: 3,
        outlineColor: OUTLINE,
        fontSettings: FONT_SETTINGS,
        characterSet: 'auto',
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
    )
  }
  if (on.routes && d.routes.length) {
    layers.push(
      new PathLayer<RouteDraw>({
        id: LAYER_IDS.routes,
        data: d.routes,
        getPath: (r) => r.path,
        getColor: (r) => ROUTE[r.action],
        getWidth: 3,
        widthUnits: 'pixels',
        pickable: true,
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
    )
  }
  if (on.zones && d.zones.length) {
    layers.push(
      new ScatterplotLayer<DropZone>({
        id: LAYER_IDS.zones,
        data: d.zones,
        getPosition: (z) => z.at,
        getRadius: (z) => z.radius_m,
        radiusUnits: 'meters',
        radiusMinPixels: 5,
        radiusMaxPixels: 14,
        getFillColor: (z) => ZONE[observationTone(z.observation, z.closed)],
        getLineColor: OUTLINE,
        getLineWidth: 1.5,
        lineWidthUnits: 'pixels',
        stroked: true,
        pickable: true,
        updateTriggers: { getFillColor: d.zones.map((z) => `${z.observation?.state}${z.closed}`).join('') },
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
    )
  }
  if (on.hubs && d.hubs.length) {
    layers.push(
      new ScatterplotLayer<Hub>({
        id: LAYER_IDS.hubs,
        data: d.hubs,
        getPosition: (h) => h.at,
        getRadius: (h) => h.radius_m,
        radiusUnits: 'meters',
        radiusMinPixels: 9,
        getFillColor: (h) => (h.open ? [74, 140, 214, 90] : [92, 102, 114, 90]),
        getLineColor: (h) => (h.open ? [127, 184, 255, 255] : [150, 160, 172, 255]),
        getLineWidth: 2.5,
        lineWidthUnits: 'pixels',
        stroked: true,
        pickable: true,
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
      new TextLayer<Hub>({
        id: LAYER_IDS.hubNames,
        data: d.hubs.filter((h) => plan.level(`dh:${h.id}`) !== 'none'),
        getPosition: (h) => h.at,
        getText: (h) => hubText(h, plan.level(`dh:${h.id}`) === 'full'),
        getSize: 13,
        getColor: TEXT,
        getTextAnchor: 'start',
        getAlignmentBaseline: 'center',
        getPixelOffset: (h) => hubOffset(h, screen),
        updateTriggers: { getText: d.hubs.map((h) => `${h.packs}${plan.level(`dh:${h.id}`)}`).join(), getPixelOffset: screen ? d.hubs.map((h) => hubOffset(h, screen).join()).join() : '' },
        fontFamily: FONT,
        fontWeight: 600,
        outlineWidth: 3,
        outlineColor: OUTLINE,
        fontSettings: FONT_SETTINGS,
        characterSet: 'auto',
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
    )
  }
  if (on.parcels && d.places.length) {
    const places = d.places
    const offsets = screen ? places.map((pl) => parcelOffset(pl, screen).join()).join() : ''
    const counted = places.filter((pl) => parcelCountText(pl) && plan.level(parcelKey(pl)) !== 'none')
    layers.push(
      new IconLayer<ParcelPlace>({
        id: LAYER_IDS.parcels,
        data: places,
        getPosition: (pl) => pl.at,
        getIcon: (pl) => parcelMark(leadState(pl), pl.late > 0),
        getSize: PARCEL_MARK_PX,
        sizeUnits: 'pixels',
        getPixelOffset: (pl) => parcelOffset(pl, screen),
        pickable: true,
        updateTriggers: { getIcon: places.map((pl) => `${leadState(pl)}${pl.late > 0}`).join(), getPixelOffset: offsets },
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
      new TextLayer<ParcelPlace>({
        id: LAYER_IDS.parcelCounts,
        data: counted,
        getPosition: (pl) => pl.at,
        getText: parcelCountText,
        getSize: PARCEL_COUNT_PX,
        getColor: TEXT,
        getTextAnchor: 'start',
        getAlignmentBaseline: 'center',
        getPixelOffset: (pl) => parcelCountOffset(parcelOffset(pl, screen)),
        pickable: true,
        updateTriggers: { getText: counted.map((pl) => pl.parcels.length).join(), getPixelOffset: offsets },
        fontFamily: FONT,
        fontWeight: 600,
        outlineWidth: 3,
        outlineColor: OUTLINE,
        fontSettings: FONT_SETTINGS,
        characterSet: 'auto',
        parameters: { depthCompare: 'always' as const, depthWriteEnabled: false },
      }),
    )
  }
  return layers
}

/** The hover text of a delivery layer's object, or null for another layer. */
export function deliveryTooltip(layer: string, object: unknown, elapsedS: number, name: (uid: string) => string): { key: string; text: string } | null {
  switch (layer) {
    case LAYER_IDS.hubs: {
      const h = object as Hub
      return { key: `hub:${h.id}`, text: `${h.name} (${h.id}): ${h.packs} charged pack${h.packs === 1 ? '' : 's'}, ${h.pads} pads${h.open ? '' : ', closed'}` }
    }
    case LAYER_IDS.zones: {
      const z = object as DropZone
      return { key: `dz:${z.id}`, text: `${z.id} (${z.kind}, ${z.surface}${z.populated ? ', populated' : ''}${z.closed ? ', closed' : ''}): ${observationText(z.observation)}` }
    }
    case LAYER_IDS.parcels:
    case LAYER_IDS.parcelCounts: {
      // not the count's own label key: the hover says more than the number, so it shows with it
      const pl = object as ParcelPlace
      return { key: `parcels:${pl.key}`, text: `${pl.kind === 'vehicle' ? name(pl.id) : pl.name}: ${placeParcelsText(pl, elapsedS)} · click for the list` }
    }
    case LAYER_IDS.routes: {
      const r = object as RouteDraw
      return { key: `route:${r.key}`, text: `${name(String(r.uid))}: ${r.action === 'home' ? 'returning to a hub' : r.action}${r.parcel ? ` ${r.parcel}` : ''}` }
    }
    case LAYER_IDS.areas: {
      const a = object as SuspectArea
      return { key: `area:${a.kind}:${a.uids.join()}`, text: `${a.label} (suspected, from what these vehicles report: ${a.uids.map((u) => name(String(u))).join(', ')})` }
    }
    default:
      return null
  }
}
