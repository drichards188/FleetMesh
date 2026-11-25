// Traffic symbols: standard map symbology for the traffic the fleet does not control, drawn by
// milsymbol (milsymbolCore.js, MIL-STD-2525D / APP-6D number codes): a frame whose shape and colour
// say how far it is identified, and a glyph inside for what kind it is, from the civilian function
// codes only. An operator who has seen a traffic display reads it at once; one who has not learns
// two colours and a dashed line (the bar's Key).
//
// The frames, in their civil meaning only (never friend, assumed friend, suspect or hostile):
//   neutral (green, square)        identified: it broadcasts (ADS-B, AIS, Remote ID) and says who it is
//   unknown (yellow, a cloud in    unidentified or not broadcasting: seen only by our own sensors, or
//     the air, four lobes on the     broadcasting with no identity (anonymous)
//     water)
//   pending (unknown's shape,      a tentative track, not yet confirmed: drawn hollow
//     finely dashed)
//   status anticipated (dashed)    a coasting or stale track: its position is extrapolated
// Symbols stand upright (the speed leader shows the course). Our own vehicles keep their round
// delivery icons (symbols.ts); no traffic frame is round, and none is blue. Far traffic is a plain
// frame without its glyph, which is illegible at that size. Pure apart from the caches:
// trafficSymbols.check.ts.
import ms from './milsymbolCore.js'
import { MIN_ALPHA, type Tier, type Track, type TrafficClass, type TrafficDomain } from './traffic.ts'

// ---- what kind it is: the function code -------------------------------------------------------

/** A symbol set (01 air, 30 sea surface) and its six-digit entity code, with the standard's name. */
export interface TrafficFunction {
  set: '01' | '30'
  entity: string
  name: string
}

const SET: Record<TrafficDomain, TrafficFunction['set']> = { air: '01', surface: '30' }
const FIXED_WING: TrafficFunction = { set: '01', entity: '120100', name: 'civilian fixed wing' }
const NO_FUNCTION: Record<TrafficDomain, TrafficFunction> = {
  air: { set: '01', entity: '000000', name: 'air track, no function' },
  surface: { set: '30', entity: '000000', name: 'sea surface track, no function' },
}

/** Each class's function: the matching civilian or merchant one where the standard has it, else
 *  the nearest general one, else none (the frame alone). Not over-specific: every airplane is
 *  civilian fixed wing. */
export const TRAFFIC_FUNCTION: Record<TrafficClass, TrafficFunction> = {
  airliner: FIXED_WING,
  bizjet: FIXED_WING,
  ga: FIXED_WING,
  seaplane: FIXED_WING,
  helicopter: { set: '01', entity: '120200', name: 'civilian rotary wing' },
  drone: { set: '01', entity: '120300', name: 'civilian unmanned aircraft' },
  unknown_air: NO_FUNCTION.air,
  cargo: { set: '30', entity: '140101', name: 'cargo, general' },
  tanker: { set: '30', entity: '140109', name: 'oiler/tanker' },
  passenger: { set: '30', entity: '140110', name: 'passenger ship' },
  ferry: { set: '30', entity: '140105', name: 'ferry' },
  tug: { set: '30', entity: '140111', name: 'tug, ocean going (civilian)' },
  // the standard has no pilot boat: a working vessel, so the general merchant ship
  pilot: { set: '30', entity: '140100', name: 'merchant ship, general' },
  fishing: { set: '30', entity: '140200', name: 'fishing vessel' },
  sailing: { set: '30', entity: '140400', name: 'leisure craft, sailing' },
  pleasure: { set: '30', entity: '140500', name: 'leisure craft, motorized' },
  small_craft: NO_FUNCTION.surface,
  unknown_surface: NO_FUNCTION.surface,
}

/** A class's function; a class it does not know, or one of the other domain, is its domain's no
 *  function, so the frame always says air or water as the track does. */
export function functionOf(cls: string, domain: TrafficDomain): TrafficFunction {
  const f = TRAFFIC_FUNCTION[cls as TrafficClass] as TrafficFunction | undefined
  return f && f.set === SET[domain] ? f : NO_FUNCTION[domain]
}

// ---- how far it is identified: the frame ------------------------------------------------------

export type Identity = 'neutral' | 'unknown' | 'pending'

export interface Frame {
  identity: Identity
  /** status anticipated: no fresh report, its position extrapolated */
  dashed: boolean
}

type FrameInput = Pick<Track, 'cooperative' | 'ids' | 'flags' | 'state' | 'static'>

/** It broadcasts and says who it is: cooperative, not anonymous, with an address or a name. */
export function isIdentified(t: Omit<FrameInput, 'state'>): boolean {
  if (!t.cooperative || t.ids.anonymous || t.flags.includes('anonymous')) return false
  const named = (s: string | undefined | null) => (s ?? '').trim() !== ''
  return named(t.ids.icao24) || t.ids.mmsi != null || named(t.ids.uas_id) || named(t.ids.callsign) || named(t.ids.name) || named(t.static?.name)
}

/** The frame a track gets: pending while tentative, else neutral when identified and unknown
 *  otherwise; dashed while coasting or stale. */
export function frameOf(t: FrameInput): Frame {
  const identity: Identity = t.state === 'tentative' ? 'pending' : isIdentified(t) ? 'neutral' : 'unknown'
  return { identity, dashed: t.state === 'coasting' || t.flags.includes('stale') }
}

/** the standard identity digit: only these three are ever written */
export const IDENTITY_CODE: Record<Identity, string> = { pending: '0', unknown: '1', neutral: '4' }

/** The 20-digit symbol code: version 10 (2525D), context 0 (reality), the identity, the symbol
 *  set, status 0 (present) or 1 (anticipated: a dashed frame), no headquarters or amplifier, the
 *  entity, no modifiers. */
export function sidcOf(fn: TrafficFunction, f: Frame): string {
  return `100${IDENTITY_CODE[f.identity]}${fn.set}${f.dashed ? '1' : '0'}000${fn.entity}0000`
}

/** A track's symbol code. */
export function trackSidc(t: FrameInput & Pick<Track, 'class' | 'domain'>): string {
  return sidcOf(functionOf(t.class, t.domain), frameOf(t))
}

// ---- drawing ----------------------------------------------------------------------------------

/** the traffic's own colours for what is not a symbol (its speed leader, stem, label, a bearing
 *  wedge): lavender in the air, sand on the water (ours are blue, green and teal) */
export const AIR_TRAFFIC = '#c7b5ff'
export const SURFACE_TRAFFIC = '#e8d6a6'
export const AIR_TRAFFIC_RGB: [number, number, number] = [199, 181, 255]
export const SURFACE_TRAFFIC_RGB: [number, number, number] = [232, 214, 166]
const HALO = '#0b0f14'
/** the dark outline round the frame, in the symbol's units */
const OUTLINE_WIDTH = 4
/** the frames' fills (milsymbol's light colours, the standard's): a hollow pending frame and its
 *  glyph are drawn in the unknown one, heavier and coarsely dashed, so they read on the dark map at
 *  these sizes (the standard's fine dash is under a pixel) */
export const UNKNOWN_FILL = 'rgb(255,255,128)'
export const NEUTRAL_FILL = 'rgb(170,255,170)'
const HOLLOW_STROKE = 10
ms.setDashArrays('12,10', ms.getDashArrays().anticipated, ms.getDashArrays().feintDummy)

/** Symbol size by tier (milsymbol's size: a neutral frame is about 1.1 of it across, an unknown
 *  one about 1.45): in full near the fleet, smaller a little off, small far away. */
export const SYMBOL_SIZE: Record<Tier, number> = { near: 20, mid: 16, far: 10 }

export interface DrawOptions {
  /** the function glyph inside the frame (off far away) */
  glyph: boolean
}

interface Drawn {
  svg: string
  /** CSS px at this size, and the anchor (the frame's centre) within them */
  width: number
  height: number
  anchor: { x: number; y: number }
}
const drawn = new Map<string, Drawn>()

/** The symbol as SVG at milsymbol size `size` (the standard's light fills; never its civilian
 *  purple, which would hide the identity colour). A pending frame is drawn hollow: provisional. */
export function sidcSvg(sidc: string, size: number, o: DrawOptions = { glyph: true }): Drawn {
  const key = `${sidc}|${size}|${o.glyph}`
  const hit = drawn.get(key)
  if (hit) return hit
  const hollow = sidc[3] === IDENTITY_CODE.pending
  const s = new ms.Symbol(sidc, {
    size,
    outlineColor: HALO,
    outlineWidth: OUTLINE_WIDTH,
    civilianColor: false,
    icon: o.glyph,
    ...(hollow ? { monoColor: UNKNOWN_FILL, strokeWidth: HOLLOW_STROKE, outlineWidth: OUTLINE_WIDTH - 1 } : {}),
  })
  const { width, height } = s.getSize()
  const out = { svg: s.asSVG(), width, height, anchor: s.getAnchor() }
  drawn.set(key, out)
  return out
}

/** A track's symbol as inline SVG (its details, the key). */
export function trackSvg(t: FrameInput & Pick<Track, 'class' | 'domain'>, size: number): string {
  return sidcSvg(trackSidc(t), size).svg
}

/** rasterised at twice the largest on-screen size, so it stays sharp on HiDPI */
const ATLAS_SIZE = 2 * SYMBOL_SIZE.near

/** A symbol for deck.gl's IconLayer, and where its frame reaches: one per code and glyph, so 500
 *  tracks share a few dozen in one atlas. */
export interface TrafficIcon {
  id: string
  url: string
  width: number
  height: number
  anchorX: number
  anchorY: number
  /** per unit of symbol size, CSS px: the icon's height (deck.gl's getSize), the frame's reach to
   *  the right of the anchor (where a label goes) and its largest reach from it (a ring round it) */
  perSize: { height: number; right: number; reach: number }
}

const icons = new Map<string, TrafficIcon>()

export function sidcIcon(sidc: string, glyph: boolean): TrafficIcon {
  const id = `${sidc}|${glyph ? 'g' : 'f'}`
  const hit = icons.get(id)
  if (hit) return hit
  const r = sidcSvg(sidc, ATLAS_SIZE, { glyph })
  const width = Math.ceil(r.width)
  const height = Math.ceil(r.height)
  const { x, y } = r.anchor
  const icon: TrafficIcon = {
    id,
    url: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(r.svg)}`,
    width,
    height,
    anchorX: x,
    anchorY: y,
    // atlas pixels to CSS pixels at symbol size s: times s / ATLAS_SIZE
    perSize: { height: height / ATLAS_SIZE, right: (width - x) / ATLAS_SIZE, reach: Math.max(x, width - x, y, height - y) / ATLAS_SIZE },
  }
  icons.set(id, icon)
  return icon
}

/** A track's icon at a tier: its glyph in full near and mid, a plain frame far away. */
export function trackIcon(t: FrameInput & Pick<Track, 'class' | 'domain'>, tier: Tier): TrafficIcon {
  return sidcIcon(trackSidc(t), tier !== 'far')
}

/** A bearing-only track's mark on its line: a plain air frame (no function, no glyph) as its frame
 *  rules give, unidentified in practice: something in the air along that line, not a position. */
export function bearingIcon(t: FrameInput): TrafficIcon {
  return sidcIcon(sidcOf(NO_FUNCTION.air, frameOf(t)), false)
}

/** On-screen extents at a tier, CSS px: the height (getSize), the reach right of the anchor, the
 *  largest reach. */
export function iconExtent(icon: TrafficIcon, tier: Tier): { height: number; right: number; reach: number } {
  const s = SYMBOL_SIZE[tier]
  return { height: icon.perSize.height * s, right: icon.perSize.right * s, reach: icon.perSize.reach * s }
}

/** An unidentified track out of focus (not near the fleet, selected, hovered or flagged) is drawn
 *  at this much of its strength, as the earlier console drew its unknowns: the many radar contacts
 *  away from the fleet recede and the rest of the picture reads. Never below MIN_ALPHA. */
export const UNKNOWN_OUT_OF_FOCUS = 0.6

/** How strongly a symbol is drawn: the track's own strength (traffic.ts trackAlpha), less for an
 *  unidentified one out of focus. */
export function symbolAlpha(alpha: number, f: Frame, focused: boolean): number {
  if (f.identity !== 'unknown' || focused) return alpha
  return Math.max(MIN_ALPHA, Math.round(alpha * UNKNOWN_OUT_OF_FOCUS * 100) / 100)
}

/** A class's symbol with a given frame as inline SVG (the key's examples). */
export function classSvg(cls: string, domain: TrafficDomain, f: Frame, size: number, glyph = true): string {
  return sidcSvg(sidcOf(functionOf(cls, domain), f), size, { glyph }).svg
}
