// One exception slot per vehicle (bd murmur-iobd.1, logs/ui-sweep W1): the one thing wrong with one
// of ours, said in the same words on its fleet row and under its map symbol, and the only place on
// either that takes colour for it: a silent or dropped link, a lost position fix and a low
// battery each had a mark of its own somewhere, or none.
//
// The most pressing one wins, in this order:
//
//   lost       an operator confirmed it lost (rules 30): settled, grey
//   presumed   the engine presumes it lost on evidence (rules 30)
//   silent     its link is stale: "SILENT 12s" (rules 30), "LOST 12s" before it (nodeLoss.ts)
//   link_down  the link emulator has its link down, before it goes silent
//   no_gnss    no satellite fix: dead reckoning, or no position its autopilot trusts while under
//              way
//   low_battery its battery at LOW_BATTERY_PCT or under, or too low to bring it home with a
//              margin at its platform's endurance
//   degraded   the link emulator degrades it, or the bridge measures more than LOSS_PCT loss
//
// Display only: nothing here is a decision or changes one. Pure (no browser):
// vehicleException.check.ts runs it under node.
import type { Priority } from './declutter.ts'
import { type NodeLossView, type NodeStanding, staleBadge, standingOf } from './nodeLoss.ts'
import { distanceM } from './geo.ts'
import type { Vehicle } from './types'
import { isStale } from './types.ts'

export type ExceptionKind = 'lost' | 'presumed' | 'silent' | 'link_down' | 'no_gnss' | 'low_battery' | 'degraded'

/** `bad` red, `warn` amber, `settled` grey (a person decided it: no longer an alarm) */
export type ExceptionTone = 'bad' | 'warn' | 'settled'

export interface VehicleException {
  kind: ExceptionKind
  tone: ExceptionTone
  /** the slot's words, on the row and under the symbol alike: "SILENT 12s" */
  text: string
  /** what it means and where to look, for the hover */
  title: string
}

/** Measured packet loss above this, %, is a degraded link (as the fleet row's numbers turned amber). */
export const LOSS_PCT = 10

/** A battery at this, %, or under is low whatever the way home. */
export const LOW_BATTERY_PCT = 20
/** A battery is low too when what is left of the platform's endurance is under this many times
 *  the time to fly home at its cruise speed. */
export const HOME_MARGIN = 1.5

/** What the battery has to do: what is left of the platform's endurance and the time home, s
 *  (null where the catalog or the report does not say). */
export interface BatteryFacts {
  pct: number
  leftS: number | null
  homeS: number | null
}

/** Whether a battery is low: at the floor, or short of the way home with its margin. */
export function batteryLow(b: BatteryFacts): boolean {
  if (b.pct <= LOW_BATTERY_PCT) return true
  return b.leftS != null && b.homeS != null && b.leftS < b.homeS * HOME_MARGIN
}

export interface ExceptionFacts {
  stale: boolean
  /** seconds since it was last heard */
  silentS: number
  /** rules 30's standing (null before rules 30, or reporting and not presumed) */
  standing: NodeStanding | null
  /** the link emulator's state for its link, the cause in words */
  emu: { state: 'ok' | 'degraded' | 'down'; cause: string } | null
  lossPct: number | null
  /** the map draws where it probably is (the bridge's estimate) */
  estimate: boolean
  /** its position without a satellite fix: `dead_reckoning` (inertial or airspeed), or
   *  `none` while under way; null when it has one */
  noGnss?: 'dead_reckoning' | 'none' | null
  /** its battery, when it reports one */
  battery?: BatteryFacts | null
}

/** The one exception to show, or none. */
export function vehicleException(f: ExceptionFacts): VehicleException | null {
  const silence = f.stale ? staleBadge(f.standing, f.silentS) : ''
  const where = f.estimate ? ' The map shows where it probably is.' : ''
  if (f.standing === 'lost') {
    return { kind: 'lost', tone: 'settled', text: 'LOST', title: 'Confirmed lost by an operator: it takes no order (Review).' }
  }
  if (f.standing === 'presumed_lost') {
    return { kind: 'presumed', tone: 'bad', text: silence || 'PRESUMED LOST', title: `Presumed lost by the engine on evidence: a flag that decides nothing until an operator confirms it (Review).${where}` }
  }
  if (f.stale) {
    return { kind: 'silent', tone: 'bad', text: silence, title: `Not heard for ${Math.floor(f.silentS)} s: its link is lost.${where}` }
  }
  if (f.emu?.state === 'down') {
    return { kind: 'link_down', tone: 'bad', text: 'LINK DOWN', title: `The link emulator has its link down${f.emu.cause ? `: ${f.emu.cause}` : ''}. It goes silent once nothing more is heard.` }
  }
  if (f.noGnss) {
    return {
      kind: 'no_gnss',
      tone: 'warn',
      text: 'NO GNSS',
      title:
        f.noGnss === 'dead_reckoning'
          ? 'No satellite fix: it navigates on dead reckoning, its position error growing with distance travelled.'
          : 'No satellite fix and no position its autopilot trusts: it cannot navigate to an order.',
    }
  }
  if (f.battery && batteryLow(f.battery)) {
    const b = f.battery
    const left = b.leftS != null ? ` About ${minutes(b.leftS)} left at its endurance` : ''
    const home = b.homeS != null ? `${left ? ';' : ' About'} ${minutes(b.homeS)} to get home` : ''
    return { kind: 'low_battery', tone: 'warn', text: `LOW BATTERY ${Math.round(b.pct)}%`, title: `Battery ${Math.round(b.pct)}%.${left}${home}${left || home ? '.' : ''}` }
  }
  if (f.emu?.state === 'degraded') {
    return { kind: 'degraded', tone: 'warn', text: 'LINK DEGRADED', title: `The link emulator degrades its link${f.emu.cause ? `: ${f.emu.cause}` : ''}.` }
  }
  if (f.lossPct != null && f.lossPct > LOSS_PCT) {
    return { kind: 'degraded', tone: 'warn', text: `LINK ${Math.round(f.lossPct)}% LOSS`, title: `The bridge measures ${Math.round(f.lossPct)}% packet loss over the last 10 s.` }
  }
  return null
}

function minutes(s: number): string {
  return s < 90 ? `${Math.max(0, Math.round(s))} s` : `${Math.round(s / 60)} min`
}

/** How the map's label budget weighs a slot under the symbol (declutter.ts), given its vehicle's
 *  own priority: a presumed loss with what waits on the person, a silence or a dropped link
 *  with ours on alert (as the LOST badge always was), the rest as the vehicle. */
export function exceptionPriority(x: VehicleException, own: Priority): Priority {
  const most: Priority = x.kind === 'presumed' ? 1 : x.kind === 'silent' || x.kind === 'link_down' || x.kind === 'lost' ? 3 : 4
  return Math.min(own, most) as Priority
}

/** What the slots are made from, beside each vehicle's own report (App.tsx). */
export interface ExceptionContext {
  /** rules 30: our vehicles' standing (the review's) */
  nodes: NodeLossView
  /** each platform's endurance and cruise speed (the catalog, /api/platforms): a battery too low
   *  to bring its vehicle home; without it, only the floor */
  platforms?: ReadonlyMap<string, { endurance_h?: number | null; cruise_mps?: number | null }> | null
}

export const NO_EXCEPTIONS: ExceptionContext = {
  nodes: { on: false, review: new Map(), lost: new Map() },
}

/** What a vehicle's report says of its navigation and battery, where it carries them (a lite
 *  snapshot out of view carries no battery). */
type Report = Partial<Pick<Vehicle, 'kind' | 'identity' | 'nav_source' | 'armed' | 'battery_pct' | 'position' | 'home'>>

/** Its satellite fix, as the slot reads it. */
export function noGnssOf(v: Report): 'dead_reckoning' | 'none' | null {
  if (v.nav_source !== 'dead_reckoning' && v.nav_source !== 'none') return null
  // `none` before its first fix is every vehicle at start: trouble only once it is under way
  if (v.nav_source === 'none' && !v.armed) return null
  return v.nav_source
}

/** Its battery against the way home (`platforms`: the catalog's endurance and cruise speed). */
export function batteryOf(v: Report, platforms: ExceptionContext['platforms']): BatteryFacts | null {
  if (v.battery_pct == null) return null
  const p = v.identity ? platforms?.get(v.identity.platform) : undefined
  const leftS = p?.endurance_h ? (v.battery_pct / 100) * p.endurance_h * 3600 : null
  const homeS = p?.cruise_mps && v.position && v.home ? distanceM(v.position, v.home) / p.cruise_mps : null
  return { pct: v.battery_pct, leftS, homeS }
}

/** A vehicle's slot. `cause` puts the link emulator's cause in words (linkEmu.ts emuCause). */
export function exceptionOf(v: Pick<Vehicle, 'uid' | 'link' | 'estimate'> & Report, ctx: ExceptionContext, cause: (c: string | null) => string = (c) => c ?? ''): VehicleException | null {
  const stale = isStale(v)
  const emu = v.link.emu
  return vehicleException({
    stale,
    silentS: (v.link.since_last_ms ?? 0) / 1000,
    standing: standingOf(ctx.nodes, v.uid, stale),
    emu: emu ? { state: emu.state, cause: cause(emu.cause) } : null,
    lossPct: v.link.loss_pct,
    estimate: stale && !!v.estimate,
    noGnss: noGnssOf(v),
    battery: batteryOf(v, ctx.platforms),
  })
}
