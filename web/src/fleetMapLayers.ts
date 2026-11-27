// The hubs' marks on the map (fleetMap.ts): a house per hub (or per site, zoomed out), our fleet's
// navy and blue so it never reads as other traffic, its roof the colour of the hub's state (blue
// while all is well, amber while the wind or a hold slows it, red while it is grounded or someone is
// needed, grey while closed), the number of its parked vehicles on its walls, a red count on its
// eave of its vehicles that need the person, and beside it its name with one line under it in the
// state's colour. An opened hub's house is hollow, its vehicles drawn one by one. deck.gl layers for
// MapView's `layers` prop; display only.
import { IconLayer, TextLayer } from '@deck.gl/layers'
import { ALL_FULL, type LabelPlan } from './declutter'
import { BADGE, type BaseMarker, labelLine, labelOffset, NEED_AT } from './fleetMap'
import type { HubTone } from './hubSummary'

/** layer ids that stand for a hub's mark under the pointer */
export const BASE_PICK_LAYERS = ['fleet-bases']

type RGBA = [number, number, number, number]
const TEXT: RGBA = [215, 221, 228, 255]
const COUNT: RGBA = [235, 242, 250, 255]
const BG: RGBA = [11, 15, 20, 215]
const LINE: Record<HubTone, RGBA> = { quiet: [150, 165, 180, 255], warn: [242, 193, 78, 255], bad: [255, 120, 112, 255], closed: [138, 150, 163, 255] }
const NEED: RGBA = [255, 93, 93, 255]
const FONT = 'IBM Plex Mono, ui-monospace, monospace'
const FONT_SETTINGS = { sdf: true, fontSize: 48, buffer: 6 }

// the house, drawn on a 32 x 50 grid at twice its size on the screen: its walls from y 14 to 34, its
// roof a gable in the state's colour from its apex at y 2 to its eaves at y 15.5, its stem down to its foot at y 47, which is the hub's point
const ROOF: Record<HubTone, string> = { quiet: 'rgb(127,184,255)', warn: 'rgb(242,193,78)', bad: 'rgb(255,93,93)', closed: 'rgb(138,150,163)' }
const ATLAS = 2
const houses = new Map<string, { id: string; url: string; width: number; height: number; anchorX: number; anchorY: number }>()
function house(tone: HubTone, open: boolean) {
  const id = `hub-${tone}${open ? '-open' : ''}`
  const hit = houses.get(id)
  if (hit) return hit
  const edge = 'rgb(127,184,255)'
  const walls = open ? `fill="rgba(18,34,54,0.25)" stroke="${edge}" stroke-width="2" stroke-dasharray="4 3"` : `fill="rgba(18,34,54,0.94)" stroke="${edge}" stroke-width="2"`
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${32 * ATLAS}" height="${50 * ATLAS}" viewBox="0 0 32 50">` +
    `<path d="M16 35 L16 46" stroke="rgb(11,15,20)" stroke-width="3.2" stroke-linecap="round"/>` +
    `<path d="M16 35 L16 46" stroke="${edge}" stroke-width="1.6" stroke-linecap="round"/>` +
    `<circle cx="16" cy="47" r="2.4" fill="${edge}" stroke="rgb(11,15,20)" stroke-width="1"/>` +
    `<path d="M3 14 L16 3 L29 14 L29 34 L3 34 Z" ${walls} stroke-linejoin="round"/>` +
    (open ? '' : `<path d="M1 15.5 L16 2 L31 15.5 Z" fill="${ROOF[tone]}" stroke="rgb(11,15,20)" stroke-width="1" stroke-linejoin="round"/>`) +
    '</svg>'
  const icon = { id, url: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`, width: 32 * ATLAS, height: 50 * ATLAS, anchorX: 16 * ATLAS, anchorY: 47 * ATLAS }
  houses.set(id, icon)
  return icon
}

export function buildBaseLayers(markers: readonly BaseMarker[], plan: LabelPlan = ALL_FULL) {
  if (!markers.length) return []
  const onTop = { depthCompare: 'always' as const, depthWriteEnabled: false }
  const at = (m: BaseMarker): [number, number, number] => [m.at[0], m.at[1], 0]
  const level = (m: BaseMarker) => plan.level(`fb:${m.id}`)
  const named = markers.filter((m) => level(m) !== 'none')
  const lined = named.filter((m) => level(m) === 'full')
  const needing = markers.filter((m) => m.view.needYou > 0)
  const key = markers.map((m) => `${m.id}:${m.n}:${m.open}:${m.view.tone}:${m.view.needYou}:${m.view.line}`).join('|')
  const levels = markers.map(level).join()
  const [lx, ly] = labelOffset()
  const label = {
    getPosition: at,
    getSize: 12,
    getTextAnchor: 'start' as const,
    getAlignmentBaseline: 'top' as const,
    background: true,
    getBackgroundColor: BG,
    backgroundPadding: [4, 2] as [number, number],
    fontFamily: FONT,
    fontWeight: 600,
    fontSettings: FONT_SETTINGS,
    characterSet: 'auto' as const,
    parameters: onTop,
  }
  return [
    new IconLayer<BaseMarker>({
      id: 'fleet-bases',
      data: markers,
      pickable: true,
      getPosition: at,
      getIcon: (m) => house(m.view.tone, m.open),
      // the grid's height on the screen (the house is 34 of its 50)
      getSize: 50,
      sizeUnits: 'pixels',
      updateTriggers: { getIcon: key },
      parameters: onTop,
    }),
    new TextLayer<BaseMarker>({
      id: 'fleet-base-counts',
      data: markers.filter((m) => m.n > 0),
      getPosition: at,
      getText: (m) => String(m.n),
      // smaller once a hub's number has three digits
      getSize: (m) => (m.n >= 100 ? 10 : 12),
      getColor: COUNT,
      getTextAnchor: 'middle',
      getAlignmentBaseline: 'center',
      getPixelOffset: [0, -BADGE.lift + 1],
      fontFamily: FONT,
      fontWeight: 700,
      fontSettings: FONT_SETTINGS,
      characterSet: 'auto',
      updateTriggers: { getText: key },
      parameters: onTop,
    }),
    new IconLayer<BaseMarker>({
      id: 'fleet-base-need-dot',
      data: needing,
      getPosition: at,
      getIcon: () => NEED_DOT,
      getSize: 17,
      sizeUnits: 'pixels',
      getPixelOffset: NEED_AT,
      parameters: onTop,
    }),
    new TextLayer<BaseMarker>({
      id: 'fleet-base-need-count',
      data: needing,
      getPosition: at,
      getText: (m) => String(m.view.needYou),
      getSize: 11,
      getColor: [255, 255, 255, 255],
      getTextAnchor: 'middle',
      getAlignmentBaseline: 'center',
      getPixelOffset: [NEED_AT[0], NEED_AT[1] + 0.5],
      fontFamily: FONT,
      fontWeight: 700,
      fontSettings: FONT_SETTINGS,
      characterSet: 'auto',
      updateTriggers: { getText: key },
      parameters: onTop,
    }),
    new TextLayer<BaseMarker>({
      ...label,
      id: 'fleet-base-labels',
      data: named,
      getText: (m) => m.name,
      getColor: TEXT,
      getPixelOffset: [lx, ly],
      updateTriggers: { getText: `${key}#${levels}` },
    }),
    new TextLayer<BaseMarker>({
      ...label,
      id: 'fleet-base-lines',
      data: lined,
      getText: labelLine,
      getColor: (m) => (m.open ? LINE.quiet : LINE[m.view.tone]),
      getPixelOffset: [lx, ly + 16],
      fontWeight: 500,
      updateTriggers: { getText: `${key}#${levels}`, getColor: key },
    }),
  ]
}

const NEED_DOT = (() => {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" viewBox="0 0 18 18"><circle cx="9" cy="9" r="7.6" fill="rgb(${NEED.slice(0, 3).join(',')})" stroke="rgb(11,15,20)" stroke-width="1.4"/></svg>`
  return { id: 'hub-need', url: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`, width: 36, height: 36, anchorX: 18, anchorY: 18 }
})()
