// Traffic icons: plain pictograms in the style of the fleet's delivery icons (symbols.ts), one per
// class of the traffic the fleet does not control. Not military symbology. Each is a 40 by 40
// viewBox drawing pointing north. They differ from our own vehicles' at a glance: no round frame,
// an outline over a translucent fill instead of a solid body, and their own colours, lavender in
// the air and sand on the water (ours are blue, green and teal). A tentative track is drawn
// provisional: dashed and unfilled. Pure: trafficSymbols.check.ts.
import type { TrafficClass, TrafficDomain } from './traffic.ts'

type Shape =
  | 'jet'
  | 'bizjet'
  | 'light'
  | 'seaplane'
  | 'helicopter'
  | 'drone'
  | 'unknownAir'
  | 'ship'
  | 'tanker'
  | 'passenger'
  | 'ferry'
  | 'tug'
  | 'smallBoat'
  | 'fishing'
  | 'sail'
  | 'kayak'
  | 'unknownSurface'

const SHAPE: Record<TrafficClass, Shape> = {
  airliner: 'jet',
  bizjet: 'bizjet',
  ga: 'light',
  seaplane: 'seaplane',
  helicopter: 'helicopter',
  drone: 'drone',
  unknown_air: 'unknownAir',
  cargo: 'ship',
  tanker: 'tanker',
  passenger: 'passenger',
  ferry: 'ferry',
  tug: 'tug',
  pilot: 'smallBoat',
  pleasure: 'smallBoat',
  fishing: 'fishing',
  sailing: 'sail',
  small_craft: 'kayak',
  unknown_surface: 'unknownSurface',
}

/** the traffic's colours: lavender in the air, sand on the water */
export const AIR_TRAFFIC = '#c7b5ff'
export const SURFACE_TRAFFIC = '#e8d6a6'
export const AIR_TRAFFIC_RGB: [number, number, number] = [199, 181, 255]
export const SURFACE_TRAFFIC_RGB: [number, number, number] = [232, 214, 166]
const HALO = '#0b0f14'

/** a shape with no direction to show (it is drawn upright) */
const UNORIENTED: ReadonlySet<Shape> = new Set(['unknownAir', 'unknownSurface', 'drone'])

export function shapeOf(c: string, domain: TrafficDomain): Shape {
  return SHAPE[c as TrafficClass] ?? (domain === 'air' ? 'unknownAir' : 'unknownSurface')
}

/** Whether the icon turns with the track's course. */
export function orients(c: string, domain: TrafficDomain): boolean {
  return !UNORIENTED.has(shapeOf(c, domain))
}

/** The body's paths, pointing up, as `d` strings, then the details drawn on top. */
function body(shape: Shape): { paths: string[]; marks?: string } {
  switch (shape) {
    case 'jet':
      return { paths: ['M20 3 L22.4 12.5 L37 21.5 L37 24.5 L22.4 20.5 L21.8 30 L26.5 34 L26.5 36.5 L20 34.8 L13.5 36.5 L13.5 34 L18.2 30 L17.6 20.5 L3 24.5 L3 21.5 L17.6 12.5 Z'] }
    case 'bizjet':
      return { paths: ['M20 6 L21.8 15 L32 22 L32 24.5 L21.8 21.5 L21.4 29 L25 32 L25 34 L20 32.8 L15 34 L15 32 L18.6 29 L18.2 21.5 L8 24.5 L8 22 L18.2 15 Z'] }
    case 'light':
      return { paths: ['M18.6 5 L21.4 5 L21.6 13.5 L36 13.5 L36 17.5 L21.6 17.5 L21.2 29.5 L26 30.5 L26 33.5 L14 33.5 L14 30.5 L18.8 29.5 L18.4 17.5 L4 17.5 L4 13.5 L18.4 13.5 Z'] }
    case 'seaplane':
      return {
        paths: ['M18.6 5 L21.4 5 L21.6 13.5 L36 13.5 L36 17.5 L21.6 17.5 L21.2 29.5 L26 30.5 L26 33.5 L14 33.5 L14 30.5 L18.8 29.5 L18.4 17.5 L4 17.5 L4 13.5 L18.4 13.5 Z', 'M11 9 L14 9 L14 27 L11 27 Z', 'M26 9 L29 9 L29 27 L26 27 Z'],
      }
    case 'helicopter':
      return { paths: ['M20 9 C24 9 25.5 13 25.5 17 C25.5 21.5 23.5 24.5 21.2 25.5 L21.2 33 L25 33 L25 35.5 L15 35.5 L15 33 L18.8 33 L18.8 25.5 C16.5 24.5 14.5 21.5 14.5 17 C14.5 13 16 9 20 9 Z'], marks: 'M6 6 L34 28 M34 6 L6 28' }
    case 'drone':
      return {
        paths: ['M16.5 16.5 L23.5 16.5 L23.5 23.5 L16.5 23.5 Z', ...[[11, 11], [29, 11], [11, 29], [29, 29]].map(([x, y]) => `M${x - 5} ${y} a5 5 0 1 0 10 0 a5 5 0 1 0 -10 0 Z`)],
        marks: 'M14 14 L26 26 M26 14 L14 26',
      }
    case 'unknownAir':
      return { paths: ['M20 5 L34 20 L20 35 L6 20 Z'] }
    case 'ship':
      return { paths: ['M20 2 C24.5 7 25.5 13 25.5 19 L25.5 37 L14.5 37 L14.5 19 C14.5 13 15.5 7 20 2 Z'], marks: 'M16.5 14 H23.5 M16.5 19 H23.5 M16.5 24 H23.5 M16 31 H24' }
    case 'tanker':
      return { paths: ['M20 2 C24.5 7 25.5 13 25.5 19 L25.5 37 L14.5 37 L14.5 19 C14.5 13 15.5 7 20 2 Z'], marks: 'M20 9 V30 M16 31 H24' }
    case 'passenger':
      return { paths: ['M20 2 C25 7 26 13 26 19 L26 34 C26 36 24.5 37 22.5 37 L17.5 37 C15.5 37 14 36 14 34 L14 19 C14 13 15 7 20 2 Z'], marks: 'M17 13 V31 M23 13 V31' }
    case 'ferry':
      return { paths: ['M20 3 C25.5 9 26.5 15 26.5 20 C26.5 25 25.5 31 20 37 C14.5 31 13.5 25 13.5 20 C13.5 15 14.5 9 20 3 Z'], marks: 'M16.5 15 H23.5 V25 H16.5 Z' }
    case 'tug':
      return { paths: ['M20 8 C26 11 27.5 16 27.5 21 L27.5 31 C27.5 33 26 34 24 34 L16 34 C14 34 12.5 33 12.5 31 L12.5 21 C12.5 16 14 11 20 8 Z'], marks: 'M16 19 H24 V27 H16 Z' }
    case 'smallBoat':
      return { paths: ['M20 7 C24 12 25 17 25 22 L25 33 L15 33 L15 22 C15 17 16 12 20 7 Z'], marks: 'M16.5 20 C18.5 18.5 21.5 18.5 23.5 20' }
    case 'fishing':
      return { paths: ['M20 8 C24 12.5 25 17 25 22 L25 33 L15 33 L15 22 C15 17 16 12.5 20 8 Z'], marks: 'M20 14 V28 M14 21 L26 17' }
    case 'sail':
      return { paths: ['M20 5 C23.5 10 24.5 16 24.5 22 L24.5 35 L15.5 35 L15.5 22 C15.5 16 16.5 10 20 5 Z'], marks: 'M20 9 L20 31 L28 31 Z' }
    case 'kayak':
      return { paths: ['M20 5 C22.6 11 23 16 23 20 C23 24 22.6 29 20 35 C17.4 29 17 24 17 20 C17 16 17.4 11 20 5 Z'], marks: 'M11 17 L29 23' }
    case 'unknownSurface':
      return { paths: ['M11 11 L29 11 L29 29 L11 29 Z'] }
  }
}

/** The icon as SVG, `px` square: a dark halo under a coloured outline over a translucent fill;
 *  provisional (tentative): dashed and unfilled. */
export function trafficSvg(cls: string, domain: TrafficDomain, provisional: boolean, px: number): string {
  const shape = shapeOf(cls, domain)
  const c = domain === 'air' ? AIR_TRAFFIC : SURFACE_TRAFFIC
  const { paths, marks } = body(shape)
  const d = paths.join(' ')
  const dash = provisional ? ' stroke-dasharray="3.2 2.4"' : ''
  const fill = provisional ? 'none' : c
  const halo = `<path d="${d}" fill="none" stroke="${HALO}" stroke-width="5" stroke-linejoin="round" stroke-opacity="0.85"/>`
  const main = `<path d="${d}" fill="${fill}" fill-opacity="0.28" fill-rule="nonzero" stroke="${c}" stroke-width="2.2" stroke-linejoin="round"${dash}/>`
  const detail = marks ? `<path d="${marks}" fill="none" stroke="${HALO}" stroke-width="3.6" stroke-linecap="round" stroke-opacity="0.7"/><path d="${marks}" fill="none" stroke="${c}" stroke-width="1.5" stroke-linecap="round"${dash}/>` : ''
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="0 0 40 40">${halo}${main}${detail}</svg>`
}

/** rasterised at this multiple of its largest on-screen size, so it stays sharp on HiDPI */
const SCALE = 2
const ATLAS_PX = 28 * SCALE

export interface TrafficIcon {
  id: string
  url: string
  width: number
  height: number
  anchorX: number
  anchorY: number
}

const icons = new Map<string, TrafficIcon>()

/** A class's icon for deck.gl's IconLayer: one per class and provisional state, so 500 tracks share
 *  some 36 icons in one atlas. Built once. */
export function trafficIcon(cls: string, domain: TrafficDomain, provisional: boolean): TrafficIcon {
  const id = `t|${shapeOf(cls, domain)}|${domain}|${provisional}`
  const hit = icons.get(id)
  if (hit) return hit
  const icon: TrafficIcon = {
    id,
    url: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(trafficSvg(cls, domain, provisional, ATLAS_PX))}`,
    width: ATLAS_PX,
    height: ATLAS_PX,
    anchorX: ATLAS_PX / 2,
    anchorY: ATLAS_PX / 2,
  }
  icons.set(id, icon)
  return icon
}

/** On-screen size by tier, px: in full near the fleet, smaller a little off, small far away. */
export const ICON_PX = { near: 26, mid: 20, far: 13 } as const
