// The traffic the fleet does not control, as the company's tracker sees it (docs/design/traffic.md,
// section 5: never ground truth), and what the console makes of it: which tracks matter to the fleet
// (declutter), how sure each is (fade, provisional), where a bearing-only track can be (a wedge,
// never a point), where a track is heading (a constant-velocity prediction with its growing
// uncertainty). Pure: traffic.check.ts. Display only: nothing here decides anything.
import type { LngLat } from './types'

// ---- the wire (GET /api/traffic, GET /api/traffic/sensors, the WebSocket `traffic` message) ----

export type TrafficDomain = 'air' | 'surface'
export type TrafficClass =
  | 'airliner'
  | 'bizjet'
  | 'ga'
  | 'helicopter'
  | 'seaplane'
  | 'drone'
  | 'unknown_air'
  | 'cargo'
  | 'tanker'
  | 'passenger'
  | 'ferry'
  | 'tug'
  | 'pilot'
  | 'fishing'
  | 'sailing'
  | 'pleasure'
  | 'small_craft'
  | 'unknown_surface'
export type TrackState = 'tentative' | 'confirmed' | 'coasting'
export type SourceKind = 'adsb_1090' | 'adsb_uat' | 'ais_a' | 'ais_b' | 'remote_id' | 'daa' | 'marine_radar' | 'ground_radar'
export type TrackFlag =
  | 'stale'
  | 'anonymous'
  | 'ladd'
  | 'id_changed'
  | 'sensor_disagree'
  | 'position_jump'
  | 'invalid_mmsi'
  | 'on_ground'
  | 'moored'
  | 'at_anchor'
  | 'emergency'
  | 'bearing_only'
  | 'triangulated'

export interface TrackIds {
  icao24?: string
  mmsi?: number
  uas_id?: string
  callsign?: string
  /** a name, when the bridge puts the vessel's AIS name here too (static.name otherwise) */
  name?: string
  anonymous: boolean
}

/** What AIS types 5, 19 and 24 gave, an aircraft's last squawk, Remote ID's own fields. */
export interface TrackStatic {
  name?: string
  callsign?: string
  imo?: number
  ship_type?: number
  ship_type_text?: string
  length_m?: number
  beam_m?: number
  draught_m?: number
  destination?: string
  eta?: string
  nav_status?: number
  nav_status_text?: string
  mothership_mmsi?: number
  squawk?: string
  id_type?: string
  ua_type?: string
  op_lat?: number
  op_lon?: number
}

/** A bearing-only track's geometry: the line of sight from one sensor, and the range interval
 *  along it the target can be in (metres). */
export interface BearingLine {
  sensor_lat: number
  sensor_lon: number
  sensor_alt_m: number
  bearing_deg: number
  sigma_bearing_deg: number
  elev_deg: number | null
  range_m: [number, number]
  t: number
}

export interface TrackSource {
  kind: SourceKind
  /** the receiver (a hub's id, a network feed's) or the own vehicle that carries the sensor (its uid) */
  rx: string
  last_t: number
}

export interface Track {
  id: string
  /** the picture's time, Unix ms */
  t: number
  domain: TrafficDomain
  class: TrafficClass
  cooperative: boolean
  ids: TrackIds
  lat: number
  lon: number
  /** metres; null for surface traffic and an unknown altitude (alt_ref `none`) */
  alt_m: number | null
  alt_ref: 'baro' | 'geo' | 'none'
  /** 1 sigma, metres (the semi-major axis) */
  h_sigma_m: number
  v_sigma_m: number | null
  /** over ground, [east, north] m/s; null for a bearing-only track */
  vel_mps: [number, number] | null
  vs_mps: number | null
  heading_deg: number | null
  state: TrackState
  sources: TrackSource[]
  flags: string[]
  static: TrackStatic
  bearing_only?: BearingLine | null
}

export interface FeedStatus {
  connected: boolean
  last_t: number | null
  lines_per_s: number
}

export interface Picture {
  state: 'ok' | 'degraded'
  reasons: string[]
  feeds: Record<string, FeedStatus>
}

export interface TrafficFrame {
  t: number
  tracks: Track[]
  picture: Picture
}

/** A receiver or sensor (GET /api/traffic/sensors, the simulator's /sensors). A hub's receivers
 *  share the hub's id: unique by id and kind. */
export interface TrafficSensor {
  id: string
  kind: string
  num?: number | null
  owner: 'company' | 'network'
  platform?: string | null
  lat?: number | null
  lon?: number | null
  alt_m?: number | null
  nominal_range_m: number
  feeders?: number | null
  enabled?: boolean
  up?: boolean
}

/** One object of the simulator's truth (GET /api/traffic/truth, dev runs only). */
export interface TruthObject {
  id: string
  stream?: string
  domain: TrafficDomain
  class: string
  lat: number
  lon: number
  alt_m: number
  track_deg?: number
  speed_mps?: number
  on_ground?: boolean
  ids?: { icao24?: string | null; callsign?: string | null; mmsi?: number | null; uas_id?: string | null; name?: string | null }
  faults?: string[]
}

export const AIR_CLASSES: readonly TrafficClass[] = ['airliner', 'bizjet', 'ga', 'helicopter', 'seaplane', 'drone', 'unknown_air']
export const SURFACE_CLASSES: readonly TrafficClass[] = ['cargo', 'tanker', 'passenger', 'ferry', 'tug', 'pilot', 'fishing', 'sailing', 'pleasure', 'small_craft', 'unknown_surface']

export function hasFlag(t: Pick<Track, 'flags'>, f: TrackFlag): boolean {
  return t.flags.includes(f)
}

/** A track whose own reading says something is wrong with it: a broadcast the company's own
 *  sensors contradict, a jump it could not have made, an emergency squawk. Always shown,
 *  emphasised, whatever its distance. */
export const ALARM_FLAGS: readonly TrackFlag[] = ['sensor_disagree', 'position_jump', 'emergency']
export function isAlarming(t: Pick<Track, 'flags'>): boolean {
  return ALARM_FLAGS.some((f) => t.flags.includes(f))
}

export function isBearingOnly(t: Track): t is Track & { bearing_only: BearingLine } {
  return t.bearing_only != null && (hasFlag(t, 'bearing_only') || t.vel_mps == null)
}

// ---- local metres ------------------------------------------------------------------------------

const M_PER_DEG = 111_320

/** A point `rangeM` from (lon, lat) on `bearingDeg` (true), on a flat earth: within a metre over the
 *  few kilometres a sensor sees (the same approximation as geo.ts circle). */
export function destination([lon, lat]: LngLat, bearingDeg: number, rangeM: number): LngLat {
  const b = (bearingDeg * Math.PI) / 180
  const dLat = (rangeM * Math.cos(b)) / M_PER_DEG
  const dLon = (rangeM * Math.sin(b)) / (M_PER_DEG * Math.cos((lat * Math.PI) / 180))
  return [lon + dLon, lat + dLat]
}

/** Metres east and north of `origin`. */
export function localXY(origin: LngLat, p: LngLat): [number, number] {
  const k = Math.cos((origin[1] * Math.PI) / 180)
  return [(p[0] - origin[0]) * M_PER_DEG * k, (p[1] - origin[1]) * M_PER_DEG]
}

/** Horizontal metres from `p` to the polyline `path` (to its nearest segment). */
export function distanceToPath(p: LngLat, path: readonly LngLat[]): number {
  if (!path.length) return Infinity
  if (path.length === 1) return Math.hypot(...localXY(p, path[0]))
  let best = Infinity
  for (let k = 1; k < path.length; k++) {
    const [ax, ay] = localXY(p, path[k - 1])
    const [bx, by] = localXY(p, path[k])
    const dx = bx - ax
    const dy = by - ay
    const len2 = dx * dx + dy * dy
    const s = len2 > 0 ? Math.max(0, Math.min(1, -(ax * dx + ay * dy) / len2)) : 0
    best = Math.min(best, Math.hypot(ax + s * dx, ay + s * dy))
  }
  return best
}

/** Ground speed, m/s, or null. */
export function speedOf(t: Pick<Track, 'vel_mps'>): number | null {
  return t.vel_mps ? Math.hypot(t.vel_mps[0], t.vel_mps[1]) : null
}

/** The direction it moves (true degrees): its heading as reported, else its course over ground,
 *  else null (a bearing-only track, a still one). */
export function courseOf(t: Pick<Track, 'heading_deg' | 'vel_mps'>): number | null {
  if (t.heading_deg != null) return t.heading_deg
  if (!t.vel_mps || Math.hypot(t.vel_mps[0], t.vel_mps[1]) < 0.5) return null
  const deg = (Math.atan2(t.vel_mps[0], t.vel_mps[1]) * 180) / Math.PI
  return (deg + 360) % 360
}

// ---- how it is drawn in height -----------------------------------------------------------------

/** Aircraft above this (metres) are drawn at their position on the ground, not at their height:
 *  drawn at twice their height (map3d.ts) an airliner at altitude is kilometres above the camera,
 *  so far off any view that its foot is all that reads; and it is never the fleet's concern. */
export const FLATTEN_ALT_M = 3000

/** The height a track (or a truth object) is drawn at, metres above mean sea level before the map's
 *  exaggeration: an aircraft at its altitude up to FLATTEN_ALT_M, everything else at the surface. */
export function drawAltM(domain: TrafficDomain, altM: number | null | undefined): number {
  if (domain !== 'air' || altM == null || !Number.isFinite(altM)) return 0
  return altM > FLATTEN_ALT_M ? 0 : Math.max(0, altM)
}

// ---- which tracks matter (declutter) ------------------------------------------------------------

/** What the fleet is: our vehicles where they are, and the routes they follow. */
export interface FleetContext {
  ours: { uid: number; at: LngLat; altM: number | null; domain: 'air' | 'ground' | 'surface' }[]
  routes: { uid: number; path: LngLat[]; domain: 'air' | 'ground' | 'surface' }[]
}

export type Tier = 'near' | 'mid' | 'far'

export interface Relevance {
  tier: Tier
  /** horizontal metres to the nearest of ours or our routes that the track can matter to */
  distM: number
  /** the nearest of ours (its uid), when one is nearer than any route */
  nearest: number | null
  /** the track's altitude above that vehicle's, metres (null: unknown) */
  dAltM: number | null
  /** why it is in its tier, in plain words (for the hover and the check) */
  why: string
}

/** Thresholds (metres). Air: within NEAR horizontally and AIR_NEAR_ALT vertically of one of ours
 *  (or of a route, taken at our cruise height); manned aircraft have the right of way and the
 *  well-clear volume is 2,000 ft by 250 ft (traffic.md section 6): this is a margin around that for
 *  the eye, not the rule. Surface: within SURFACE_NEAR of one of our boats or their routes. */
export const AIR_NEAR_M = 3000
export const AIR_MID_M = 8000
export const AIR_NEAR_ALT_M = 600
export const AIR_MID_ALT_M = 1500
export const SURFACE_NEAR_M = 1500
export const SURFACE_MID_M = 5000
/** a big vessel under one of our aircraft or its route: mid (a mast or a stack can reach 60 m) */
export const SHIP_UNDER_AIR_M = 800
const BIG_VESSELS: ReadonlySet<TrafficClass> = new Set(['cargo', 'tanker', 'passenger', 'ferry'])
/** the height our aircraft are taken at on a route, metres above mean sea level */
export const ROUTE_ALT_M = 100

/** How much a track matters to the fleet now. A track with an alarming flag, or one an alert names,
 *  is near whatever its distance (`force`). An unknown altitude counts as close vertically. */
export function relevance(t: Track, ctx: FleetContext, force = false): Relevance {
  if (force) {
    const base = relevance(t, ctx, false)
    if (base.tier === 'near') return base
    return { ...base, tier: 'near', why: `${isAlarming(t) ? 'flagged' : 'selected or named by an alert'}; otherwise ${base.why}` }
  }
  const at: LngLat = [t.lon, t.lat]
  let distM = Infinity
  let nearest: number | null = null
  let dAltM: number | null = null
  let underAir = Infinity
  if (t.domain === 'air') {
    // the nearest of our aircraft, vertically weighed: a helicopter 1,000 m above a drone is not near it
    for (const o of ctx.ours) {
      if (o.domain !== 'air') continue
      const d = Math.hypot(...localXY(at, o.at))
      if (d < distM) {
        distM = d
        nearest = o.uid
        dAltM = t.alt_m != null && o.altM != null ? t.alt_m - o.altM : null
      }
    }
    for (const r of ctx.routes) {
      if (r.domain !== 'air') continue
      const d = distanceToPath(at, r.path)
      if (d < distM) {
        distM = d
        nearest = null
        dAltM = t.alt_m != null ? t.alt_m - ROUTE_ALT_M : null
      }
    }
  } else {
    for (const o of ctx.ours) {
      const d = Math.hypot(...localXY(at, o.at))
      if (o.domain === 'air') underAir = Math.min(underAir, d)
      else if (o.domain === 'surface' && d < distM) {
        distM = d
        nearest = o.uid
      }
    }
    for (const r of ctx.routes) {
      const d = distanceToPath(at, r.path)
      if (r.domain === 'air') underAir = Math.min(underAir, d)
      else if (r.domain === 'surface' && d < distM) {
        distM = d
        nearest = null
      }
    }
  }
  const dz = dAltM == null ? 0 : Math.abs(dAltM)
  if (t.domain === 'air') {
    if ((t.class === 'airliner' || t.class === 'bizjet') && (t.alt_m ?? 0) > FLATTEN_ALT_M) return { tier: 'far', distM, nearest, dAltM, why: 'an airliner at altitude' }
    if (distM <= AIR_NEAR_M && dz <= AIR_NEAR_ALT_M) return { tier: 'near', distM, nearest, dAltM, why: 'near our aircraft or their routes' }
    if (distM <= AIR_MID_M && (t.alt_m == null || t.alt_m <= AIR_MID_ALT_M)) return { tier: 'mid', distM, nearest, dAltM, why: 'low, within a few km of the fleet' }
    return { tier: 'far', distM, nearest, dAltM, why: Number.isFinite(distM) ? 'far from the fleet or well above it' : 'none of ours in the air' }
  }
  if (distM <= SURFACE_NEAR_M) return { tier: 'near', distM, nearest, dAltM: null, why: 'near our boats or their routes' }
  if (distM <= SURFACE_MID_M || (BIG_VESSELS.has(t.class) && underAir <= SHIP_UNDER_AIR_M)) return { tier: 'mid', distM, nearest, dAltM: null, why: distM <= SURFACE_MID_M ? 'within a few km of our boats' : 'a large vessel under our aircraft' }
  return { tier: 'far', distM, nearest, dAltM: null, why: Number.isFinite(distM) ? 'far from our boats' : 'none of ours on the water' }
}

/** At most this many tracks are drawn in full (an icon at full size, its speed leader, a label
 *  candidate): the nearest; the rest of the near ones are drawn as mid. A flagged or named track is
 *  never demoted. */
export const MAX_NEAR = 40

export interface ShownTrack {
  track: Track
  rel: Relevance
  /** 0 to 1: how strongly it is drawn (tier, then how fresh: trackAlpha) */
  alpha: number
  /** drawn as provisional (a tentative track: outlined, not filled) */
  provisional: boolean
  /** something about it is wrong: a red ring */
  alarm: boolean
}

export interface DeclutterOptions {
  /** the domains shown (the bar's toggles) */
  air: boolean
  surface: boolean
  /** tracks that are near whatever their distance: selected, hovered, named by an alert */
  force?: ReadonlySet<string>
}

export interface Decluttered {
  shown: ShownTrack[]
  /** counts of what is drawn, for the bar */
  counts: { air: number; surface: number; near: number; hidden: number }
}

/** The tracks to draw and how: per domain toggle, by relevance, the nearest first within a tier;
 *  the number drawn in full capped at MAX_NEAR. */
export function declutter(tracks: readonly Track[], ctx: FleetContext, opts: DeclutterOptions): Decluttered {
  const counts = { air: 0, surface: 0, near: 0, hidden: 0 }
  const rows: ShownTrack[] = []
  for (const t of tracks) {
    if ((t.domain === 'air' && !opts.air) || (t.domain === 'surface' && !opts.surface)) {
      counts.hidden++
      continue
    }
    const forced = isAlarming(t) || (opts.force?.has(t.id) ?? false)
    const rel = relevance(t, ctx, forced)
    rows.push({ track: t, rel, alpha: 1, provisional: t.state === 'tentative', alarm: isAlarming(t) })
    if (t.domain === 'air') counts.air++
    else counts.surface++
  }
  // the cap: forced first, then by distance
  const near = rows.filter((r) => r.rel.tier === 'near')
  if (near.length > MAX_NEAR) {
    const forced = (r: ShownTrack) => r.alarm || (opts.force?.has(r.track.id) ?? false)
    near.sort((a, b) => Number(forced(b)) - Number(forced(a)) || a.rel.distM - b.rel.distM || (a.track.id < b.track.id ? -1 : 1))
    for (const r of near.slice(MAX_NEAR)) if (!forced(r)) r.rel = { ...r.rel, tier: 'mid', why: `${r.rel.why} (more than ${MAX_NEAR} near: drawn smaller)` }
  }
  for (const r of rows) {
    r.alpha = trackAlpha(r.track, r.rel.tier)
    if (r.rel.tier === 'near') counts.near++
  }
  return { shown: rows, counts }
}

// ---- how sure it is (fade) ----------------------------------------------------------------------

const TIER_ALPHA: Record<Tier, number> = { near: 1, mid: 0.78, far: 0.38 }
/** never fainter than this: a faded track is still there */
export const MIN_ALPHA = 0.2

/** How strongly a track is drawn: by its tier, then a coasting track (no report, extrapolated) at
 *  half, a stale one (overdue) at two thirds, a tentative one a little less. */
export function trackAlpha(t: Pick<Track, 'state' | 'flags'>, tier: Tier): number {
  let a = TIER_ALPHA[tier]
  if (t.state === 'coasting') a *= 0.5
  else if (t.flags.includes('stale')) a *= 0.66
  if (t.state === 'tentative') a *= 0.85
  return Math.max(MIN_ALPHA, Math.round(a * 100) / 100)
}

/** Seconds since the track's freshest source last reported, on the picture's own clock. */
export function ageS(t: Pick<Track, 'sources'>, pictureT: number): number | null {
  if (!t.sources.length) return null
  const last = Math.max(...t.sources.map((s) => s.last_t))
  return Math.max(0, (pictureT - last) / 1000)
}

// ---- where it is heading ------------------------------------------------------------------------

/** The spectral density of the tracker's white-noise acceleration (m^2/s^3), by kind of track
 *  (fleetmesh-tracker README, "Filter"): what its uncertainty grows by. */
export function processNoise(t: Pick<Track, 'domain' | 'cooperative'>): number {
  if (t.domain === 'air') return t.cooperative ? 4 : 9
  return t.cooperative ? 0.05 : 0.2
}

/** How far ahead the predicted path runs, and its ticks: a minute for aircraft, two for vessels. */
export function predictionHorizon(domain: TrafficDomain): { horizonS: number; stepS: number } {
  return domain === 'air' ? { horizonS: 60, stepS: 15 } : { horizonS: 120, stepS: 30 }
}

export interface Predicted {
  dt: number
  at: LngLat
  altM: number | null
  /** 1 sigma, metres: sqrt(sigma0^2 + q dt^3 / 3), the tracker's own growth */
  sigmaM: number
}

/** A constant-velocity prediction (the tracker's own model) from the track's time, `stepS` apart up
 *  to `horizonS`; empty when it has no velocity (a bearing-only track). */
export function predictPath(t: Track, horizonS?: number, stepS?: number): Predicted[] {
  if (!t.vel_mps || isBearingOnly(t)) return []
  const h = predictionHorizon(t.domain)
  const H = horizonS ?? h.horizonS
  const S = stepS ?? h.stepS
  const q = processNoise(t)
  const out: Predicted[] = []
  const at: LngLat = [t.lon, t.lat]
  const k = Math.cos((t.lat * Math.PI) / 180)
  for (let dt = 0; dt <= H + 1e-9; dt += S) {
    const p: LngLat = [at[0] + (t.vel_mps[0] * dt) / (M_PER_DEG * k), at[1] + (t.vel_mps[1] * dt) / M_PER_DEG]
    const alt = t.alt_m == null ? null : Math.max(0, t.alt_m + (t.vs_mps ?? 0) * dt)
    out.push({ dt, at: p, altM: alt, sigmaM: Math.sqrt(t.h_sigma_m ** 2 + (q * dt ** 3) / 3) })
  }
  return out
}

/** The speed leader: where it will be in `leadS` (30 s for aircraft, 60 s for vessels), on the
 *  ground, or null when it is not moving. */
export function leaderEnd(t: Track, leadS = t.domain === 'air' ? 30 : 60): LngLat | null {
  const v = speedOf(t)
  if (v == null || v < 0.5 || !t.vel_mps) return null
  const k = Math.cos((t.lat * Math.PI) / 180)
  return [t.lon + (t.vel_mps[0] * leadS) / (M_PER_DEG * k), t.lat + (t.vel_mps[1] * leadS) / M_PER_DEG]
}

// ---- bearing-only: a wedge, never a point -------------------------------------------------------

/** How many sigma either side of the bearing the wedge spans (about 95 %). */
export const WEDGE_SIGMA = 2

export interface Wedge {
  /** the wedge's outline: the near arc, then the far arc back ([lon, lat, metres MSL]) */
  polygon: [number, number, number][]
  /** the line of bearing itself from the near end of the interval to the far */
  centre: [number, number, number][]
  /** the sensor that sees it */
  sensor: [number, number, number]
  /** the half-angle drawn, degrees */
  halfDeg: number
}

/** The height along the line of sight `r` metres out: the sensor's height plus r tan(elevation),
 *  never below the sea; the sensor's own height when the elevation is unknown. */
export function heightAlong(b: Pick<BearingLine, 'sensor_alt_m' | 'elev_deg'>, r: number): number {
  const e = b.elev_deg == null ? 0 : (b.elev_deg * Math.PI) / 180
  return Math.max(0, b.sensor_alt_m + r * Math.tan(e))
}

/** The region a bearing-only track can be in: from the near to the far end of its range interval,
 *  WEDGE_SIGMA bearing sigmas either side of the line (at least half a degree), in 3D along the
 *  elevation when it is known. `steps` segments to each arc. */
export function bearingWedge(b: BearingLine, steps = 10): Wedge {
  const sensor: LngLat = [b.sensor_lon, b.sensor_lat]
  const [r0raw, r1raw] = b.range_m
  const r0 = Math.max(0, Math.min(r0raw, r1raw))
  const r1 = Math.max(r0 + 1, Math.max(r0raw, r1raw))
  const half = Math.max(0.5, WEDGE_SIGMA * Math.abs(b.sigma_bearing_deg))
  const pt = (bearing: number, r: number): [number, number, number] => {
    const [lon, lat] = destination(sensor, bearing, r)
    return [lon, lat, heightAlong(b, r)]
  }
  const near: [number, number, number][] = []
  const far: [number, number, number][] = []
  for (let k = 0; k <= steps; k++) {
    const a = b.bearing_deg - half + (2 * half * k) / steps
    near.push(pt(a, r0))
    far.push(pt(a, r1))
  }
  const polygon = [...near, ...far.reverse(), near[0]]
  return { polygon, centre: [pt(b.bearing_deg, r0), pt(b.bearing_deg, r1)], sensor: [b.sensor_lon, b.sensor_lat, b.sensor_alt_m], halfDeg: half }
}

// ---- the pairs a disagreement names ------------------------------------------------------------

/** For each cooperative track flagged sensor_disagree, the flagged non-cooperative track the
 *  company's sensors see instead (the nearest within `maxM`): where it says it is, and where our
 *  sensors see something moving exactly like it. */
export function disagreementPairs(tracks: readonly Track[], maxM = 3000): { broadcast: Track; sensed: Track; distM: number }[] {
  const flagged = tracks.filter((t) => t.flags.includes('sensor_disagree'))
  const sensed = flagged.filter((t) => !t.cooperative)
  const out: { broadcast: Track; sensed: Track; distM: number }[] = []
  for (const b of flagged) {
    if (!b.cooperative) continue
    let best: Track | null = null
    let bestD = maxM
    for (const s of sensed) {
      if (s.domain !== b.domain) continue
      const d = Math.hypot(...localXY([b.lon, b.lat], [s.lon, s.lat]))
      if (d <= bestD) {
        best = s
        bestD = d
      }
    }
    if (best) out.push({ broadcast: b, sensed: best, distM: bestD })
  }
  return out
}

// ---- the simulator's truth beside the picture (dev runs only) ----------------------------------

export interface TruthMatch {
  truth: TruthObject
  /** the track that stands for it, or null: the company misses it */
  track: Track | null
  /** horizontal metres between them */
  errM: number | null
}

/** Each truth object with the track that stands for it: by identity (ICAO address, MMSI, UAS id),
 *  else the nearest unclaimed non-cooperative track of its domain within `gateM` (or 4 sigma of
 *  that track). What has none is what the picture misses. */
export function matchTruth(truth: readonly TruthObject[], tracks: readonly Track[], gateM = 400): TruthMatch[] {
  const byIcao = new Map<string, Track>()
  const byMmsi = new Map<number, Track>()
  const byUas = new Map<string, Track>()
  for (const t of tracks) {
    if (t.ids.icao24) byIcao.set(t.ids.icao24.replace(/^~/, '').toLowerCase(), t)
    if (t.ids.mmsi != null) byMmsi.set(t.ids.mmsi, t)
    if (t.ids.uas_id) byUas.set(t.ids.uas_id, t)
  }
  const claimed = new Set<string>()
  const out: TruthMatch[] = []
  const pending: TruthObject[] = []
  for (const o of truth) {
    const i = o.ids ?? {}
    const t = (i.icao24 ? byIcao.get(i.icao24.toLowerCase()) : undefined) ?? (i.mmsi != null ? byMmsi.get(i.mmsi) : undefined) ?? (i.uas_id ? byUas.get(i.uas_id) : undefined)
    if (t) {
      claimed.add(t.id)
      out.push({ truth: o, track: t, errM: Math.hypot(...localXY([o.lon, o.lat], [t.lon, t.lat])) })
    } else pending.push(o)
  }
  const free = tracks.filter((t) => !t.cooperative)
  for (const o of pending) {
    let best: Track | null = null
    let bestD = Infinity
    for (const t of free) {
      if (claimed.has(t.id) || t.domain !== o.domain) continue
      const d = Math.hypot(...localXY([o.lon, o.lat], [t.lon, t.lat]))
      if (d < bestD && d <= Math.max(gateM, 4 * t.h_sigma_m)) {
        best = t
        bestD = d
      }
    }
    if (best) claimed.add(best.id)
    out.push({ truth: o, track: best, errM: best ? bestD : null })
  }
  return out
}

// ---- the picture's health -----------------------------------------------------------------------

export interface FeedRow {
  name: string
  connected: boolean
  ageS: number | null
  rate: number
  tone: 'ok' | 'warn' | 'bad'
}

/** A feed is quiet past this many seconds without a line (warn), silent past SILENT_S (bad). */
export const QUIET_S = 15
export const SILENT_S = 60

/** The feeds as rows, worst first then by name: disconnected or silent bad, quiet warn. */
export function feedRows(p: Picture | null, pictureT: number): FeedRow[] {
  if (!p) return []
  const rows = Object.entries(p.feeds ?? {}).map(([name, f]): FeedRow => {
    const age = f.last_t == null ? null : Math.max(0, (pictureT - f.last_t) / 1000)
    const tone: FeedRow['tone'] = !f.connected || age == null || age > SILENT_S ? 'bad' : age > QUIET_S ? 'warn' : 'ok'
    return { name, connected: f.connected, ageS: age, rate: f.lines_per_s, tone }
  })
  const w = { bad: 0, warn: 1, ok: 2 }
  return rows.sort((a, b) => w[a.tone] - w[b.tone] || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
}
