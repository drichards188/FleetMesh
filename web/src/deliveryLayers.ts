// The delivery layers on the map (the Layers menu): hubs with their battery packs, drop zones
// coloured by their latest look, parcels where they are, each running delivery's route, and the
// areas where vehicles show GNSS interference or a link outage (suspected, from symptoms:
// suspectAreas.ts). deck.gl layers for MapView's `layers` prop; display only.
import { PathLayer, PolygonLayer, ScatterplotLayer, TextLayer } from '@deck.gl/layers'
import { type DropZone, type Hub, type Parcel, isLate, observationText, observationTone, parcelAt, parcelStateLabel } from './delivery'
import { type RouteDraw } from './deliveryRoutes'
import { ALL_FULL, type LabelPlan } from './declutter'
import type { MapScreen } from './aloft'
import { areaKey, areaText, hubOffset, hubText } from './deliveryLabels'
import { circle } from './geo'
import type { SuspectArea } from './suspectAreas'
import type { LngLat } from './types'

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
  routes: 'delivery-routes',
  areas: 'delivery-areas',
  areaNames: 'delivery-area-names',
} as const

const ZONE: Record<'ok' | 'bad' | 'unknown', RGBA> = { ok: [96, 200, 130, 235], bad: [236, 94, 84, 240], unknown: [150, 160, 172, 220] }
const PARCEL: Record<string, RGBA> = {
  waiting: [242, 193, 78, 240],
  ready: [242, 193, 78, 240],
  assigned: [127, 184, 255, 240],
  loaded: [196, 155, 255, 245],
  delivered: [96, 200, 130, 150],
  failed: [236, 94, 84, 220],
}
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
  parcels: Parcel[]
  elapsedS: number
  routes: RouteDraw[]
  areas: SuspectArea[]
  /** a vehicle's position by its uid (as the parcels name it) */
  vehicleAt: (uid: string) => LngLat | null
}

export interface ParcelDot {
  id: string
  at: LngLat
  parcel: Parcel
  late: boolean
}

/** Parcels at the same place spread on a small ring around it (a hub holds dozens), so each is a
 *  dot of its own: 25 m out, eight to a ring, further rings 14 m beyond. */
export function parcelDots(d: Pick<DeliveryData, 'hubs' | 'zones' | 'parcels' | 'elapsedS' | 'vehicleAt'>): ParcelDot[] {
  const hubs = new Map(d.hubs.map((h) => [h.id, h]))
  const zones = new Map(d.zones.map((z) => [z.id, z]))
  const seen = new Map<string, number>()
  const out: ParcelDot[] = []
  for (const p of d.parcels) {
    const at = parcelAt(p, hubs, zones, d.vehicleAt)
    if (!at) continue
    const key = `${at[0].toFixed(5)},${at[1].toFixed(5)}`
    const k = seen.get(key) ?? 0
    seen.set(key, k + 1)
    const ring = Math.floor(k / 8)
    const r = 25 + ring * 14
    const a = ((k % 8) / 8) * 2 * Math.PI + ring * 0.4
    const dLat = (r * Math.cos(a)) / 111_320
    const dLon = (r * Math.sin(a)) / (111_320 * Math.cos((at[1] * Math.PI) / 180))
    out.push({ id: p.id, at: [at[0] + dLon, at[1] + dLat], parcel: p, late: isLate(p, d.elapsedS) })
  }
  return out
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
  if (on.parcels) {
    const dots = parcelDots(d)
    if (dots.length)
      layers.push(
        new ScatterplotLayer<ParcelDot>({
          id: LAYER_IDS.parcels,
          data: dots,
          getPosition: (p) => p.at,
          getRadius: 5,
          radiusUnits: 'pixels',
          getFillColor: (p) => PARCEL[p.parcel.state] ?? PARCEL.waiting,
          getLineColor: (p) => (p.late ? [236, 94, 84, 255] : OUTLINE),
          getLineWidth: (p) => (p.late ? 2 : 1),
          lineWidthUnits: 'pixels',
          stroked: true,
          pickable: true,
          updateTriggers: { getFillColor: dots.map((p) => p.parcel.state).join(), getLineColor: dots.map((p) => p.late).join() },
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
    case LAYER_IDS.parcels: {
      const p = (object as ParcelDot).parcel
      const late = isLate(p, elapsedS) ? ', late' : ''
      return { key: `parcel:${p.id}`, text: `${p.id}: ${parcelStateLabel(p.state)}${late}, ${p.mass_kg} kg, ${p.from} to ${p.to}${p.vehicle ? `, ${name(p.vehicle)}` : ''}` }
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
