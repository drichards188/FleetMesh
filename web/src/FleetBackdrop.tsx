import { useEffect, useRef } from 'react'
import './backdrop.css'
import { symbolSvg } from './symbols'
import type { Kind } from './types'

/** The main menu's backdrop: a harbour city in cross-section, the sky over the water and the
 *  land, each band named at the screen's edge. Delivery drones and a plane fly in the air, cars
 *  drive ashore, boats work the water, and hubs are where they base: a floating dock out in the
 *  harbour, a depot and a rooftop vertiport on the quay. Each vehicle is the console's own delivery
 *  pictogram (symbols.ts), carrying a parcel out from its hub and coming back empty, and each is
 *  joined to the network it would use (sim/links.toml): cellular to the nearest hub, a line of
 *  sight radio the terrain can cut, a mesh among the cars, SATCOM through a passing LEO satellite
 *  when nothing else reaches. Now and then one vehicle's link degrades (it is drawn red, its
 *  icon goes grey and its packets die) and the vehicle fails over to its backup link until it
 *  recovers. Interference is only ever something a vehicle receives: nothing here emits any.
 *
 *  Decoration only: none of it is a vehicle, a link or a figure of any run, and it says no number.
 *  It reads the menu's layout (the gutters and the gap between the two columns, which the panels
 *  leave open) and keeps the vehicles that hold station there. One canvas, at most 30 frames a
 *  second, DPR capped at 1.5, nothing while the tab is hidden, one still frame under
 *  prefers-reduced-motion. Its colours are the menu's (menu.css) and the console's (index.css),
 *  with a fallback for each. The canvas takes no pointer events and sits under the panels. */

type Dom = 'air' | 'ground' | 'surface'
type V = 'copter' | 'plane' | 'car' | 'boat' | 'depot' | 'tower' | 'dock' | 'sat'
type Hub = 'depot' | 'tower' | 'dock'
type Vehicle = 'copter' | 'plane' | 'car' | 'boat'
type LinkKind = 'cellular' | 'radio' | 'mesh' | 'satcom' | 'lost'

const DOMAIN: Record<V, Dom | 'orbit'> = {
  copter: 'air', plane: 'air', car: 'ground', boat: 'surface', depot: 'ground', tower: 'ground', dock: 'surface', sat: 'orbit',
}
/** the console's icon for each vehicle (symbols.ts) */
const KIND: Record<Vehicle, Kind> = { copter: 'copter', plane: 'plane', car: 'ugv', boat: 'boat' }
/** the ring's radius (px at scale 1); the icon's ring is 18 of its 40 */
const RADIUS: Record<Vehicle, number> = { copter: 15, plane: 17, car: 14, boat: 16 }
/** the short name by one of each kind: platforms.toml's names */
const TAG: Record<V, string> = {
  copter: 'Delivery drone', plane: 'Delivery plane', car: 'Delivery car', boat: 'Delivery boat', depot: 'Hub', tower: 'Hub', dock: 'Hub', sat: 'Satellite',
}
/** each hub's antenna (where its links start, px at scale 1 from its base, up is negative), its
 *  roof pad for drones and its height */
const HUB: Record<Hub, { mast: [number, number]; pad: number; top: number }> = {
  depot: { mast: [11, -36], pad: -19, top: 40 },
  tower: { mast: [10, -56], pad: -46, top: 62 },
  dock: { mast: [-17, -26], pad: -20, top: 30 },
}
const isHub = (v: V): v is Hub => v === 'depot' || v === 'tower' || v === 'dock'
const isVehicle = (v: V): v is Vehicle => v === 'copter' || v === 'plane' || v === 'car' || v === 'boat'

/** The links the backdrop draws, in the style the legend shows. `color` is a CSS custom property
 *  (menu.css, index.css) with its fallback. */
const LINKS: Record<LinkKind, { label: string; color: string; fallback: string; dash: number[]; width: number; alpha: number }> = {
  cellular: { label: 'Cellular', color: '--mm-link', fallback: '#8aa2bd', dash: [], width: 1, alpha: 0.45 },
  radio: { label: 'Radio', color: '--dom-air', fallback: '#7fb8ff', dash: [7, 3], width: 0.9, alpha: 0.45 },
  mesh: { label: 'Mesh', color: '--dom-ground', fallback: '#c9b98a', dash: [3, 2], width: 0.9, alpha: 0.45 },
  satcom: { label: 'SATCOM', color: '--violet', fallback: '#c49bff', dash: [1.5, 4], width: 1.1, alpha: 0.55 },
  lost: { label: 'Degraded', color: '--bad', fallback: '#ff5d5d', dash: [3, 3], width: 1.1, alpha: 0.8 },
}

interface Node {
  id: number
  v: V
  d: Dom | 'orbit'
  /** its origin: a vehicle's contact with the ground or the water (its centre in the air), a
   *  hub's base */
  x: number
  y: number
  /** facing: 1 east, -1 west; its icon's heading (rad, 0 north) follows */
  dir: number
  ang: number
  sp: number
  ph: number
  /** lit by a delivery or a reconnection: 1, fading to 0 */
  lit: number
  /** air: its cruising height in the sky (0 the top, 1 the bottom) */
  alt: number
  /** the ring's radius and the scale of a hub, and the ring's centre, each frame */
  r: number
  k: number
  cx: number
  cy: number
  tag: boolean
  /** a station it hovers about (x) */
  hold?: number
  /** a route out from its hub: it patrols lo to hi, loaded when it leaves `base` */
  pat: boolean
  lo: number
  hi: number
  base: number
  /** the height of its hub's pad and of its drop, for a drone on a route */
  padY: number
  farY: number
  carry: boolean
  /** seconds left loading or handing over */
  pause: number
  /** how far it has faded in (1) or out (0), and which way it is going: a scene's change */
  fade: number
  goal: 0 | 1
}
interface Geo {
  w: number
  h: number
  /** the nodes' scale */
  S: number
  sea: number
  /** the water's depth as a band */
  wd: number
  /** the solid surface (the harbour floor under the water, the land ashore) every STEP px across */
  ground: Float32Array
  /** where the land begins */
  coast: number
  /** the top of the sky an aircraft uses, and the satellites' height */
  top: number
  satY: number
  water: Path2D
  sky: Path2D
  /** the stations the scenes use */
  west: [number, number]
  east: [number, number]
  dockX: number
  floatX: number
  depotX: number
  towerX: number | null
  houses: number[]
}
/** what the menu's panels leave open */
interface Open {
  /** the left gutter's right edge and the right gutter's left edge */
  left: number
  right: number
  /** the gap between the two columns */
  gap: [number, number] | null
  /** the top of the panels */
  top: number
  /** the title and its pitch (x0, y0, x1, y1), where no tag goes */
  title: [number, number, number, number] | null
}
interface Link {
  kind: LinkKind
  /** the vehicle */
  a: Node
  /** its hub, its satellite or the car it meshes with */
  b: Node
  /** its quality: 1, falling to 0 at the edge of its range */
  q: number
  key: string
  /** the link a vehicle fails over to while its own is degraded */
  backup?: boolean
}
interface Ghost {
  a: Node
  b: Node
  /** where the terrain cut it, along a to b */
  u: number
  t0: number
}
interface Net {
  links: Link[]
  /** when each link came up */
  since: Map<string, number>
  ghosts: Ghost[]
  /** when the terrain last cut one */
  cutT: number
  /** a vehicle whose link is degraded for a while */
  burst: { key: string; vid: number; t0: number } | null
}

const STEP = 4
const FRAME_MS = 1000 / 30
const GHOST_S = 1.8
const FLASH_S = 0.9
const BURST_S = 3
const CUT_EVERY_S = 5
/** a scene is held this long, and fades into the next over FADE_S */
const SCENE_S = 15
const FADE_S = 2.5
/** the scene the opening one first changes to (SCENES) */
const FIRST_CHANGE = 1

/** a small seeded generator, so the backdrop opens the same each time */
function rng(seed: number) {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const smooth = (a: number, b: number, u: number) => {
  const t = Math.min(1, Math.max(0, (u - a) / (b - a)))
  return t * t * (3 - 2 * t)
}
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v))
/** a number in [0, 1) from any number, steady for the same input */
const hash = (v: number) => {
  const s = Math.sin(v * 12.9898) * 43758.5453
  return s - Math.floor(s)
}

// ---- the scene's shape ---------------------------------------------------------------------------

function geometry(w: number, h: number, open: Open): Geo {
  const S = clamp(Math.min(w / 1250, h / 780), 0.8, 1.45)
  const sea = Math.round(h * 0.76)
  const wd = h * 0.1
  const top = clamp(h * 0.14, 96, 140)
  // clear of the band names up the left edge
  const west: [number, number] = [44, Math.max(44, open.left - 10)]
  const east: [number, number] = [Math.min(w - 12, open.right + 10), w - 12]
  const ww = west[1] - west[0], ew = east[1] - east[0]
  // the land fills the east gutter (its shore under the panel's edge); with no gutter, the far east
  const coast = ew >= 70 * S ? east[0] - 10 * S : w * 0.84
  const gapX = open.gap ? (open.gap[0] + open.gap[1]) / 2 : w * 0.36
  const dockX = ww >= 60 * S ? west[0] + ww * (ww >= 96 * S ? 0.3 : 0.5) : gapX
  const floatX = ww >= 60 * S ? clamp(west[0] + ww * 0.86, dockX + 44 * S, Math.max(dockX + 44 * S, west[1] - 4)) : gapX
  const depotX = (ew >= 70 * S ? east[0] : coast) + 24 * S
  const towerX = w - 12 - depotX >= 130 * S ? depotX + 48 * S : null
  const n = Math.ceil(w / STEP) + 2
  const ground = new Float32Array(n)
  const bump = (u: number, c: number, s: number) => Math.exp(-(((u - c) / s) ** 2))
  for (let i = 0; i < n; i++) {
    const x = i * STEP
    // the harbour floor under the water; the land rising from a quay to low hills at the edge
    const land = smooth(coast - 110 * S, coast + 10 * S, x)
    const rise = h * (0.034 + 0.007 * Math.sin(x / (38 * S)) + 0.04 * bump(x, w + 30 * S, 110 * S))
    ground[i] = sea + wd * (1 - land) - rise * land
  }
  const water = new Path2D()
  water.moveTo(0, sea)
  water.lineTo(w, sea)
  for (let i = n - 1; i >= 0; i--) water.lineTo(i * STEP, clamp(ground[i], sea, sea + wd))
  water.closePath()
  const sky = new Path2D()
  sky.moveTo(0, 0)
  sky.lineTo(w, 0)
  for (let i = n - 1; i >= 0; i--) sky.lineTo(i * STEP, Math.min(sea, ground[i]))
  sky.closePath()
  // houses to deliver to, after the hubs
  const houses: number[] = []
  const from = (towerX ?? depotX) + (towerX != null ? 44 : 40) * S
  for (let x = from; x < w - 14 && houses.length < 4; x += 26 * S) houses.push(x)
  const satY = clamp((48 + open.top) / 2, 60, top - 12)
  return { w, h, S, sea, wd, ground, coast, top, satY, water, sky, west, east, dockX, floatX, depotX, towerX, houses }
}

const groundAt = (g: Geo, x: number) => g.ground[Math.max(0, Math.min(g.ground.length - 1, Math.round(x / STEP)))]
const inWater = (g: Geo, x: number) => groundAt(g, x) > g.sea + 10
/** a height `a` in the sky (0 its top, 1 its bottom) over x, for a ring of radius r */
function skyY(g: Geo, x: number, a: number, r: number) {
  const top = g.top + r + 8
  const bottom = Math.min(g.sea, groundAt(g, x)) - r * 1.5
  return top + clamp(a, 0, 1) * Math.max(0, bottom - top)
}

function measure(canvas: HTMLCanvasElement, w: number): Open {
  const c = canvas.getBoundingClientRect()
  const box = (sel: string) => {
    const r = canvas.parentElement?.querySelector(sel)?.getBoundingClientRect()
    return r && r.width > 0 ? r : null
  }
  const l = box('.mm-left'), r = box('.mm-right'), id = box('.mm-ident')
  const title: Open['title'] = id ? [id.left - c.left - 10, id.top - c.top - 10, id.right - c.left + 10, id.bottom - c.top + 10] : null
  // one column (a small screen): nothing between, a narrow gutter each side
  if (!l || !r) return { left: 16, right: w - 16, gap: null, top: 82, title }
  return {
    left: l.left - c.left,
    right: r.right - c.left,
    gap: r.left - l.right > 24 ? [l.right - c.left, r.left - c.left] : null,
    top: Math.max(56, r.top - c.top),
    title,
  }
}

// ---- the cast ------------------------------------------------------------------------------------

/** what a scene places its nodes with */
interface Cast {
  g: Geo
  S: number
  /** a laptop's screen or larger: room for the extras */
  full: boolean
  add: (v: V, o?: Partial<Node>) => Node
  /** the hubs and the two satellites every scene has */
  common: () => void
  /** a route out from `base` to `far` (x), the pad's and the drop's heights for a drone */
  route: (base: number, far: number, padY?: number, farY?: number) => Partial<Node>
  /** the height of a hub's pad over its base, and of the drop over a house or the float */
  padOf: (h: Hub) => number
  dropAt: (x: number) => number
}

/** One delivery job with its own focus, held SCENE_S before the backdrop moves to another. Each
 *  has a vehicle on every domain where the panels leave it seen, and each link it uses has a
 *  hub or a satellite at its other end. */
interface Scene {
  name: string
  cast: (c: Cast) => void
}

const SCENES: Scene[] = [
  {
    // one of each kind of thing: the opening scene
    name: 'morning parcel run',
    cast: (c) => {
      const { g, S, full, add, common, route, padOf, dropAt } = c
      common()
      add('plane', { alt: 0.1, sp: 19 })
      add('copter', { alt: 0.5, sp: 15, ...route(g.dockX, g.floatX, padOf('dock'), dropAt(g.floatX)) })
      const east = g.towerX ?? g.depotX
      add('copter', { alt: 0.56, sp: 13, ...route(east, g.houses[0] ?? east + 60 * S, padOf(g.towerX != null ? 'tower' : 'depot'), dropAt(g.houses[0] ?? east + 60 * S)) })
      add('car', { sp: 7, ...route(g.depotX, g.houses[1] ?? g.houses[0] ?? g.depotX + 70 * S) })
      add('boat', { sp: 9, ...route(g.dockX, g.coast - 40 * S) })
      if (full) add('car', { sp: 5, ...route(g.depotX, g.houses[0] ?? g.depotX + 60 * S) })
    },
  },
  {
    // the harbour crossing: parcels by boat from the dock to the quay, a drone to the float
    name: 'harbour crossing',
    cast: (c) => {
      const { g, S, full, add, common, route, padOf, dropAt } = c
      common()
      add('boat', { sp: 8, ...route(g.dockX, g.coast - 40 * S) })
      add('boat', { sp: 12, ...route(g.dockX, g.coast - 90 * S) })
      if (full) add('boat', { sp: 6, ...route(g.dockX, g.floatX + 20 * S) })
      add('copter', { alt: 0.45, sp: 16, ...route(g.dockX, g.floatX, padOf('dock'), dropAt(g.floatX)) })
      add('plane', { alt: 0.14, sp: 21 })
      add('car', { sp: 6, ...route(g.depotX, g.houses[0] ?? g.depotX + 60 * S) })
    },
  },
  {
    // rooftop deliveries: drones lifting off the vertiport and the depot to the houses
    name: 'rooftop drone deliveries',
    cast: (c) => {
      const { g, S, full, add, common, route, padOf, dropAt } = c
      common()
      const pad = g.towerX != null ? 'tower' : 'depot'
      const at = g.towerX ?? g.depotX
      add('copter', { alt: 0.5, sp: 14, ...route(at, g.houses[0] ?? at + 60 * S, padOf(pad), dropAt(g.houses[0] ?? at + 60 * S)) })
      add('copter', { alt: 0.62, sp: 11, ...route(at, g.houses[2] ?? g.houses[0] ?? at + 90 * S, padOf(pad), dropAt(g.houses[2] ?? g.houses[0] ?? at + 90 * S)) })
      if (full) add('copter', { alt: 0.4, sp: 17, ...route(g.dockX, g.floatX, padOf('dock'), dropAt(g.floatX)) })
      add('car', { sp: 7, ...route(g.depotX, g.houses[1] ?? g.depotX + 60 * S) })
      add('boat', { sp: 9, ...route(g.dockX, g.coast - 40 * S) })
    },
  },
  {
    // medical supplies by air: the plane on the radio to each hub it passes, drones for the last leg
    name: 'medical supplies by air',
    cast: (c) => {
      const { g, S, full, add, common, route, padOf, dropAt } = c
      common()
      add('plane', { alt: 0.09, sp: 20 })
      if (full) add('plane', { alt: 0.22, sp: 16, dir: -1 })
      const at = g.towerX ?? g.depotX
      add('copter', { alt: 0.52, sp: 14, ...route(at, g.houses[1] ?? at + 70 * S, padOf(g.towerX != null ? 'tower' : 'depot'), dropAt(g.houses[1] ?? at + 70 * S)) })
      add('copter', { alt: 0.46, sp: 15, ...route(g.dockX, g.floatX, padOf('dock'), dropAt(g.floatX)) })
      add('car', { sp: 8, ...route(g.depotX, g.houses[0] ?? g.depotX + 60 * S) })
    },
  },
  {
    // the neighbourhood van loop: cars each on their own stretch, meshing where the hills allow
    name: 'neighbourhood van loop',
    cast: (c) => {
      const { g, S, full, add, common, route, padOf, dropAt } = c
      common()
      const h = g.houses
      add('car', { sp: 7, ...route(g.depotX, h[0] ?? g.depotX + 50 * S) })
      add('car', { sp: 6, ...route(g.depotX, h[1] ?? g.depotX + 80 * S) })
      if (h.length >= 3) add('car', { sp: 8, ...route(g.depotX, h[Math.min(3, h.length - 1)]) })
      const at = g.towerX ?? g.depotX
      if (full) add('copter', { alt: 0.58, sp: 12, ...route(at, h[1] ?? at + 70 * S, padOf(g.towerX != null ? 'tower' : 'depot'), dropAt(h[1] ?? at + 70 * S)) })
      add('boat', { sp: 8, ...route(g.dockX, g.coast - 40 * S) })
      add('plane', { alt: 0.12, sp: 18 })
    },
  },
  {
    // out in the harbour, beyond the cells: boats on SATCOM, the plane overhead
    name: 'offshore delivery',
    cast: (c) => {
      const { g, S, full, add, common, route, padOf, dropAt } = c
      common()
      add('boat', { sp: 14, ...route(g.dockX, g.coast - 40 * S) })
      add('boat', { sp: 10, ...route(g.dockX, g.coast - 120 * S) })
      add('plane', { alt: 0.1, sp: 22 })
      add('copter', { alt: 0.5, sp: 15, ...route(g.dockX, g.floatX, padOf('dock'), dropAt(g.floatX)) })
      if (full) add('car', { sp: 6, ...route(g.depotX, g.houses[0] ?? g.depotX + 60 * S) })
    },
  },
]

/** a scene's nodes (ids from `ids`, so they stay unique across scenes) */
function seed(g: Geo, k: number, ids: { n: number }): Node[] {
  const r = rng(7 + k * 101)
  const S = g.S
  // all of them from a laptop's screen up; fewer on a small one, where they would crowd the words
  const full = (g.w * g.h) / (1280 * 720) >= 0.6
  const nodes: Node[] = []
  const seen = new Set<string>()
  const add = (v: V, o: Partial<Node> = {}): Node => {
    const rad = (isVehicle(v) ? RADIUS[v] : v === 'sat' ? 10 : 0) * S
    const n: Node = {
      id: ids.n++,
      v,
      d: DOMAIN[v],
      x: r() * g.w,
      y: 0,
      dir: r() < 0.5 ? -1 : 1,
      ang: 0,
      sp: 0,
      ph: r() * Math.PI * 2,
      lit: 0,
      alt: 0.5,
      r: rad,
      k: S,
      cx: 0,
      cy: 0,
      tag: !seen.has(TAG[v]),
      pat: false,
      lo: 0,
      hi: g.w,
      base: 0,
      padY: 0,
      farY: 0,
      carry: false,
      pause: 0,
      fade: 1,
      goal: 1,
      ...o,
    }
    seen.add(TAG[v])
    nodes.push(n)
    return n
  }
  const hubY = (h: Hub) => (h === 'dock' ? g.sea : groundAt(g, h === 'tower' ? (g.towerX ?? g.depotX) : g.depotX))
  const common = () => {
    add('dock', { x: g.dockX })
    add('depot', { x: g.depotX })
    if (g.towerX != null) add('tower', { x: g.towerX })
    add('sat', { x: g.w * 0.2, sp: 26, dir: 1 })
    add('sat', { x: g.w * 0.7, sp: 26, dir: 1 })
  }
  const route = (base: number, far: number, padY = 0, farY = 0): Partial<Node> => ({
    pat: true, base, lo: Math.min(base, far), hi: Math.max(base, far), padY, farY,
  })
  const padOf = (h: Hub) => hubY(h) + HUB[h].pad * S
  const dropAt = (x: number) => (inWater(g, x) ? g.sea : groundAt(g, x)) - 26 * S
  SCENES[k].cast({ g, S, full, add, common, route, padOf, dropAt })

  for (const n of nodes) {
    if (isHub(n.v)) n.y = n.v === 'dock' ? g.sea : groundAt(g, n.x)
    else if (n.pat) {
      n.x = n.lo + r() * (n.hi - n.lo)
      n.carry = n.dir * (n.x - n.base) > 0
      if (n.d === 'air') n.padY -= n.r
    } else if (n.d === 'surface') n.x = r() * Math.max(40, g.coast - 80 * S)
    if (n.d === 'air' && !n.pat) n.y = skyY(g, n.x, n.alt, n.r)
    if (n.d === 'air' && n.pat) n.y = n.padY
    if (n.d === 'surface' && !isHub(n.v)) n.y = g.sea
    if (n.d === 'ground' && !isHub(n.v)) n.y = groundAt(g, n.x)
    if (n.d === 'orbit') n.y = g.satY
    n.ang = n.dir * Math.PI / 2
  }
  return nodes
}

// ---- motion --------------------------------------------------------------------------------------

/** at the end of a route: a stop to hand the parcel over (or load the next), then back */
function arrive(n: Node, t: number) {
  const atBase = Math.abs(n.x - n.base) < 1
  n.pause = 1.4 + 1.6 * hash(t + n.id)
  n.dir = -n.dir
  n.carry = atBase
  // a parcel delivered
  if (!atBase) n.lit = 1
}

function step(nodes: Node[], g: Geo, dt: number, t: number) {
  const S = g.S
  for (const n of nodes) {
    n.lit = Math.max(0, n.lit - dt * 0.9)
    n.ph += dt * 0.5
    if (isHub(n.v)) continue
    if (n.d === 'orbit') {
      n.x += n.dir * n.sp * S * dt
      const m = 40 * S
      if (n.x > g.w + m) n.x = -m
      else if (n.x < -m) n.x = g.w + m
      n.y = g.satY
      continue
    }
    if (n.pause > 0) n.pause -= dt
    else if (n.pat) {
      n.x += n.dir * n.sp * S * dt
      if (n.dir > 0 && n.x >= n.hi) {
        n.x = n.hi
        arrive(n, t)
      } else if (n.dir < 0 && n.x <= n.lo) {
        n.x = n.lo
        arrive(n, t)
      }
    } else if (n.hold != null) {
      n.x = n.hold + Math.sin(t * 0.07 + n.ph) * 5 * S
    } else {
      n.x += n.dir * n.sp * S * dt
      const m = n.r * 3
      if (n.x > g.w + m) n.x = -m
      else if (n.x < -m) n.x = g.w + m
    }
    if (n.d === 'air') {
      const cruise = skyY(g, n.x, n.alt + 0.02 * Math.sin(n.ph * 0.5), n.r)
      let want = cruise
      if (n.pat) {
        // up from the pad and down to the drop: low at both ends, at its height between
        const dB = Math.abs(n.x - n.base)
        const dF = Math.abs(n.x - (n.base === n.lo ? n.hi : n.lo))
        const f = smooth(0, 90 * S, Math.min(dB, dF))
        const y0 = dB < dF ? n.padY : n.farY
        want = y0 + (cruise - y0) * f
      }
      n.y += (want - n.y) * Math.min(1, dt * 2.5)
    } else if (n.d === 'ground') {
      n.y = groundAt(g, n.x)
    } else {
      n.y = g.sea + Math.sin(n.ph * 2) * 0.8
    }
    // the icon's heading follows its way, turning through north at the end of a route
    const slope = n.d === 'ground' ? Math.atan2(groundAt(g, n.x + 6) - groundAt(g, n.x - 6), 12) * n.dir * 0.6 : 0
    const want = n.dir * (Math.PI / 2) + slope + (n.d === 'air' ? 0.05 * Math.sin(n.ph * 1.7) : 0)
    n.ang += (want - n.ang) * Math.min(1, dt * 3)
  }
  for (const n of nodes) {
    n.cx = n.x
    n.cy = n.d === 'ground' && !isHub(n.v) ? n.y - n.r * 0.95 : n.d === 'surface' && !isHub(n.v) ? n.y - n.r * 0.8 : n.y
  }
}

/** where the terrain cuts the line from a to b (a fraction along it), or null if it is clear */
function cut(g: Geo, ax: number, ay: number, bx: number, by: number): number | null {
  const n = Math.ceil(Math.hypot(bx - ax, by - ay) / 10)
  for (let i = 1; i < n; i++) {
    const u = i / n
    const x = ax + (bx - ax) * u
    const y = ay + (by - ay) * u
    if (x >= 0 && x <= g.w && y > Math.min(groundAt(g, x), g.sea + 2) - 1) return u
  }
  return null
}

/** where a node's links start: a hub's antenna, else its centre */
const end = (n: Node): [number, number] => (isHub(n.v) ? [n.x + HUB[n.v].mast[0] * n.k, n.y + HUB[n.v].mast[1] * n.k] : [n.cx, n.cy])
const offscreen = (n: Node) => n.fade <= 0

/** this frame's links, and the pairs in range the terrain cut (with where). A vehicle takes its
 *  first link in range; one whose link is degraded (`vid`) also shows the next, its backup. */
function network(nodes: Node[], g: Geo, vid: number): { links: Link[]; blocked: Map<string, number> } {
  const S = g.S
  const links: Link[] = []
  const blocked = new Map<string, number>()
  const live = nodes.filter((n) => !offscreen(n))
  const hubs = live.filter((n) => isHub(n.v))
  const sats = live.filter((n) => n.v === 'sat')
  const cars = live.filter((n) => n.v === 'car')
  const key = (k: LinkKind, a: Node, b: Node) => `${k}:${Math.min(a.id, b.id)}:${Math.max(a.id, b.id)}`
  const nearest = (n: Node, list: Node[]) => {
    const [x, y] = end(n)
    let best: Node | undefined
    let bd = Infinity
    for (const o of list) {
      if (o === n) continue
      const [ox, oy] = end(o)
      const d = Math.hypot(ox - x, oy - y)
      if (d < bd) { bd = d; best = o }
    }
    return best
  }
  // the satellite overhead: the one nearest in x
  const overhead = (n: Node) => {
    let best: Node | undefined
    for (const s of sats) if (!best || Math.abs(s.cx - n.cx) < Math.abs(best.cx - n.cx)) best = s
    return best
  }
  const tryLink = (kind: LinkKind, a: Node, to: Node | undefined, range: number, los: boolean): Link | null => {
    if (!to) return null
    const [ax, ay] = end(a), [bx, by] = end(to)
    const d = Math.hypot(bx - ax, by - ay)
    if (d > range) return null
    const k = key(kind, a, to)
    if (los) {
      const u = cut(g, ax, ay, bx, by)
      if (u != null) {
        blocked.set(k, u)
        return null
      }
    }
    return { kind, a, b: to, q: 1 - smooth(0.72, 1, d / range), key: k }
  }
  const CELL = 330 * S
  const seen = new Set<string>()
  for (const n of live) {
    if (!isVehicle(n.v)) continue
    let options: (() => Link | null)[]
    if (n.v === 'copter') {
      options = [
        () => tryLink('cellular', n, nearest(n, hubs), CELL, false),
        () => tryLink('radio', n, nearest(n, hubs), g.w * 0.7, true),
        () => tryLink('satcom', n, overhead(n), Infinity, false),
      ]
    } else if (n.v === 'plane') {
      options = [
        () => tryLink('radio', n, nearest(n, hubs), g.w * 0.9, true),
        () => tryLink('satcom', n, overhead(n), Infinity, false),
      ]
    } else if (n.v === 'car') {
      options = [
        () => tryLink('cellular', n, nearest(n, hubs), CELL, false),
        () => tryLink('mesh', n, nearest(n, [...cars, ...hubs]), 190 * S, true),
      ]
    } else {
      options = [
        () => tryLink('cellular', n, nearest(n, hubs), CELL, false),
        () => tryLink('satcom', n, overhead(n), Infinity, false),
      ]
    }
    const want = n.id === vid ? 2 : 1
    const got: Link[] = []
    for (const f of options) {
      const l = f()
      if (l) got.push(l)
      if (got.length >= want) break
    }
    got.forEach((l, i) => {
      if (seen.has(l.key)) return
      seen.add(l.key)
      links.push(i ? { ...l, backup: true } : l)
    })
  }
  return { links, blocked }
}

// ---- drawing -------------------------------------------------------------------------------------

interface Palette {
  dom: Record<Dom, string>
  link: Record<LinkKind, string>
  hub: string
  bg: string
  disc: string
  lamp: string
  parcel: string
  skyTop: string
  skyLow: string
  water: string
  deep: string
  land: string
  line: string
  font: string
}

function palette(el: Element): Palette {
  const css = getComputedStyle(el)
  const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback
  const link = Object.fromEntries((Object.keys(LINKS) as LinkKind[]).map((k) => [k, v(LINKS[k].color, LINKS[k].fallback)])) as Record<LinkKind, string>
  return {
    dom: { air: v('--dom-air', '#7fb8ff'), ground: v('--dom-ground', '#c9b98a'), surface: v('--dom-surface', '#6fe0c4') },
    link,
    hub: v('--text', '#d7dde4'),
    bg: v('--bg', '#0b0f14'),
    disc: v('--mm-disc', 'rgba(8, 11, 15, 0.8)'),
    lamp: v('--mm-lamp', '#fff3c4'),
    parcel: v('--accent', '#ffb347'),
    skyTop: v('--mm-sky', '#0a131d'),
    skyLow: v('--mm-sky-low', '#11202e'),
    water: v('--mm-water', '#0b2227'),
    deep: v('--mm-deep', '#0d1325'),
    land: v('--mm-land', '#201d16'),
    line: v('--line', '#233040'),
    font: v('--mono', 'monospace'),
  }
}

function spaced(ctx: CanvasRenderingContext2D, em: string) {
  const c = ctx as CanvasRenderingContext2D & { letterSpacing?: string }
  if ('letterSpacing' in c) c.letterSpacing = em
}

/** a band's name, written up the screen's edge beside a bracket from y0 to y1 */
function caption(ctx: CanvasRenderingContext2D, text: string, x: number, y0: number, y1: number, color: string, font: string, side: 1 | -1) {
  if (y1 - y0 < 30) return
  ctx.save()
  ctx.font = `600 9px ${font}`
  spaced(ctx, '0.2em')
  const len = ctx.measureText(text).width
  const bx = x - side * 9
  ctx.strokeStyle = color
  ctx.lineWidth = 1
  ctx.globalAlpha = 0.4
  ctx.beginPath()
  ctx.moveTo(bx + side * 4, y0 + 0.5)
  ctx.lineTo(bx, y0 + 0.5)
  ctx.lineTo(bx, y1 - 0.5)
  ctx.lineTo(bx + side * 4, y1 - 0.5)
  ctx.stroke()
  ctx.globalAlpha = 0.85
  ctx.fillStyle = color
  ctx.translate(x, (y0 + y1) / 2)
  ctx.rotate(-Math.PI / 2)
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  if (len < y1 - y0 - 8) ctx.fillText(text, 0, 0)
  ctx.restore()
}

/** a skyline of plain blocks along y, from x0 to x1, with a few lit windows */
function skyline(ctx: CanvasRenderingContext2D, seedN: number, x0: number, x1: number, y: number, maxH: number, fill: string, S: number) {
  const r = rng(seedN)
  let x = x0
  while (x < x1) {
    const bw = (7 + r() * 14) * S
    const bh = (0.25 + 0.75 * r()) * maxH
    ctx.fillStyle = fill
    ctx.fillRect(x, y - bh, bw, bh)
    if (bh > 12 * S && r() < 0.7) {
      ctx.fillStyle = 'rgba(255, 243, 196, 0.22)'
      for (let k = 0; k < 3; k++) ctx.fillRect(x + (1.5 + r() * (bw - 4)), y - bh + 3 + r() * (bh - 7), 1.2, 1.2)
    }
    x += bw + (1 + r() * 4) * S
  }
}

/** what does not move, drawn once a size */
function drawStatic(ctx: CanvasRenderingContext2D, g: Geo, p: Palette, open: Open) {
  const { w, h, sea } = g
  const S = g.S
  ctx.clearRect(0, 0, w, h)
  // the sky, lighter towards the horizon
  const sky = ctx.createLinearGradient(0, 0, 0, sea)
  sky.addColorStop(0, p.skyTop)
  sky.addColorStop(1, p.skyLow)
  ctx.fillStyle = sky
  ctx.fill(g.sky)
  // the city across the harbour, far and faint, and the town behind the quay
  skyline(ctx, 11, 0, g.coast - 20 * S, sea, 24 * S, 'rgba(127, 184, 255, 0.07)', S)
  skyline(ctx, 23, g.coast - 10 * S, w, groundAt(g, g.coast + 30 * S) + 2, 62 * S, 'rgba(201, 185, 138, 0.1)', S)
  // the water, darker with depth: a band, not a column
  const water = ctx.createLinearGradient(0, sea, 0, sea + g.wd)
  water.addColorStop(0, p.water)
  water.addColorStop(1, p.deep)
  ctx.fillStyle = water
  ctx.fill(g.water)
  // the floor under the band and the land ashore, fading out below
  const n = g.ground.length
  const solid = new Path2D()
  solid.moveTo(0, h)
  for (let i = 0; i < n; i++) solid.lineTo(i * STEP, g.ground[i])
  solid.lineTo(w, h)
  solid.closePath()
  const earth = ctx.createLinearGradient(g.coast - 70 * S, 0, g.coast + 10 * S, 0)
  earth.addColorStop(0, p.deep)
  earth.addColorStop(1, p.land)
  ctx.fillStyle = earth
  ctx.fill(solid)
  const fade = ctx.createLinearGradient(0, sea + g.wd * 0.5, 0, h)
  fade.addColorStop(0, 'rgba(0, 0, 0, 0)')
  fade.addColorStop(1, p.bg)
  ctx.fillStyle = fade
  ctx.fillRect(0, sea + g.wd * 0.5, w, h - sea - g.wd * 0.5)
  // the land's edge and the road along it
  const shore = Math.max(0, Math.floor((g.coast - 110 * S) / STEP))
  for (let c = 3; c >= 0; c--) {
    ctx.beginPath()
    for (let i = shore; i < n; i++) ctx.lineTo(i * STEP, g.ground[i] + c * 7)
    ctx.strokeStyle = c ? p.line : p.dom.ground
    ctx.globalAlpha = c ? 0.6 / c : 0.6
    ctx.lineWidth = c ? 1 : 1.2
    ctx.stroke()
  }
  ctx.globalAlpha = 0.25
  ctx.strokeStyle = p.dom.ground
  ctx.setLineDash([6 * S, 7 * S])
  ctx.beginPath()
  for (let i = Math.max(shore, Math.floor(g.coast / STEP)); i < n; i++) ctx.lineTo(i * STEP, g.ground[i] + 5)
  ctx.stroke()
  ctx.setLineDash([])
  ctx.globalAlpha = 1
  // the houses to deliver to
  for (const x of g.houses) {
    const y = groundAt(g, x)
    ctx.fillStyle = p.bg
    ctx.strokeStyle = p.dom.ground
    ctx.globalAlpha = 0.75
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.rect(x - 6 * S, y - 8 * S, 12 * S, 8 * S)
    ctx.moveTo(x - 8 * S, y - 8 * S)
    ctx.lineTo(x, y - 14 * S)
    ctx.lineTo(x + 8 * S, y - 8 * S)
    ctx.fill()
    ctx.stroke()
    ctx.fillStyle = p.dom.ground
    ctx.globalAlpha = 0.5
    ctx.fillRect(x - 1.5 * S, y - 5 * S, 3 * S, 5 * S)
  }
  // the float the harbour's drones deliver to: a moored locker
  {
    const x = g.floatX
    ctx.globalAlpha = 0.8
    ctx.fillStyle = p.bg
    ctx.strokeStyle = p.dom.surface
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.rect(x - 8 * S, sea - 3 * S, 16 * S, 3 * S)
    ctx.fill()
    ctx.stroke()
    ctx.beginPath()
    ctx.rect(x - 4 * S, sea - 11 * S, 8 * S, 8 * S)
    ctx.fill()
    ctx.stroke()
    ctx.beginPath()
    ctx.moveTo(x - 4 * S, sea - 7 * S)
    ctx.lineTo(x + 4 * S, sea - 7 * S)
    ctx.stroke()
  }
  // the waterline
  ctx.globalAlpha = 0.7
  ctx.strokeStyle = p.dom.surface
  ctx.lineWidth = 1.2
  ctx.beginPath()
  ctx.moveTo(0, sea + 0.5)
  ctx.lineTo(g.coast - 10 * S, sea + 0.5)
  ctx.stroke()
  ctx.globalAlpha = 1
  // the bands' names: up the left edge (the water), the ground's up the right
  const lx = Math.min(31, Math.max(14, open.left - 14))
  caption(ctx, 'AIR', lx, 60, sea - 42, p.dom.air, p.font, 1)
  caption(ctx, 'SURFACE', lx, sea - 36, sea + 36, p.dom.surface, p.font, 1)
  const rx = Math.max(w - 31, Math.min(w - 14, open.right + 14))
  const gr = groundAt(g, rx)
  caption(ctx, 'GROUND', rx, gr + 16, Math.min(h - 16, gr + 96), p.dom.ground, p.font, -1)
}

/** wind in the sky and the current on the water, drifting: the conditions the fleet works in */
function drawAmbient(ctx: CanvasRenderingContext2D, g: Geo, p: Palette, t: number) {
  const S = g.S
  ctx.lineWidth = 1
  ctx.strokeStyle = p.dom.air
  ctx.lineCap = 'round'
  for (let i = 0; i < 9; i++) {
    const len = (34 + 50 * hash(i * 1.7)) * S
    const sp = (18 + 22 * hash(i * 3.3)) * S
    const x = ((hash(i * 5.1) * (g.w + 200) + t * sp) % (g.w + 200)) - 100
    const y = g.top + 20 * S + hash(i * 7.7) * (g.sea - g.top - 100 * S)
    ctx.globalAlpha = 0.05 + 0.07 * hash(i * 2.9)
    ctx.beginPath()
    ctx.moveTo(x, y)
    ctx.quadraticCurveTo(x + len * 0.5, y - 2 * S, x + len, y)
    ctx.stroke()
  }
  ctx.strokeStyle = p.dom.surface
  for (let i = 0; i < 14; i++) {
    const x = (hash(i * 5.3) * g.w + t * 12 * S) % g.w
    if (!inWater(g, x)) continue
    const y = g.sea + 6 * S + hash(i * 2.2) * (g.wd - 16 * S)
    const len = (6 + 9 * hash(i * 9.1)) * S
    ctx.globalAlpha = 0.14 * (1 - (y - g.sea) / g.wd)
    ctx.beginPath()
    ctx.moveTo(x, y)
    ctx.lineTo(x + len, y)
    ctx.stroke()
  }
  ctx.lineCap = 'butt'
  ctx.globalAlpha = 1
}

/** the links' lines, a SATCOM link's beam, a dropped link's red ghost */
function drawLinks(ctx: CanvasRenderingContext2D, net: Net, g: Geo, p: Palette, t: number) {
  ctx.lineCap = 'butt'
  for (const l of net.links) {
    const st = LINKS[l.kind]
    const f = Math.min(l.a.fade, l.b.fade)
    const [ax, ay] = end(l.a), [bx, by] = end(l.b)
    const fresh = t - (net.since.get(l.key) ?? -9)
    const burst = net.burst?.key === l.key && t - net.burst.t0 < BURST_S
    let a = st.alpha * (0.3 + 0.7 * l.q) + (fresh < FLASH_S ? 0.45 * (1 - fresh / FLASH_S) : 0)
    if (l.kind === 'satcom') {
      // the satellite's beam, narrowing up to it
      const dx = ax - bx, dy = ay - by, d = Math.hypot(dx, dy) || 1
      const nx = -dy / d, ny = dx / d, half = l.a.r * 1.2
      ctx.globalAlpha = 0.06 * f
      ctx.fillStyle = p.link.satcom
      ctx.beginPath()
      ctx.moveTo(bx, by)
      ctx.lineTo(ax + nx * half, ay + ny * half)
      ctx.lineTo(ax - nx * half, ay - ny * half)
      ctx.closePath()
      ctx.fill()
    }
    const s = burst ? LINKS.lost : st
    if (burst) a = s.alpha * (0.45 + 0.55 * Math.abs(Math.sin(t * 9)))
    ctx.globalAlpha = Math.min(1, a) * f
    ctx.strokeStyle = burst ? p.link.lost : p.link[l.kind]
    ctx.lineWidth = s.width
    ctx.setLineDash(s.dash)
    ctx.beginPath()
    ctx.moveTo(ax, ay)
    ctx.lineTo(bx, by)
    ctx.stroke()
  }
  ctx.setLineDash(LINKS.lost.dash)
  ctx.strokeStyle = p.link.lost
  ctx.lineWidth = LINKS.lost.width
  for (const gh of net.ghosts) {
    const f = 1 - (t - gh.t0) / GHOST_S
    if (f <= 0) continue
    const [ax, ay] = end(gh.a), [bx, by] = end(gh.b)
    ctx.globalAlpha = LINKS.lost.alpha * f * Math.min(gh.a.fade, gh.b.fade)
    ctx.beginPath()
    ctx.moveTo(ax, ay)
    ctx.lineTo(bx, by)
    ctx.stroke()
    // where the terrain cut it
    const x = ax + (bx - ax) * gh.u, y = ay + (by - ay) * gh.u, k = 3.5 * g.S
    ctx.setLineDash([])
    ctx.lineWidth = 1.4
    ctx.beginPath()
    ctx.moveTo(x - k, y - k)
    ctx.lineTo(x + k, y + k)
    ctx.moveTo(x + k, y - k)
    ctx.lineTo(x - k, y + k)
    ctx.stroke()
    ctx.setLineDash(LINKS.lost.dash)
    ctx.lineWidth = LINKS.lost.width
  }
  ctx.setLineDash([])
  ctx.globalAlpha = 1
}

/** the traffic on each link: packets each way, slowly over SATCOM, and dying halfway on a link
 *  that is degraded */
function drawTraffic(ctx: CanvasRenderingContext2D, net: Net, g: Geo, p: Palette, t: number) {
  const S = g.S
  for (const l of net.links) {
    const [ax, ay] = end(l.a), [bx, by] = end(l.b)
    const len = Math.hypot(bx - ax, by - ay)
    if (len < 1) continue
    const burst = net.burst?.key === l.key && t - net.burst.t0 < BURST_S
    const col = burst ? p.link.lost : p.link[l.kind]
    const f = Math.min(l.a.fade, l.b.fade)
    const seed = hash(l.a.id * 31 + l.b.id * 7)
    /** packets at fractions `us` along it, from a (or from b) */
    const dots = (us: number[], fromB: boolean) => {
      for (const u0 of us) {
        if (u0 < 0 || u0 > 1) continue
        // a burst of loss: packets die halfway
        if (burst && u0 > 0.5) continue
        const u = fromB ? 1 - u0 : u0
        ctx.globalAlpha = (burst ? 0.9 * (1 - u0 * 1.6) : 0.9) * (0.4 + 0.6 * l.q) * f
        ctx.fillStyle = col
        ctx.beginPath()
        ctx.arc(ax + (bx - ax) * u, ay + (by - ay) * u, 1.5 * S, 0, Math.PI * 2)
        ctx.fill()
      }
    }
    /** a packet each period, travelling at `speed` px/s (at scale 1) */
    const run = (speed: number, rest: number, phase: number) => {
      const travel = len / (speed * S)
      const tau = (t + phase * (travel + rest)) % (travel + rest)
      return tau < travel ? tau / travel : -1
    }
    if (l.kind === 'cellular') {
      dots([run(190, 1.1, seed)], false)
      dots([run(190, 1.1, seed + 0.5)], true)
    } else if (l.kind === 'radio') {
      // commands up to the vehicle, its telemetry and video down
      dots([run(240, 1.2, seed)], false)
      const v = run(240, 0.5, seed + 0.3)
      dots([v, v - 0.06, v - 0.12], true)
    } else if (l.kind === 'mesh') {
      dots([run(150, 1.4, seed)], false)
      dots([run(150, 1.4, seed + 0.5)], true)
    } else if (l.kind === 'satcom') {
      dots([run(110, 0.9, seed)], false)
      dots([run(110, 0.9, seed + 0.5)], true)
    }
  }
  ctx.globalAlpha = 1
}

/** a hub's drawing, its base at the origin and up negative: a depot with its antenna, a
 *  vertiport tower with its roof pad, a floating dock with its shed and crane */
function drawHub(ctx: CanvasRenderingContext2D, v: Hub, p: Palette, t: number, id: number) {
  const body = 'rgba(195, 204, 214, 0.5)'
  const soft = 'rgba(195, 204, 214, 0.22)'
  const edge = p.hub
  ctx.lineWidth = 1
  ctx.strokeStyle = edge
  ctx.fillStyle = body
  const rect = (x: number, y: number, w: number, h: number, fill = body) => {
    ctx.fillStyle = fill
    ctx.beginPath()
    ctx.rect(x, y, w, h)
    ctx.fill()
    ctx.stroke()
  }
  const cutRect = (x: number, y: number, w: number, h: number) => {
    ctx.fillStyle = p.bg
    ctx.fillRect(x, y, w, h)
  }
  const mast = (x: number, y0: number, y1: number) => {
    ctx.beginPath()
    ctx.moveTo(x, y0)
    ctx.lineTo(x, y1)
    ctx.stroke()
    // a cell panel on the mast
    ctx.fillStyle = edge
    ctx.fillRect(x - 2, y1 - 1, 4, 6)
    if (Math.sin(t * 2.1 + id) > 0.55) {
      ctx.fillStyle = p.lamp
      ctx.beginPath()
      ctx.arc(x, y1 - 3, 1.6, 0, Math.PI * 2)
      ctx.fill()
    }
  }
  if (v === 'depot') {
    rect(-17, -13, 34, 13)
    ctx.fillStyle = soft
    ctx.beginPath()
    ctx.moveTo(-19, -13)
    ctx.lineTo(-12, -19)
    ctx.lineTo(12, -19)
    ctx.lineTo(19, -13)
    ctx.closePath()
    ctx.fill()
    ctx.stroke()
    cutRect(-13, -9, 8, 9)
    cutRect(-3, -9, 8, 9)
    cutRect(9, -9, 5, 4)
    mast(11, -19, -34)
  } else if (v === 'tower') {
    rect(-8, -42, 16, 42)
    ctx.fillStyle = p.bg
    for (let y = -37; y < -4; y += 8) for (const x of [-4.5, 1.5]) ctx.fillRect(x, y, 3, 3)
    rect(-15, -46, 30, 4, soft)
    // the landing ring on the pad
    ctx.beginPath()
    ctx.ellipse(0, -47, 9, 2.2, 0, 0, Math.PI * 2)
    ctx.stroke()
    mast(10, -46, -54)
  } else {
    // pilings under the deck, the deck, a shed with the pad on its roof, a crane, the antenna
    ctx.beginPath()
    for (const x of [-20, -8, 4, 16]) {
      ctx.moveTo(x, -1)
      ctx.lineTo(x, 7)
    }
    ctx.stroke()
    rect(-24, -4, 48, 4)
    rect(-16, -16, 20, 12)
    cutRect(-12, -12, 6, 8)
    rect(-18, -18, 24, 2, soft)
    ctx.beginPath()
    ctx.moveTo(14, -4)
    ctx.lineTo(14, -22)
    ctx.lineTo(24, -20)
    ctx.moveTo(22, -20)
    ctx.lineTo(22, -11)
    ctx.stroke()
    mast(-17, -18, -24)
  }
}

/** a satellite relay, a small body between two panels */
function drawSat(ctx: CanvasRenderingContext2D, n: Node, color: string) {
  const k = n.k
  ctx.save()
  ctx.translate(n.cx, n.cy)
  ctx.scale(k, k)
  ctx.globalAlpha = 0.9 * n.fade
  ctx.fillStyle = color
  ctx.strokeStyle = color
  ctx.lineWidth = 1
  ctx.fillRect(-3.5, -2.5, 7, 5)
  ctx.globalAlpha = 0.55 * n.fade
  ctx.fillRect(-13, -1.6, 8, 3.2)
  ctx.fillRect(5, -1.6, 8, 3.2)
  ctx.beginPath()
  ctx.moveTo(0, -2.5)
  ctx.lineTo(0, -6)
  ctx.stroke()
  ctx.restore()
}

/** the icons: the console's own pictograms (symbols.ts), each rasterised once a size */
type Icons = Map<string, HTMLImageElement>
function loadIcons(S: number, dpr: number, onLoad: () => void): Icons {
  const icons: Icons = new Map()
  for (const v of ['copter', 'plane', 'car', 'boat'] as Vehicle[]) {
    for (const stale of [false, true]) {
      const px = Math.round(RADIUS[v] * S * 2 * (40 / 36) * dpr)
      const img = new Image()
      img.onload = onLoad
      img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(symbolSvg({ kind: KIND[v] }, { stale, size: px }).svg)}`
      icons.set(`${v}|${stale}`, img)
    }
  }
  return icons
}

/** wakes behind the boats moving */
function drawWakes(ctx: CanvasRenderingContext2D, nodes: Node[], g: Geo, p: Palette) {
  const S = g.S
  ctx.strokeStyle = p.dom.surface
  ctx.lineWidth = 1
  for (const n of nodes) {
    if (n.v !== 'boat' || offscreen(n) || n.pause > 0) continue
    const stern = n.x - n.dir * n.r * 0.85
    const bow = n.x + n.dir * n.r * 0.85
    const len = (10 + n.sp * 0.8) * S
    ctx.globalAlpha = 0.45 * n.fade
    ctx.beginPath()
    ctx.moveTo(stern, g.sea + 0.8)
    ctx.lineTo(stern - n.dir * len, g.sea + 0.8)
    ctx.moveTo(bow, g.sea)
    ctx.lineTo(bow + n.dir * 3 * S, g.sea - 2.5 * S)
    ctx.stroke()
    ctx.globalAlpha = 0.22 * n.fade
    ctx.beginPath()
    ctx.moveTo(stern - n.dir * 3 * S, g.sea + 2.8)
    ctx.lineTo(stern - n.dir * len * 0.6, g.sea + 2.8)
    ctx.stroke()
  }
  ctx.globalAlpha = 1
}

function drawNodes(ctx: CanvasRenderingContext2D, nodes: Node[], g: Geo, p: Palette, t: number, open: Open, icons: Icons, net: Net) {
  const S = g.S
  for (const n of nodes) {
    if (offscreen(n)) continue
    if (isHub(n.v)) {
      ctx.save()
      ctx.translate(n.x, n.y)
      ctx.scale(n.k, n.k)
      ctx.globalAlpha = 0.9 * n.fade
      drawHub(ctx, n.v, p, t, n.id)
      ctx.restore()
      continue
    }
    if (n.v === 'sat') {
      drawSat(ctx, n, p.link.satcom)
      continue
    }
    const c = p.dom[n.d as Dom]
    // its link degraded: the icon goes grey and dashed, as the console's does for a stale link
    const stale = net.burst?.vid === n.id && t - net.burst.t0 < BURST_S
    const img = icons.get(`${n.v}|${stale}`)
    if (img?.complete && img.naturalWidth > 0) {
      const d = n.r * 2 * (40 / 36)
      ctx.save()
      ctx.translate(n.cx, n.cy)
      ctx.rotate(n.ang)
      ctx.globalAlpha = n.fade
      ctx.drawImage(img, -d / 2, -d / 2, d, d)
      ctx.restore()
    }
    if (n.carry && n.v !== 'plane') {
      // the parcel it carries: a small box on a short line, below a drone, above the rest
      const air = n.d === 'air'
      const y = air ? n.cy + n.r + 4 * S : n.cy - n.r - 5 * S
      ctx.globalAlpha = 0.95 * n.fade
      ctx.strokeStyle = p.parcel
      ctx.lineWidth = 1
      if (air) {
        ctx.beginPath()
        ctx.moveTo(n.cx, n.cy + n.r - 1)
        ctx.lineTo(n.cx, y - 2.5 * S)
        ctx.stroke()
      }
      ctx.fillStyle = p.parcel
      ctx.fillRect(n.cx - 2.8 * S, y - 2.5 * S, 5.6 * S, 5 * S)
      ctx.fillStyle = p.bg
      ctx.fillRect(n.cx - 0.5 * S, y - 2.5 * S, S, 5 * S)
    }
    if (n.lit > 0.02) {
      ctx.globalAlpha = n.lit * 0.8 * n.fade
      ctx.strokeStyle = c
      ctx.lineWidth = 1
      ctx.beginPath()
      ctx.arc(n.cx, n.cy, n.r + 2 + (1 - n.lit) * 7, 0, Math.PI * 2)
      ctx.stroke()
    }
  }
  // a tag on one of each type: the catalog's short name
  ctx.font = `600 ${Math.round(9 * Math.min(1.2, g.S))}px ${p.font}`
  spaced(ctx, '0.08em')
  ctx.textBaseline = 'alphabetic'
  const tt = open.title
  for (const n of nodes) {
    if (!n.tag || offscreen(n)) continue
    const side = n.v === 'sat'
    const x = side ? n.cx + 20 * S : n.x
    const y = side ? n.cy + 3 : isHub(n.v) ? n.y - HUB[n.v].top * S - 6 : n.cy - n.r - (n.carry && n.d !== 'air' ? 14 * S : 5)
    if (tt && x > tt[0] && x < tt[2] && y > tt[1] && y < tt[3]) continue
    ctx.globalAlpha = 0.72 * n.fade
    ctx.fillStyle = isHub(n.v) ? p.hub : n.v === 'sat' ? p.link.satcom : p.dom[n.d as Dom]
    // kept on the screen
    const half = ctx.measureText(TAG[n.v]).width / 2
    ctx.textAlign = side ? 'left' : 'center'
    ctx.fillText(TAG[n.v], side ? Math.min(x, g.w - 8 - half * 2) : clamp(x, 8 + half, g.w - 8 - half), y)
  }
  ctx.globalAlpha = 1
}

/** whether a point is where the panels leave the scene open */
const visible = (o: Open, w: number, x: number, y: number) =>
  x > 0 && x < w && (y < o.top || x < o.left || x > o.right || (o.gap != null && x > o.gap[0] && x < o.gap[1]))

/** `onScene` hears each scene's name as it comes in (the menu shows it, faintly, in its top rail) */
export function FleetBackdrop({ onScene }: { onScene?: (scene: string) => void }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const tell = useRef(onScene)
  useEffect(() => {
    tell.current = onScene
  }, [onScene])
  useEffect(() => {
    const canvas = ref.current
    const ctx = canvas?.getContext('2d')
    if (!canvas || !ctx) return
    const still = window.matchMedia('(prefers-reduced-motion: reduce)')
    const layer = document.createElement('canvas')
    const lctx = layer.getContext('2d')
    if (!lctx) return
    const p = palette(canvas)
    let g: Geo | null = null
    let open: Open = { left: 0, right: 0, gap: null, top: 0, title: null }
    let nodes: Node[] = []
    let icons: Icons = new Map()
    const net: Net = { links: [], since: new Map(), ghosts: [], cutT: -99, burst: null }
    let burstN = 0
    let raf = 0
    let last = 0
    let clock = 0
    let nextBurst = 7
    let dpr = 1
    // the opening scene, then another at random every so often
    let scene = 0
    let changed = false
    let nextScene = SCENE_S
    const ids = { n: 0 }

    /** to another scene: who is in both stays, who is not fades out, who is new fades in */
    const change = () => {
      if (!g) return
      let k = Math.floor(Math.random() * (SCENES.length - 1))
      if (k >= scene) k++
      scene = changed ? k : FIRST_CHANGE
      changed = true
      tell.current?.(SCENES[scene].name)
      const stay = nodes.filter((n) => n.goal === 1)
      const kept = new Map<Node, Node>()
      const fresh = seed(g, scene, ids)
      for (const n of fresh) {
        const o = stay.find((x) => x.v === n.v && ![...kept.values()].includes(x))
        if (o) kept.set(n, o)
      }
      const held = new Set(kept.values())
      for (const o of stay) if (!held.has(o)) o.goal = 0
      for (const [n, o] of kept) {
        // it carries on with the new scene's route and pace
        if (!isHub(o.v) && o.d !== 'orbit') Object.assign(o, { pat: n.pat, lo: n.lo, hi: n.hi, base: n.base, padY: n.padY, farY: n.farY, alt: n.alt, sp: n.sp, hold: n.hold })
      }
      for (const n of fresh) {
        if (kept.has(n)) continue
        n.fade = 0
        n.tag = !nodes.some((x) => TAG[x.v] === TAG[n.v] && x.tag)
        nodes.push(n)
      }
    }

    /** this frame's links, against the last: what came up, what the terrain cut */
    const relink = (quiet: boolean) => {
      if (!g) return
      const { links, blocked } = network(nodes, g, net.burst?.vid ?? -1)
      const now = new Set(links.map((l) => l.key))
      for (const l of links) {
        if (net.since.has(l.key)) continue
        net.since.set(l.key, quiet ? -9 : clock)
        // a vehicle back in contact, or on to another link
        if (!quiet) l.a.lit = Math.max(l.a.lit, 0.6)
      }
      const prev = new Map(net.links.map((l) => [l.key, l]))
      for (const [k, l] of prev) {
        if (now.has(k)) continue
        net.since.delete(k)
        // a link the terrain cut: shown red, one at a time
        const u = blocked.get(k)
        if (!quiet && u != null && clock - net.cutT > CUT_EVERY_S) {
          net.ghosts.push({ a: l.a, b: l.b, u, t0: clock })
          net.cutT = clock
        }
      }
      net.ghosts = net.ghosts.filter((gh) => clock - gh.t0 < GHOST_S)
      net.links = links
    }
    const frame = () => {
      if (!g) return
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, g.w, g.h)
      ctx.drawImage(layer, 0, 0, g.w, g.h)
      drawAmbient(ctx, g, p, clock)
      drawLinks(ctx, net, g, p, clock)
      drawTraffic(ctx, net, g, p, clock)
      drawWakes(ctx, nodes, g, p)
      drawNodes(ctx, nodes, g, p, clock, open, icons, net)
    }
    /** now and then one vehicle's link degrades: a link that can be seen */
    const startBurst = () => {
      if (!g) return
      const geo = g
      const seen = net.links.filter((l) => {
        if (l.backup) return false
        const [ax, ay] = end(l.a), [bx, by] = end(l.b)
        let k = 0
        for (let i = 1; i < 10; i++) if (visible(open, geo.w, ax + ((bx - ax) * i) / 10, ay + ((by - ay) * i) / 10)) k++
        return k >= 2
      })
      const pick = seen[burstN++ % Math.max(1, seen.length)]
      if (pick) net.burst = { key: pick.key, vid: pick.a.id, t0: clock }
    }
    const tick = (now: number) => {
      raf = requestAnimationFrame(tick)
      if (!g || now - last < FRAME_MS) return
      const dt = Math.min(0.1, (now - last) / 1000)
      last = now
      clock += dt
      if (clock > nextScene) {
        change()
        nextScene = clock + SCENE_S
      }
      for (const n of nodes) n.fade = clamp(n.fade + ((n.goal ? 1 : -1) * dt) / FADE_S, 0, 1)
      nodes = nodes.filter((n) => n.goal || n.fade > 0)
      step(nodes, g, dt, clock)
      relink(false)
      if (net.burst && clock - net.burst.t0 > BURST_S) {
        // recovered: it comes back as a fresh link
        net.since.set(net.burst.key, clock)
        net.burst = null
      }
      if (clock > nextBurst) {
        startBurst()
        nextBurst = clock + 10 + 5 * hash(clock)
      }
      frame()
    }
    const play = () => {
      cancelAnimationFrame(raf)
      raf = 0
      if (still.matches || document.hidden) {
        frame()
        return
      }
      last = performance.now()
      raf = requestAnimationFrame(tick)
    }
    const resize = () => {
      const w = canvas.clientWidth, h = canvas.clientHeight
      if (!w || !h) return
      dpr = Math.min(1.5, window.devicePixelRatio || 1)
      canvas.width = layer.width = Math.round(w * dpr)
      canvas.height = layer.height = Math.round(h * dpr)
      open = measure(canvas, w)
      g = geometry(w, h, open)
      icons = loadIcons(g.S, dpr, () => {
        // a still frame draws again once its icons are in
        if (still.matches || document.hidden) frame()
      })
      nodes = seed(g, scene, ids)
      net.links = []
      net.since.clear()
      net.ghosts = []
      net.burst = null
      // a still frame shows them already under way
      for (let k = 0; k < 150; k++) {
        clock += 1 / 30
        step(nodes, g, 1 / 30, clock)
        relink(true)
      }
      nextBurst = clock + 6
      nextScene = clock + SCENE_S
      lctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      drawStatic(lctx, g, p, open)
      frame()
    }
    const ro = new ResizeObserver(resize)
    ro.observe(canvas)
    resize()
    tell.current?.(SCENES[scene].name)
    play()
    document.addEventListener('visibilitychange', play)
    still.addEventListener('change', play)
    return () => {
      cancelAnimationFrame(raf)
      ro.disconnect()
      document.removeEventListener('visibilitychange', play)
      still.removeEventListener('change', play)
    }
  }, [])
  return <canvas ref={ref} className="fm-backdrop" aria-hidden="true" />
}

/** The backdrop's link styles, for the menu's top rail: quiet, and only where there is room. */
export function FleetLegend() {
  return (
    <span className="fm-legend" aria-hidden="true">
      {(Object.keys(LINKS) as LinkKind[]).map((k) => (
        <span key={k}>
          <svg viewBox="0 0 18 6" width="18" height="6">
            <line x1="0" y1="3" x2="18" y2="3" style={{ stroke: `var(${LINKS[k].color}, ${LINKS[k].fallback})` }} strokeWidth={Math.max(1.2, LINKS[k].width)} strokeDasharray={LINKS[k].dash.join(' ') || undefined} />
          </svg>
          {LINKS[k].label}
        </span>
      ))}
    </span>
  )
}
