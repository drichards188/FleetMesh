import { type Check } from './decisions'
import { type Observation, observationText, observationTone, navWords, type NavTrust, payloadWords } from './delivery'
import { DELIVERY_KINDS, evidenceOf, heldChecks, inputRows, lookInRationale, reasonOf } from './deliveryReasons'
import { linkLabel } from './catalogLabels'
import type { Intent } from './decisions'
import type { Vehicle } from './types'
import { callsign } from './types'
import { useDelivery } from './useDelivery'
import './delivery.css'

/** Why a delivery order waits for a person, on its Inbox card: for each reason code a sentence in
 *  plain language and the inputs the rule used (wind and gust against the limit, battery against the
 *  reserve, the link prediction, the navigation state), and for a confirm-drop-zone order the drop
 *  zone and what was seen there. Display only: the buttons are the card's own. */
export function DeliveryHolds({ intent: i, vehicles }: { intent: Intent; vehicles: Vehicle[] }) {
  const feed = useDelivery()
  if (!DELIVERY_KINDS.has(i.kind)) return null
  const held = heldChecks(i.checks as Check[])
  const confirm = i.kind === 'confirm_drop_zone'
  if (!held.length && !confirm) return null
  const vehicle = (uid: string | null) => vehicles.find((v) => String(v.uid) === uid)
  const nameOf = (uid: string | null) => {
    const v = vehicle(uid)
    return v ? callsign(v) : uid ? `vehicle ${uid}` : null
  }
  return (
    <section className="dh" aria-label="Why it waits for you">
      {held.length > 0 && <span className="ix-label">{held.some((h) => h.outcome === 'hold') ? 'Why it waits for you' : 'What the rules found'}</span>}
      {held.map((c, k) => {
        const codes = c.codes.length ? c.codes : [null]
        const rows = inputRows(c.rule, c.detail)
        const v = vehicle(c.uid)
        return (
          <div key={k} className={`dh-item ${c.outcome}`}>
            {codes.map((code, n) => {
              const r = code ? reasonOf(code) : null
              return (
                <div key={n}>
                  <span className="dh-title">{r ? r.title : `${c.rule.replace(/_/g, ' ')}: ${c.outcome}`}</span>
                  {code && <code>{code}</code>}
                  {r && <p className="dh-plain">{r.plain}</p>}
                </div>
              )
            })}
            {rows.length > 0 && (
              <dl className="dh-inputs" aria-label="What the rule used">
                {rows.map((r, n) => (
                  <Row key={n} r={r} />
                ))}
              </dl>
            )}
            {c.rule === 'nav_integrity' && c.uid != null && <NavLine state={feed.nav.vehicles[c.uid]?.state ?? 'unknown'} />}
            {c.rule === 'lost_link' && v && <LinkLine v={v} />}
            {c.rule === 'payload' && v && <p className="dh-detail">Payload now: {payloadWords(v.payload).toLowerCase()}.</p>}
            <p className="dh-detail">
              {nameOf(c.uid) ? `${nameOf(c.uid)}: ` : ''}
              {c.detail}
            </p>
          </div>
        )
      })}
      {confirm && <DropZoneLook intent={i} zones={feed.places.drop_zones} />}
    </section>
  )
}

function Row({ r }: { r: ReturnType<typeof inputRows>[number] }) {
  return (
    <>
      <dt>{r.label}</dt>
      <dd className={r.tone}>
        {r.value}
        {r.against && <span className="against"> ({r.against})</span>}
      </dd>
    </>
  )
}

function NavLine({ state }: { state: NavTrust }) {
  return <p className="dh-detail">Navigation now: {navWords(state).toLowerCase()}.</p>
}

/** The vehicle's active link, from what the bridge reports of it. */
function LinkLine({ v }: { v: Vehicle }) {
  const e = v.link.emu
  const profile = e?.active_profile
  const on = e?.active_link === 'backup' ? 'the backup radio' : 'the main radio'
  return (
    <p className="dh-detail">
      Link now: {profile ? `${linkLabel(profile)} (${on})` : on}, {e?.state === 'down' ? 'down' : e?.state === 'degraded' ? 'degraded' : 'up'}
      {v.link.loss_pct != null ? `, ${v.link.loss_pct}% loss` : ''}.
    </p>
  )
}

/** A confirm order's drop zone: what it is and what was seen there (the evidence the dispatcher
 *  named, and the zone's latest look now). The person's approval says the zone is fit. */
function DropZoneLook({ intent: i, zones }: { intent: Intent; zones: { id: string; kind: string; surface: string; populated: boolean; closed: boolean; radius_m: number; observation: Observation | null }[] }) {
  const id = String(i.params.drop_zone ?? '')
  const z = zones.find((x) => x.id === id)
  const ev = evidenceOf(i.params.evidence as string[] | undefined)
  const said = lookInRationale(i.rationale)
  const tone = observationTone(z?.observation ?? null, z?.closed)
  return (
    <div className="dh-look" aria-label="The drop zone">
      <b>{id || 'a drop zone'}</b>
      {z && (
        <span className="muted">
          {' '}
          {z.kind}, {z.surface}, {z.radius_m} m{z.populated ? ', where people are' : ''}
          {z.closed ? ', closed' : ''}
        </span>
      )}
      {said && (
        <p>
          What the dispatcher saw: {said.state}
          {said.state === 'occupied' ? `, ${said.count} seen` : ''} (look at {said.at.replace('T', ' ').replace('Z', 'Z')}
          {ev.length ? `, ${ev[0].zone}` : ''}).
        </p>
      )}
      <p>
        Latest look now: <span className={tone}>{z ? observationText(z.observation) : 'the drop zone is not on the places list'}</span>.
      </p>
      <p className="muted">Approving says the zone is fit now and lets the delivery that waits for it go. Rejecting it ends that delivery.</p>
    </div>
  )
}
