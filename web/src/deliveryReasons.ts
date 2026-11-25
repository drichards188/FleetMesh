// Why a delivery order waits for a person, in plain language (docs/design/delivery-rules.md): a
// sentence for each reason code a rule can give, and the inputs a rule used, read out of the
// check's own detail line (the numbers the engine logged with its decision). Display only: the
// words never change what a check decided. Pure: deliveryReasons.check.ts.

export interface Reason {
  /** a few words, for the card's line */
  title: string
  /** what it means for the person deciding */
  plain: string
}

const R = (title: string, plain: string): Reason => ({ title, plain })

/** Every reason code the delivery rules give (crates/fleetmesh-decide/src/delivery_rules.rs). */
export const REASONS: Record<string, Reason> = {
  // geofence
  GEOFENCE_OUTSIDE_APPROVED: R('Outside the approved area', 'The route leaves the area where flights are approved.'),
  GEOFENCE_NO_GO: R('Enters a no-go area', 'The route crosses an area no vehicle may enter.'),
  GEOFENCE_RESTRICTED: R('Enters a restricted area', 'The route crosses an area where entry needs a person to say yes.'),
  GEOFENCE_TEMP_SOON: R('A restriction starts soon', 'The route is legal now, but a temporary restriction begins on it shortly after the vehicle would be there.'),
  GEOFENCE_CEILING: R('Above the ceiling', 'The route goes higher than the area allows.'),
  GEOFENCE_LOITER_POPULATED: R('Loiter over people', 'A plane would have to loiter over a populated area.'),
  GEOFENCE_AREAS_UNKNOWN: R('Areas unknown', 'The set of approved and restricted areas is missing, so the route cannot be checked against it.'),
  // energy
  ENERGY_BATTERY_UNKNOWN: R('Battery level unknown', 'The vehicle has not reported its battery lately, so the trip cannot be checked against it.'),
  ENERGY_ENDURANCE_UNKNOWN: R('Endurance unknown', 'The platform has no endurance figure to judge the trip by.'),
  ENERGY_MARGIN_THIN: R('Thin battery margin', 'The battery covers the trip, the return to a hub and the reserve, but only just: less spare than the hold band asks for.'),
  ENERGY_NO_HUB_REACHABLE: R('No hub to return to', 'No open hub can be reached from the end of the route with the battery left.'),
  ENERGY_INSUFFICIENT: R('Battery too low', 'The battery does not cover the trip, the return and the reserve.'),
  ENERGY_WIND_NO_HEADWAY: R('Wind too strong to make progress', 'Into this wind the vehicle could not make headway on a leg.'),
  // payload
  PAYLOAD_UNKNOWN: R('Parcel mass unknown', 'The parcel has no weight on record, so it cannot be checked against what the vehicle can carry.'),
  PAYLOAD_NEAR_MAX: R('Parcel near the limit', 'The parcel is close to the most this vehicle can carry in this wind.'),
  PAYLOAD_OVER_MAX: R('Parcel too heavy', 'The parcel is heavier than the vehicle can carry.'),
  PAYLOAD_OVER_DERATED: R('Too heavy for this wind', 'In gusts the vehicle can carry less than the parcel weighs.'),
  PAYLOAD_CLASS: R('Parcel class not carried', 'Hazardous or excluded goods are not carried in this version.'),
  // weather
  WX_UNKNOWN: R('Weather unknown', 'There is no recent weather reading for a place the vehicle will be, and unknown is never assumed to be fine.'),
  WX_WIND_LIMIT_MARGINAL: R('Wind near the limit', 'The sustained wind is above the soft limit and under the hard one: legal, but a person judges it.'),
  WX_GUST_LIMIT_MARGINAL: R('Gusts near the limit', 'Gusts are above the soft limit and under the hard one: legal, but a person judges it.'),
  WX_VISIBILITY_MARGINAL: R('Poor visibility', 'Visibility is below the soft limit and above the hard one.'),
  WX_PRECIP_MARGINAL: R('Heavy rain', 'Rainfall is above the soft limit and under the hard one.'),
  WX_CURRENT_MARGINAL: R('Strong current', 'The water current is a large share of the boat’s cruise speed.'),
  WX_WIND_LIMIT: R('Wind over the limit', 'The sustained wind is over the hard limit.'),
  WX_GUST_LIMIT: R('Gusts over the limit', 'Gusts are over the hard limit.'),
  WX_VISIBILITY: R('Visibility too poor', 'Visibility is under the hard limit.'),
  WX_PRECIP: R('Rain too heavy', 'Rainfall is over the hard limit.'),
  WX_CURRENT_LIMIT: R('Current over the limit', 'The current is over what the boat can hold against.'),
  WX_FLOOD_RISK: R('Flood risk on the route', 'The tide would be above the ground on part of the route.'),
  TIDE_SHALLOW: R('Shallow water', 'At this tide the water is shallower than the boat’s draft plus a keel clearance.'),
  TIDE_AGROUND: R('Would run aground', 'At this tide the water is too shallow for the boat.'),
  TIDE_DOCK_LEVEL: R('Dock level against the tide', 'The water is at or near the dock’s deck level when the boat would arrive.'),
  // separation
  SEP_TRAFFIC_UNKNOWN: R('Nearby traffic unknown', 'Another vehicle near the route has not reported lately, so separation from it cannot be checked.'),
  SEP_MARGINAL: R('Close to another vehicle', 'The route passes another vehicle at less than one and a half times the minimum separation.'),
  SEP_HUB_ZONE: R('Two vehicles for one hub', 'Two vehicles would meet near a hub: a person decides who waits.'),
  SEP_ROUTE_CONFLICT: R('Route conflict', 'The route would lose separation with another vehicle.'),
  // drop zone
  DZ_UNCONFIRMED: R('Drop zone not confirmed lately', 'No person has said this drop zone is fit in the last hour. Confirming it lets the delivery go.'),
  DZ_OBSERVATION_STALE: R('No recent look at a populated zone', 'The zone is where people are, and no one has looked at it lately (or the look could not tell).'),
  DZ_PEOPLE_NEAR: R('People at the drop zone', 'The latest look saw people at the zone: a person can judge whether it is a passer-by.'),
  DZ_OCCUPIED: R('Pad occupied', 'The pad is occupied.'),
  DZ_GUST_AT_ZONE: R('Gusts at the drop zone', 'A multirotor descends to a low hover to release, and the gust at the zone is above its descent limit.'),
  DZ_CLOSED: R('Drop zone closed', 'The drop zone is closed or unknown.'),
  DZ_WRONG_SURFACE: R('Wrong surface', 'The drop zone is on land or water that this vehicle cannot reach.'),
  DZ_SLOPE: R('Ground too steep', 'The slope at the drop zone is over the limit for a release.'),
  // link
  LINK_COVERAGE_GAP: R('Link gap on the route', 'The link is predicted to drop on the route for longer than allowed, and no backup radio covers the gap.'),
  LINK_BACKUP_SLOW: R('On a slow backup link', 'The main link is down and the backup radio carries only a low data rate.'),
  LINK_PLAN_UNKNOWN: R('Failsafe not read', 'The vehicle’s lost-link failsafe has not been read, so what it does if the link drops is not known.'),
  LINK_FAILSAFE_MISMATCH: R('Failsafe is not the policy’s', 'The vehicle’s lost-link failsafe is not what policy wants.'),
  LINK_FAILSAFE_UNSAFE: R('Failsafe would strand it', 'If the failsafe trips on the route, the return from there exceeds the battery.'),
  // navigation
  NAV_DEGRADED_ACCURACY: R('Position fix less accurate', 'The vehicle has a fix, but its accuracy is below what an order needs.'),
  NAV_LOW_SATS: R('Few satellites', 'The vehicle tracks fewer satellites than it should.'),
  NAV_HIGH_HDOP: R('Poor satellite geometry', 'The satellites it tracks are badly placed, so the fix is less certain.'),
  NAV_TRUST_UNKNOWN: R('Navigation state unknown', 'There are not enough recent position facts to say whether its navigation can be trusted.'),
  NAV_UNTRUSTED: R('Navigation untrusted', 'The vehicle’s position cannot be relied on.'),
  NAV_NO_FIX: R('No position fix', 'The vehicle has no GNSS fix.'),
  NAV_2D_FIX: R('Only a 2D fix', 'The vehicle has no altitude from GNSS.'),
  NAV_GLITCH: R('Position glitching', 'The vehicle’s navigation filter flags the GNSS as glitching.'),
  NAV_DEAD_RECKONING: R('Navigating blind', 'The vehicle is estimating its position from motion alone, and drifts.'),
  NAV_JUMP: R('Position jumped', 'The reported position moved farther than the vehicle’s velocity allows: a sign of interference.'),
  NAV_RANGE_MISMATCH: R('Position disagrees with the radio range', 'The range to the serving station does not match the reported position.'),
  NAV_WRONG_SURFACE: R('On the wrong surface', 'The position is on land for a boat, or water for a car.'),
  NAV_IMPOSSIBLE_SPEED: R('Impossible speed', 'The reported speed is more than the vehicle can do.'),
  NAV_STALE_POSITION: R('Position out of date', 'The last position report is too old to rely on.'),
  NAV_RECOVERING: R('Navigation recovering', 'Navigation is better again but has not held for long enough to be trusted.'),
  // orders and hubs
  HUB_NO_PACKS: R('No charged battery at the hub', 'The hub has no charged pack to swap in.'),
  ORDER_WINDOW_PASSED: R('Delivery window passed', 'The time by which the parcel had to arrive is over.'),
  ORDER_SITE_MISMATCH: R('Not at the pickup site', 'The vehicle is not at the place named for the pickup.'),
  ORDER_NOT_AT_HUB: R('Not at a hub', 'A battery swap needs the vehicle on a hub’s pad.'),
  ORDER_HUB_CLOSED: R('Hub closed', 'The hub is closed.'),
  ORDER_PARCEL_UNKNOWN: R('Unknown parcel', 'No parcel has that id.'),
  ORDER_PARCEL_ASSIGNED: R('Parcel already assigned', 'Another vehicle has this parcel.'),
  ORDER_PARCEL_NOT_READY: R('Parcel not ready', 'The parcel is not ready to be loaded yet.'),
  ORDER_PARCEL_ABOARD: R('Parcel already aboard', 'The parcel is already on a vehicle.'),
  ORDER_PARCEL_NOT_ABOARD: R('Parcel not aboard', 'The vehicle does not have this parcel loaded.'),
  ORDER_UNKNOWN_DROP_ZONE: R('Unknown drop zone', 'No drop zone has that id.'),
  ORDER_UNKNOWN_SITE: R('Unknown site', 'No hub or pickup point has that id.'),
  ORDER_NO_ROUTE_TO_REPLACE: R('No route to replace', 'The vehicle has no running order to reroute.'),
  ORDER_WORLD_UNKNOWN: R('Delivery facts missing', 'The places, parcels or orders the check needs are not available, so it cannot be judged.'),
  ORDER_REROUTE_JOIN: R('New route starts too far away', 'The new route starts farther from the vehicle than it may join within.'),
  ORDER_HUB_UNNAMED: R('Not a hub', 'The return names a point that is not a hub: a person decides whether it is a fit place to land.'),
  ORDER_NO_DELIVER_WAITING: R('No delivery waits for this', 'No delivery for this parcel and zone is queued or waiting, so there is nothing to confirm.'),
  ORDER_ABORT_TARGET: R('Nothing to abort', 'The order to stop is not running on this vehicle.'),
  ABORT_PARCEL_RELEASED: R('Parcel already released', 'A released parcel cannot be taken back, so the delivery cannot be aborted.'),
  INPUT_UNKNOWN: R('A fact is missing', 'A fact the rule needs is not known.'),
}

/** A code's reason, or one made from the code itself (a code a newer bridge adds). */
export function reasonOf(code: string): Reason {
  const r = REASONS[code]
  if (r) return r
  const words = code.toLowerCase().replace(/_/g, ' ')
  return { title: words.charAt(0).toUpperCase() + words.slice(1), plain: 'The rule gave this reason; its own detail is below.' }
}

/** A reason code's title only. */
export function reasonLabel(code: string): string {
  return reasonOf(code).title
}

// ---- the inputs a rule used ----------------------------------------------------------------------

/** One input a rule used, as a row: what, its value, and what it was held against. */
export interface InputRow {
  label: string
  value: string
  /** what it is measured against ("soft limit 11 m/s", "reserve 25%") */
  against?: string
  /** `over`: past its limit; `near`: close to it; absent: only said */
  tone?: 'over' | 'near'
}

const num = (s: string) => Number.parseFloat(s)

/** The rows a check's detail gives, by rule. Nothing it does not say is made up: a detail the
 *  patterns do not know gives no rows, and the card shows the detail's own words. */
export function inputRows(rule: string, detail: string): InputRow[] {
  const rows: InputRow[] = []
  if (rule === 'weather') {
    for (const m of detail.matchAll(/(sustained wind|gust|visibility|precipitation) ([\d.]+) (m\/s|m|mm\/h) is (above|below) the soft limit ([\d.]+)/g)) {
      const [, what, v, unit, , lim] = m
      const label = what === 'sustained wind' ? 'Wind' : what.charAt(0).toUpperCase() + what.slice(1)
      rows.push({ label, value: `${v} ${unit}`, against: `soft limit ${lim} ${unit}`, tone: 'over' })
    }
    const cur = /current ([\d.]+) m\/s is ([\d.]+) of cruise, above the soft ([\d.]+)/.exec(detail)
    if (cur) rows.push({ label: 'Current', value: `${cur[1]} m/s (${cur[2]} of cruise)`, against: `soft limit ${cur[3]} of cruise`, tone: 'over' })
    // a pass line says wind, gust and the rest together: "wind 2.8 m/s, gust 4.7 m/s, visibility ..."
    const w = /wind ([\d.]+) m\/s, gust ([\d.]+) m\/s/.exec(detail)
    if (w && !rows.length) rows.push({ label: 'Wind', value: `${w[1]} m/s` }, { label: 'Gust', value: `${w[2]} m/s` })
  } else if (rule === 'energy') {
    const m = /battery (\d+)%, need ([\d.]+)% \(route ([\d.]+) plus return ([\d.]+), times ([\d.]+) for wind margin, equals ([\d.]+), plus reserve (\d+)\), margin (-?[\d.]+) points, hold band (\d+)/.exec(detail)
    if (m) {
      const margin = num(m[8])
      rows.push(
        { label: 'Battery now', value: `${m[1]}%`, against: `needs ${m[2]}%`, tone: margin < 0 ? 'over' : margin < num(m[9]) ? 'near' : undefined },
        { label: 'Route and return', value: `${m[3]}% out + ${m[4]}% back`, against: `×${m[5]} wind margin = ${m[6]}%` },
        { label: 'Reserve', value: `${m[7]}%` },
        { label: 'Margin', value: `${m[8]} points`, against: `hold band ${m[9]} points`, tone: margin < 0 ? 'over' : margin < num(m[9]) ? 'near' : undefined },
      )
    } else {
      const b = /battery (\d+)%/.exec(detail)
      if (b) rows.push({ label: 'Battery now', value: `${b[1]}%` })
    }
  } else if (rule === 'lost_link') {
    const g = /predicted down for ([\d.]+) s on the route \(limit ([\d.]+) s\)/.exec(detail)
    if (g) rows.push({ label: 'Predicted link gap', value: `${g[1]} s`, against: `limit ${g[2]} s`, tone: 'over' })
    if (/backup radio/.test(detail)) rows.push({ label: 'Backup radio', value: 'low data rate' })
    const f = /failsafe (\w+) after ([\w ]+) s; policy wants (\w+) after ([\w ]+) s/.exec(detail)
    if (f) rows.push({ label: 'Failsafe', value: `${f[1]} after ${f[2]} s`, against: `policy wants ${f[3]} after ${f[4]} s`, tone: 'over' })
  } else if (rule === 'nav_integrity') {
    const acc = /horizontal accuracy ([\d.]+) m is above ([\d.]+) m/.exec(detail)
    if (acc) rows.push({ label: 'Position accuracy', value: `${acc[1]} m`, against: `trusted up to ${acc[2]} m`, tone: 'over' })
    const sats = /(\d+) satellites tracked, fewer than (\d+)/.exec(detail)
    if (sats) rows.push({ label: 'Satellites', value: sats[1], against: `at least ${sats[2]}`, tone: 'over' })
    const hd = /HDOP ([\d.]+) is above ([\d.]+)/.exec(detail)
    if (hd) rows.push({ label: 'HDOP', value: hd[1], against: `at most ${hd[2]}`, tone: 'over' })
    const jump = /jumped (\d+) m/.exec(detail)
    if (jump) rows.push({ label: 'Position jump', value: `${jump[1]} m`, tone: 'over' })
    const fix = /GPS fix type (\d+)/.exec(detail)
    if (fix) rows.push({ label: 'GNSS fix type', value: fix[1], against: 'a 3D fix is 3', tone: 'over' })
  } else if (rule === 'separation') {
    const m = /closest approach ([\d.]+) of the (\d+) m minimum at \+(\d+) s/.exec(detail)
    if (m) rows.push({ label: 'Closest approach', value: `${Math.round(num(m[1]) * num(m[2]))} m at +${m[3]} s`, against: `minimum ${m[2]} m`, tone: num(m[1]) < 1 ? 'over' : 'near' })
    const old = /^(\S+)'s position is (\d+) s old/.exec(detail)
    if (old) rows.push({ label: `Vehicle ${old[1]}'s position`, value: `${old[2]} s old`, tone: 'near' })
  } else if (rule === 'payload') {
    const m = /parcel ([\d.]+) kg, platform maximum ([\d.]+) kg, effective maximum ([\d.]+) kg/.exec(detail)
    if (m) rows.push({ label: 'Parcel', value: `${m[1]} kg`, against: `can carry ${m[3]} kg here (maximum ${m[2]} kg)`, tone: num(m[1]) > num(m[3]) ? 'over' : 'near' })
  } else if (rule === 'dropzone') {
    const g = /gust ([\d.]+) m\/s at the zone is above the descent limit ([\d.]+)/.exec(detail)
    if (g) rows.push({ label: 'Gust at the zone', value: `${g[1]} m/s`, against: `descent limit ${g[2]} m/s`, tone: 'over' })
    const p = /(\d+) person\(s\) seen/.exec(detail)
    if (p) rows.push({ label: 'People seen', value: p[1] })
    const c = /was not confirmed within (\d+) s/.exec(detail)
    if (c) rows.push({ label: 'Last confirmation', value: `none in the last ${Math.round(num(c[1]) / 60)} min` })
  }
  return rows
}

/** The delivery kinds an inbox card gives the delivery reasons to. */
export const DELIVERY_KINDS = new Set(['pickup', 'deliver', 'reroute', 'return_to_hub', 'land_now', 'abort', 'swap_battery', 'confirm_drop_zone', 'hold'])

export interface HeldCheck {
  rule: string
  uid: string | null
  outcome: string
  detail: string
  codes: string[]
}

/** The checks that put an order in front of a person (or refused it): holds first, then failures
 *  and unverified, each with its codes. A check with no code is shown by its rule alone. */
export function heldChecks(checks: readonly { rule: string; uid: string | null; outcome: string; detail: string; codes?: string[] }[]): HeldCheck[] {
  const rank: Record<string, number> = { hold: 0, fail: 1, unverified: 2 }
  return checks
    .filter((c) => c.outcome in rank)
    .map((c) => ({ rule: c.rule, uid: c.uid, outcome: c.outcome, detail: c.detail, codes: c.codes ?? [] }))
    .sort((a, b) => rank[a.outcome] - rank[b.outcome])
}

/** The drop zone and the observation a confirm order's evidence names: `obs:<zone>:<ms>`. */
export function evidenceOf(ids: readonly string[] | undefined): { zone: string; atMs: number }[] {
  const out: { zone: string; atMs: number }[] = []
  for (const id of ids ?? []) {
    const m = /^obs:(.+):(\d+)$/.exec(id)
    if (m) out.push({ zone: m[1], atMs: Number(m[2]) })
  }
  return out
}

/** The look the dispatcher put in a confirm order's rationale: "the latest look: Clear (0 seen) at
 *  2026-11-25T07:10:13Z". */
export function lookInRationale(rationale: string | null | undefined): { state: string; count: number; at: string } | null {
  const m = /the latest look: (\w+) \((\d+) seen\) at (\S+)/.exec(rationale ?? '')
  return m ? { state: m[1].toLowerCase(), count: Number(m[2]), at: m[3] } : null
}
