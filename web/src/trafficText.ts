// The words for a traffic track (traffic.ts): who it is, what it is, how high, how fast, what
// told us, how sure, and what its flags mean, in plain words. Pure: trafficText.check.ts.
import { ageS, hasFlag, isBearingOnly, speedOf, courseOf, type Track, type TrackFlag, type TrackSource, type TrackState, type TrafficClass, type TrafficSensor, type TrackStatic } from './traffic.ts'

const FT_PER_M = 3.28084
const KN_PER_MPS = 1.943844

const CLASS_WORDS: Record<TrafficClass, string> = {
  airliner: 'airliner',
  bizjet: 'business jet',
  ga: 'light aircraft',
  helicopter: 'helicopter',
  seaplane: 'seaplane',
  drone: 'drone (another operator)',
  unknown_air: 'aircraft of unknown type',
  cargo: 'cargo ship',
  tanker: 'tanker',
  passenger: 'passenger ship',
  ferry: 'ferry',
  tug: 'tug',
  pilot: 'pilot boat',
  fishing: 'fishing boat',
  sailing: 'sailboat',
  pleasure: 'pleasure craft',
  small_craft: 'small craft',
  unknown_surface: 'vessel of unknown type',
}

export function classWords(c: string): string {
  return CLASS_WORDS[c as TrafficClass] ?? c.replace(/_/g, ' ')
}

const clean = (s: string | undefined | null) => (s ?? '').trim()

/** Its name as a person would say it: the callsign broadcast (an aircraft's, trimmed), else the
 *  vessel's or the drone's name, else null. */
export function trackName(t: Pick<Track, 'ids' | 'static'>): string | null {
  return clean(t.ids.callsign) || clean(t.ids.name) || clean(t.static?.name) || null
}

/** Its address as data: "ICAO A1B2C3", "MMSI 366123456", "UAS 1581F5..."; null for none. */
export function trackCode(t: Pick<Track, 'ids'>): string | null {
  if (t.ids.icao24) return `ICAO ${t.ids.icao24.replace(/^~/, '').toUpperCase()}`
  if (t.ids.mmsi != null) return `MMSI ${t.ids.mmsi}`
  if (t.ids.uas_id) return `UAS ${t.ids.uas_id}`
  return null
}

/** The one name the map and the cards give it: its name, else its address, else "anonymous" for a
 *  broadcast with no identity, else "unidentified" for what only our sensors see. */
export function trackTitle(t: Pick<Track, 'ids' | 'static' | 'cooperative'>): string {
  if (t.ids.anonymous) return trackName(t) ?? 'anonymous'
  return trackName(t) ?? trackCode(t) ?? (t.cooperative ? 'anonymous' : 'unidentified')
}

/** The short form on the map: the name, else the address's digits alone, else "?" for what only
 *  our sensors see (a non-cooperative track has no name to give). */
export function mapName(t: Pick<Track, 'ids' | 'static' | 'cooperative'>): string {
  const n = trackName(t)
  if (n) return n
  if (t.ids.icao24) return t.ids.icao24.replace(/^~/, '').toUpperCase()
  if (t.ids.mmsi != null) return String(t.ids.mmsi)
  if (t.ids.uas_id) return t.ids.uas_id.length > 10 ? `${t.ids.uas_id.slice(0, 9)}…` : t.ids.uas_id
  return t.cooperative ? 'anon' : '?'
}

const int = (n: number) => Math.round(n).toLocaleString('en-US')

/** The altitude on its map label, in metres as the fleet's labels are, with its reference:
 *  "520 m baro" (pressure altitude, what ADS-B reports) or "95 m MSL" (geometric, as the fleet
 *  flies); "alt ?" for an aircraft with none; null for a vessel. */
export function altLabel(t: Pick<Track, 'domain' | 'alt_m' | 'alt_ref' | 'vs_mps'>): string | null {
  if (t.domain !== 'air') return null
  if (t.alt_m == null || t.alt_ref === 'none') return 'alt ?'
  const trend = t.vs_mps == null || Math.abs(t.vs_mps) < 1 ? '' : t.vs_mps > 0 ? ' ↑' : ' ↓'
  return `${int(t.alt_m)} m ${t.alt_ref === 'baro' ? 'baro' : 'MSL'}${trend}`
}

/** The altitude in full, both units and the reference spelled out. */
export function altDetail(t: Pick<Track, 'domain' | 'alt_m' | 'alt_ref' | 'v_sigma_m'>): string {
  if (t.domain !== 'air') return 'on the surface'
  if (t.alt_m == null || t.alt_ref === 'none') return 'unknown (no source gives it)'
  const ref = t.alt_ref === 'baro' ? 'pressure altitude (standard setting, as ADS-B reports it; within some tens of metres of mean sea level)' : 'above mean sea level, as the fleet flies'
  return `${int(t.alt_m)} m (${int(t.alt_m * FT_PER_M)} ft), ${ref}`
}

/** "62 m/s (121 kn)"; null when unknown. */
export function speedText(t: Pick<Track, 'vel_mps'>): string | null {
  const v = speedOf(t)
  if (v == null) return null
  return `${v < 10 ? v.toFixed(1) : int(v)} m/s (${int(v * KN_PER_MPS)} kn)`
}

/** A vessel's speed on its map label, in knots as at sea: "12 kn"; null for a still one or none. */
export function knotsLabel(t: Pick<Track, 'vel_mps'>): string | null {
  const v = speedOf(t)
  return v == null || v < 0.5 ? null : `${int(v * KN_PER_MPS)} kn`
}

export function courseText(t: Pick<Track, 'heading_deg' | 'vel_mps'>): string | null {
  const c = courseOf(t)
  return c == null ? null : `${String(Math.round(c) % 360).padStart(3, '0')}°`
}

export function climbText(t: Pick<Track, 'domain' | 'vs_mps'>): string | null {
  if (t.domain !== 'air' || t.vs_mps == null) return null
  if (Math.abs(t.vs_mps) < 0.5) return 'level'
  return `${t.vs_mps > 0 ? 'climbing' : 'descending'} ${Math.abs(t.vs_mps).toFixed(1)} m/s (${int(Math.abs(t.vs_mps) * FT_PER_M * 60)} ft/min)`
}

/** "0.4 s", "12 s", "3 min", "2 h". */
export function ageText(s: number | null): string {
  if (s == null || !Number.isFinite(s)) return 'unknown'
  if (s < 10) return `${s.toFixed(1)} s`
  if (s < 120) return `${Math.round(s)} s`
  if (s < 7200) return `${Math.round(s / 60)} min`
  return `${Math.round(s / 3600)} h`
}

/** "1.2 km", "350 m". */
export function distText(m: number): string {
  if (!Number.isFinite(m)) return 'unknown'
  return m >= 1000 ? `${(m / 1000).toFixed(m >= 10_000 ? 0 : 1)} km` : `${int(m)} m`
}

const SOURCE_WORDS: Record<string, string> = {
  adsb_1090: 'ADS-B (1090 MHz)',
  adsb_uat: 'ADS-B (UAT 978 MHz)',
  ais_a: 'AIS Class A',
  ais_b: 'AIS Class B',
  remote_id: 'Remote ID',
  daa: 'detect-and-avoid camera',
  marine_radar: 'marine radar',
  ground_radar: 'ground radar',
}

export function sourceKindWords(k: string): string {
  return SOURCE_WORDS[k] ?? k.replace(/_/g, ' ')
}

/** Where a source came from: a hub's receiver, the commercial feed (delayed), the sensor on one of
 *  our vehicles (by its callsign), a radar site. */
export function sourceVia(s: Pick<TrackSource, 'kind' | 'rx'>, nameOf: (uid: string) => string, sensors: readonly TrafficSensor[] = []): string {
  const site = sensors.find((x) => x.id === s.rx)
  if (site?.owner === 'network' || /^net-/.test(s.rx)) return `the commercial feed ${s.rx} (seconds late)`
  if (s.kind === 'daa' || s.kind === 'marine_radar') return `on ${nameOf(s.rx)}`
  if (s.kind === 'remote_id' && !site && /^\d+$/.test(s.rx)) return `on ${nameOf(s.rx)}`
  if (s.kind === 'ground_radar') return `at ${s.rx}`
  return `${s.rx}'s receiver`
}

/** One source in words with its age on the picture's clock: "ADS-B (1090 MHz), hub-pier's
 *  receiver, 0.6 s ago". */
export function sourceLine(s: TrackSource, pictureT: number, nameOf: (uid: string) => string, sensors: readonly TrafficSensor[] = []): string {
  return `${sourceKindWords(s.kind)}, ${sourceVia(s, nameOf, sensors)}, ${ageText(Math.max(0, (pictureT - s.last_t) / 1000))} ago`
}

export type FlagTone = 'bad' | 'warn' | 'info'

const FLAG_WORDS: Record<TrackFlag, { text: string; tone: FlagTone }> = {
  sensor_disagree: { text: "Its broadcast position disagrees with the company's own sensors: they see something moving like it elsewhere, and nothing where it says it is. The broadcast may be faulty or spoofed.", tone: 'bad' },
  position_jump: { text: 'A report jumped further than it could have moved: that report was not used.', tone: 'bad' },
  emergency: { text: 'Squawking an emergency code.', tone: 'bad' },
  bearing_only: { text: 'Bearing only: one sensor saw it along a line of sight, with no range. It is somewhere along that line, not at a point.', tone: 'warn' },
  stale: { text: 'Its source is overdue: the position is extrapolated.', tone: 'warn' },
  invalid_mmsi: { text: 'Its MMSI is not a valid ship station number.', tone: 'warn' },
  id_changed: { text: 'Its anonymous address changed: the track was carried over to the new one.', tone: 'warn' },
  anonymous: { text: 'Anonymous address (a privacy address, a self-assigned one or a Remote ID session id): who it is is not known.', tone: 'info' },
  ladd: { text: 'On the LADD list: its callsign is withheld.', tone: 'info' },
  triangulated: { text: 'Its position comes from crossing bearings of our own sensors, not from a broadcast.', tone: 'info' },
  on_ground: { text: 'On the ground.', tone: 'info' },
  moored: { text: 'Moored.', tone: 'info' },
  at_anchor: { text: 'At anchor.', tone: 'info' },
}

/** A flag in plain words with how much it matters; an unknown flag as its own name. */
export function flagWords(f: string): { text: string; tone: FlagTone } {
  return FLAG_WORDS[f as TrackFlag] ?? { text: f.replace(/_/g, ' '), tone: 'info' }
}

/** Its flags in plain words, what is wrong first. */
export function flagLines(t: Pick<Track, 'flags'>): { flag: string; text: string; tone: FlagTone }[] {
  const w = { bad: 0, warn: 1, info: 2 }
  return t.flags.map((f) => ({ flag: f, ...flagWords(f) })).sort((a, b) => w[a.tone] - w[b.tone])
}

const STATE_WORDS: Record<TrackState, string> = {
  tentative: 'Tentative: seen only a few times, not yet confirmed. It may be clutter.',
  confirmed: 'Confirmed.',
  coasting: 'Coasting: no report lately, its position is extrapolated from the last ones.',
}

export function stateWords(s: string): string {
  return STATE_WORDS[s as TrackState] ?? s
}

/** How sure the position is: "±15 m horizontally (1 sigma), ±10 m vertically"; for a bearing-only
 *  track, the line itself. */
export function uncertaintyText(t: Track, nameOf: (uid: string) => string = (u) => u): string {
  if (isBearingOnly(t)) {
    const b = t.bearing_only
    const who = t.sources.find((s) => s.kind === 'daa')?.rx
    const from = who ? ` from ${nameOf(who)}` : ''
    const elev = b.elev_deg == null ? '' : `, ${b.elev_deg.toFixed(0)}° up`
    return `Somewhere on the line of bearing ${b.bearing_deg.toFixed(0)}° ±${b.sigma_bearing_deg.toFixed(1)}°${from}${elev}, ${distText(b.range_m[0])} to ${distText(b.range_m[1])} out.`
  }
  const v = t.v_sigma_m == null ? '' : `, ±${distText(t.v_sigma_m)} vertically`
  return `±${distText(t.h_sigma_m)} horizontally (1 sigma)${v}.`
}

/** What AIS static data, an aircraft's squawk or Remote ID gave, as rows. */
export function staticRows(s: TrackStatic | null | undefined): [string, string][] {
  if (!s) return []
  const rows: [string, string][] = []
  const add = (k: string, v: string | number | null | undefined) => {
    if (v != null && String(v).trim() !== '') rows.push([k, String(v).trim()])
  }
  add('Name', s.name)
  add('Radio callsign', s.callsign)
  add('IMO', s.imo)
  add('Ship type', s.ship_type_text ?? (s.ship_type != null ? `type ${s.ship_type}` : null))
  if (s.length_m != null) add('Size', `${int(s.length_m)} m long${s.beam_m != null ? `, ${int(s.beam_m)} m beam` : ''}`)
  if (s.draught_m != null) add('Draught', `${s.draught_m.toFixed(1)} m`)
  add('Destination', s.destination)
  add('ETA', s.eta)
  add('Status', s.nav_status_text ?? (s.nav_status != null ? `code ${s.nav_status}` : null))
  add('Mother ship', s.mothership_mmsi)
  add('Squawk', s.squawk)
  add('ID type', s.id_type)
  add('Aircraft type', s.ua_type)
  if (s.op_lat != null && s.op_lon != null) add('Operator at', `${s.op_lat.toFixed(5)}, ${s.op_lon.toFixed(5)}`)
  return rows
}

/** The hover text of a track: who and what, height or speed, how sure, how old. */
export function trackTooltip(t: Track, pictureT: number, nameOf: (uid: string) => string = (u) => u): string {
  const parts = [trackTitle(t), classWords(t.class)]
  if (isBearingOnly(t)) parts.push(`bearing only from ${nameOf(t.sources.find((s) => s.kind === 'daa')?.rx ?? '?')}`)
  else {
    const a = altLabel(t)
    if (a) parts.push(a)
    const k = knotsLabel(t)
    if (k && t.domain === 'surface') parts.push(k)
  }
  if (t.state !== 'confirmed') parts.push(t.state)
  if (hasFlag(t, 'sensor_disagree')) parts.push('DISAGREES WITH OUR SENSORS')
  if (hasFlag(t, 'position_jump')) parts.push('POSITION JUMP')
  if (hasFlag(t, 'emergency')) parts.push('EMERGENCY')
  parts.push(`${ageText(ageS(t, pictureT))} old`)
  return parts.join(' · ')
}

/** The lines of a track's map label: its short name alone, or with the full block a second line
 *  (an aircraft's altitude, a vessel's speed); "?" marks a tentative track. */
export function labelLines(t: Track, full: boolean): string[] {
  const name = `${mapName(t)}${t.state === 'tentative' ? ' ?' : ''}`
  if (!full) return [name]
  const second = t.domain === 'air' ? (isBearingOnly(t) ? 'bearing only' : altLabel(t)) : knotsLabel(t)
  return second ? [name, second] : [name]
}
