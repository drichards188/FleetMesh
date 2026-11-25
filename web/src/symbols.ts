import type { Kind, Vehicle } from './types'

// Vehicle icons: simple delivery pictograms drawn here, one per kind of vehicle. Air multirotor
// (copter, vtol, blimp), air fixed wing (plane), ground car (ugv), water boat (boat) and a plain
// dot for a vehicle of no known kind. Each is a 40 by 40 viewBox drawing, fill and ring in the
// domain's colour (grey when its link is stale, dashed ring).

type Shape = 'multirotor' | 'fixedwing' | 'car' | 'boat' | 'dot'

const SHAPE: Record<Kind, Shape> = {
  copter: 'multirotor',
  vtol: 'multirotor',
  blimp: 'multirotor',
  plane: 'fixedwing',
  ugv: 'car',
  boat: 'boat',
  unknown: 'dot',
}

const COLOUR: Record<Shape, string> = {
  multirotor: '#4fa3ff',
  fixedwing: '#4fa3ff',
  car: '#53c28b',
  boat: '#46c6d6',
  dot: '#aeb8c4',
}
const STALE = '#8a949f'
const OUTLINE = '#0b0f14'

export interface RenderedSymbol {
  svg: string
  anchor: { x: number; y: number }
  size: { width: number; height: number }
}

/** The pictogram's body, in a 40 by 40 box, pointing up. */
function body(shape: Shape, c: string): string {
  const s = `fill="${c}" stroke="${OUTLINE}" stroke-width="2" stroke-linejoin="round"`
  switch (shape) {
    case 'multirotor':
      return (
        `<path d="M13 13 L27 27 M27 13 L13 27" stroke="${OUTLINE}" stroke-width="6" stroke-linecap="round"/>` +
        `<path d="M13 13 L27 27 M27 13 L13 27" stroke="${c}" stroke-width="3" stroke-linecap="round"/>` +
        [[12, 12], [28, 12], [12, 28], [28, 28]].map(([x, y]) => `<circle cx="${x}" cy="${y}" r="5" ${s}/>`).join('') +
        `<rect x="16" y="16" width="8" height="8" rx="2" ${s}/>`
      )
    case 'fixedwing':
      return `<path d="M20 5 L23 15 L36 22 L36 25 L23 22 L22 31 L26 34 L26 36 L20 34.5 L14 36 L14 34 L18 31 L17 22 L4 25 L4 22 L17 15 Z" ${s}/>`
    case 'car':
      return (
        `<rect x="11" y="6" width="18" height="28" rx="6" ${s}/>` +
        `<rect x="14" y="12" width="12" height="7" rx="2" fill="${OUTLINE}" opacity="0.55"/>` +
        `<rect x="14" y="24" width="12" height="5" rx="2" fill="${OUTLINE}" opacity="0.35"/>`
      )
    case 'boat':
      return `<path d="M20 4 C27 12 29 22 28 34 L12 34 C11 22 13 12 20 4 Z" ${s}/><rect x="16" y="22" width="8" height="6" rx="1.5" fill="${OUTLINE}" opacity="0.45"/>`
    case 'dot':
      return `<circle cx="20" cy="20" r="8" ${s}/>`
  }
}

function drawing(kind: Kind, stale: boolean, px: number, heading?: number): string {
  const shape = SHAPE[kind]
  const c = stale ? STALE : COLOUR[shape]
  const ring = `<circle cx="20" cy="20" r="18" fill="${OUTLINE}" fill-opacity="0.35" stroke="${c}" stroke-width="1.5"${stale ? ' stroke-dasharray="3 3"' : ''}/>`
  const rot = heading != null && shape !== 'dot' ? ` transform="rotate(${heading} 20 20)"` : ''
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="0 0 40 40">${ring}<g${rot}>${body(shape, c)}</g></svg>`
}

const cache = new Map<string, RenderedSymbol>()

/** A vehicle's icon as inline SVG (the fleet list and the details header). */
export function symbolSvg(v: Pick<Vehicle, 'kind'>, opts: { stale: boolean; size?: number }): RenderedSymbol {
  const kind: Kind = v.kind ?? 'unknown'
  const px = opts.size ?? 28
  const key = `${kind}|${opts.stale}|${px}`
  const hit = cache.get(key)
  if (hit) return hit
  const out = { svg: drawing(kind, opts.stale, px), anchor: { x: px / 2, y: px / 2 }, size: { width: px, height: px } }
  cache.set(key, out)
  return out
}

/** Icons are rasterised at this multiple of their on-screen size, so they stay sharp on HiDPI. */
const ICON_SCALE = 2
const ICON_PX = 28

/** A vehicle's icon as a deck.gl IconLayer icon. One entry per kind and stale state, so the whole
 *  fleet shares a few icons in one atlas. */
export interface FrameIcon {
  id: string
  url: string
  width: number
  height: number
  anchorX: number
  anchorY: number
  /** on-screen height in CSS pixels */
  size: number
  /** half extents of the icon around its anchor, CSS pixels: where labels go */
  halfW: number
  halfH: number
}

const frames = new Map<string, FrameIcon>()

export function frameIcon(v: Pick<Vehicle, 'kind'>, stale: boolean): FrameIcon {
  const kind: Kind = v.kind ?? 'unknown'
  const id = `${kind}|${stale}`
  const hit = frames.get(id)
  if (hit) return hit
  const px = ICON_PX * ICON_SCALE
  const icon: FrameIcon = {
    id,
    url: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(drawing(kind, stale, px))}`,
    width: px,
    height: px,
    anchorX: px / 2,
    anchorY: px / 2,
    size: ICON_PX,
    halfW: ICON_PX / 2,
    halfH: ICON_PX / 2,
  }
  frames.set(id, icon)
  return icon
}
