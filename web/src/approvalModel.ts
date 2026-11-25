// What an approval card says beside its ask (murmur-i9zw, ledger 2 section 3): what each vehicle
// does now and what the order will do (Now / After / Holds), and the catch (the battery reserve
// with the check's own assumption, and the unusual reasons in one callout). Built from the order,
// its checks, its policy facts and the fleet's latest orders; a row with no data behind it is left
// out, never filled from the proposer's text. Display only. Pure: approvalModel.check.ts.

/** The battery reserve check's figures, from its detail: "9 min left after task (task 19 min,
 *  battery 73% of 39 min), reserve 8 min (20%)". */
export interface Reserve {
  leftMin: number
  taskMin: number
  batteryPct: number
  enduranceMin: number
  reserveMin: number
  reservePct: number
}

const RESERVE = /(-?\d+) min left after task \(task (\d+) min, battery (\d+)% of (\d+) min\), reserve (\d+) min \((\d+)%\)/

export function parseReserve(detail: string): Reserve | null {
  const m = RESERVE.exec(detail)
  if (!m) return null
  const [, left, task, pct, of, reserve, rpct] = m
  return { leftMin: +left, taskMin: +task, batteryPct: +pct, enduranceMin: +of, reserveMin: +reserve, reservePct: +rpct }
}

/** The margin under which the battery is "close": the minutes left after the task beyond the
 *  reserve (a green "every one passes" once hid 9 min left against an 8 min reserve). */
export const CLOSE_MARGIN_MIN = 2

export interface ReserveLine {
  text: string
  close: boolean
  /** the tightest one's margin, minutes */
  margin: number
  /** how many members' reserve checks were read */
  of: number
}

/** The tightest of the reserve checks (`checks` as an order carries them, one per vehicle): its
 *  line says the assumption, the task's minutes, so a thin margin is never hidden. */
export function reserveLine(checks: readonly { rule: string; detail: string }[]): ReserveLine | null {
  const rs = checks.filter((c) => c.rule === 'reserves').map((c) => parseReserve(c.detail)).filter((r): r is Reserve => r != null)
  if (!rs.length) return null
  const tight = rs.reduce((a, b) => (b.leftMin - b.reserveMin < a.leftMin - a.reserveMin ? b : a))
  const margin = tight.leftMin - tight.reserveMin
  const close = margin <= CLOSE_MARGIN_MIN
  const who = rs.length > 1 ? 'The tightest of them has' : 'It has'
  const text = `Battery: ${who} ${tight.leftMin} min left after the task against a ${tight.reserveMin} min reserve (assuming the task takes ${tight.taskMin} min, from ${tight.batteryPct}% of ${tight.enduranceMin} min)${close ? ': close' : ''}.`
  return { text, close, margin, of: rs.length }
}

/** The facts a policy decision records (`policy.facts`). */
export interface PolicyFacts {
  leaves_approved_area?: boolean
  area_detail?: string
  unverified?: readonly string[]
}

/** The unusual reasons for a person to look, in one list: outside the approved area,
 *  unverified, you proposed it. */
export function callouts(o: { facts: PolicyFacts | undefined; extra: readonly string[]; selfApproval: boolean }): string[] {
  const out: string[] = []
  if (o.facts?.leaves_approved_area) out.push(`It leaves the approved area${o.facts.area_detail ? `: ${o.facts.area_detail}` : ''}.`)
  const unv = o.facts?.unverified ?? []
  if (unv.length) out.push(`Could not be verified: ${unv.join(', ')}. Judge it yourself.`)
  for (const e of o.extra) out.push(e)
  if (o.selfApproval) out.push('You proposed this: approving it is a self-approval, and the decision log records that.')
  return out
}

/** What the order does once sent, one sentence per kind. */
export function afterText(kind: string): string | null {
  switch (kind) {
    case 'orbit':
      return 'Each flies to the circle and orbits it until another order replaces it.'
    case 'patrol':
      return 'Each flies the route and loops it until another order replaces it.'
    case 'transit':
      return 'Each flies to the end of the route and holds there.'
    case 'hold':
      return 'Each holds where it is.'
    case 'rtl':
      return 'Each returns to its home.'
    case 'pickup':
      return 'The vehicle goes to the site and the parcel is loaded.'
    case 'deliver':
      return 'The vehicle takes the parcel to the drop zone and releases it there.'
    case 'reroute':
      return 'The vehicle takes the new route in place of what is left of its old one.'
    case 'return_to_hub':
      return 'The vehicle goes to the hub and lands or docks.'
    case 'land_now':
      return 'The vehicle lands, or stops, as soon as it is safe.'
    case 'abort':
      return 'The order is stopped and the vehicle does what the abort says next. A released parcel cannot be taken back.'
    case 'swap_battery':
      return 'The hub swaps the battery of the vehicle on its pad.'
    case 'confirm_drop_zone':
      return 'Your approval says the drop zone is fit now: the delivery that waited for it can go. Rejecting it, or letting it expire, ends that delivery.'
    default:
      return null
  }
}

/** When it stops on its own: a group's minimum and whether it re-forms, a transit's end. Null when nothing ends it but another order. */
export function holdsText(o: { kind: string; group: boolean; min: number | null; reform: boolean }): string | null {
  if (o.group && o.min != null && o.reform) return `Stays together while ${o.min} or more are heard; below that it stops and asks you.`
  if (o.group && o.min != null) return `Does not re-form: a member that drops leaves a gap, and fewer than ${o.min} heard asks you.`
  if (o.kind === 'transit') return 'Ends at the last point.'
  return null
}

/** The latest order of each of the first few vehicles, as a "Now" row, or a count of what the many
 *  are doing ("6 patrolling, 2 holding"). `doing`: a vehicle's words (orderWords.ts). */
export function nowRows(uids: readonly string[], doing: (uid: string) => string, name: (uid: string) => string, few = 3): string[] {
  if (uids.length <= few) return uids.map((u) => `${name(u)}: ${doing(u)}`)
  const by = new Map<string, number>()
  for (const u of uids) by.set(doing(u), (by.get(doing(u)) ?? 0) + 1)
  return [...by.entries()].sort((a, b) => b[1] - a[1]).map(([w, n]) => `${n} ${w}`)
}
