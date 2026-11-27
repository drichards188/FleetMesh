// Incidents and plays at the console (docs/design/incidents.md): the Inbox's items of one cause at
// one hub (an incident, the bridge's view), the plays a person adopted (rules 8) and the watch
// officer's recommendation for an incident, in words. Pure (no browser): plays.check.ts. Adopting
// and lifting a play are the person's acts (plays.ts).

export interface Incident {
  /** `<family>@<hub>` */
  id: string
  family: string
  hub: string
  /** its items in the person's Inbox now (`intent:<id>`, `alert:<id>`): their cards fold into its card */
  items: string[]
  /** over the last ten minutes: its held orders and alerts */
  held: number
  alerts: number
  /** of them: in the person's Inbox now, with the watch officer now, settled since */
  mine: number
  with_officer: number
  settled: number
  codes: string[]
  /** the other items at its hub, of another cause: never folded in */
  others: string[]
}

export interface PlayLimits {
  max_trip_m?: number
  max_mass_kg?: number
  min_battery_pct?: number
}

export interface Play {
  id: number
  kind: 'ground_stop' | 'limits'
  hub: string
  codes?: string[]
  limits?: PlayLimits
  except_priority?: number | null
  /** ms, the bridge's clock */
  until_t: number
  lifts?: string | null
  by: string
  /** how its end condition stands (the bridge watches it) */
  lift?: { below_mps: number; clear_s: number; needs_s: number }
  left_s?: number
}

/** The watch officer's recommendation for an incident (the person adopts a play or not). */
export interface PlayRec {
  incident: string
  hub: string
  kind: Play['kind']
  limits?: PlayLimits
  why: string
  by: string
  t: number
}

/** The codes a limits play may answer (the shipped policy's `[plays] codes`; the bridge refuses
 *  any other). */
export const PLAY_CODES = ['WX_GUST_LIMIT_MARGINAL', 'WX_WIND_LIMIT_MARGINAL', 'DZ_GUST_AT_ZONE']

/** The first limits a person is offered: a short, light trip on a well-charged vehicle. */
export const DEFAULT_LIMITS: Required<PlayLimits> = { max_trip_m: 1500, max_mass_kg: 1, min_battery_pct: 60 }

const FAMILY: Record<string, string> = {
  traffic: 'Traffic',
  separation: 'Separation',
  navigation: 'Navigation',
  link: 'Link',
  energy: 'Energy',
  airspace: 'Airspace',
  'drop zone': 'Drop zones',
}

/** "Wind at Crissy Field hub": its cause, at its hub. */
export function incidentTitle(i: Pick<Incident, 'family' | 'codes'>, hubName: string): string {
  const windy = i.codes.some((c) => c.startsWith('WX_GUST') || c.startsWith('WX_WIND') || c === 'DZ_GUST_AT_ZONE')
  const what = i.family === 'weather' ? (windy ? 'Wind' : 'Weather') : (FAMILY[i.family] ?? i.family)
  return `${what} at ${hubName}`
}

/** "4 held orders and 2 alerts" */
export function incidentCount(i: Pick<Incident, 'held' | 'alerts'>): string {
  const parts = [i.held ? `${i.held} held order${i.held === 1 ? '' : 's'}` : '', i.alerts ? `${i.alerts} alert${i.alerts === 1 ? '' : 's'}` : ''].filter(Boolean)
  return parts.join(' and ')
}

/** Where the pattern's items stand: "in 10 min: 2 for you, 3 with the AI watch officer, 9 settled". */
export function incidentStand(i: Pick<Incident, 'mine' | 'with_officer' | 'settled'>): string {
  const parts = [i.mine ? `${i.mine} for you` : '', i.with_officer ? `${i.with_officer} with the AI watch officer` : '', i.settled ? `${i.settled} settled` : ''].filter(Boolean)
  return parts.length ? `in 10 min: ${parts.join(', ')}` : ''
}

/** What a play does, in a line: "Ground stop: no departure without you" or "Within limits: trips
 *  up to 1.5 km, parcels up to 1 kg, battery 60% or more". */
export function playWords(p: Pick<Play, 'kind' | 'limits'>): string {
  if (p.kind === 'ground_stop') return 'Ground stop: nothing leaves without you'
  const l = p.limits ?? {}
  const parts = [
    l.max_trip_m != null ? `trips up to ${l.max_trip_m >= 1000 ? `${(l.max_trip_m / 1000).toFixed(1)} km` : `${Math.round(l.max_trip_m)} m`}` : '',
    l.max_mass_kg != null ? `parcels up to ${l.max_mass_kg} kg` : '',
    l.min_battery_pct != null ? `battery ${l.min_battery_pct}% or more` : '',
  ].filter(Boolean)
  return `Within limits: ${parts.join(', ')}`
}

/** How a play's end stands: "lifts when gusts stay under 11.0 m/s for 5 min (2 of 5 min so far)". */
export function liftWords(p: Pick<Play, 'lift' | 'lifts' | 'left_s'>): string {
  if (p.lift) {
    const done = Math.floor(p.lift.clear_s / 60)
    const need = Math.round(p.lift.needs_s / 60)
    const so = p.lift.clear_s > 0 ? ` (${done} of ${need} min so far)` : ''
    return `lifts when gusts stay under ${p.lift.below_mps.toFixed(1)} m/s for ${need} min${so}`
  }
  if (p.left_s != null) return `ends in ${Math.ceil(p.left_s / 60)} min unless you lift it`
  return p.lifts ?? ''
}

/** An Inbox card's key for an incident's item (InboxPanel.tsx keys an order `i<id>`, an alert by
 *  its id). */
export function cardKey(item: string): string {
  if (item.startsWith('intent:')) return `i${item.slice('intent:'.length)}`
  if (item.startsWith('alert:')) return item.slice('alert:'.length)
  return item
}

/** A person's request for a play at a hub. */
export interface PlayAsk {
  kind: Play['kind']
  hub: string
  codes?: string[]
  limits?: PlayLimits
  except_priority?: number
  lift?: 'wind'
}
