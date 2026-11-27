import { type ReactNode, useEffect, useMemo, useState } from 'react'
import type { Alert, Check, Decisions, Intent } from './decisions'
import { ALERT_LABEL, ALERT_RESPONSE, alertTracks, TRAFFIC_ALERT_KINDS, useAlertMemory, useOperator, verdict } from './decisions'
import { Bluf } from './Bluf'
import { RefusalFold } from './RefusalFold'
import { alertParts } from './bluf'
import { leftText, soonestLeftS, urgentInbox } from './inboxBadge'
import { needsYou } from './needsYou'
import { alertRank, alertSort, inboxOrder, RANK } from './inboxOrder'
import { agoWhen, plainText } from './plainText'
import { RationaleText } from './Rationale'
import { reviewEntries } from './reviewEntries'
import { ApprovalCard } from './ApprovalCard'
import { OfficerDesk, OfficerLog, OfficerNote } from './OfficerNote'
import { IncidentCard, PlayInForce } from './IncidentCard'
import { cardKey } from './playWords'
import type { FleetBase } from './fleetModel'
import { askFleet, nameOf } from './approvalFleet'
import { Proposer } from './Proposer'
import { askSentence } from './approvalText'
import { doingWords, latestOrders, type OrderIntent } from './orderWords'
import { fmtWorldZ } from './scenarioClock'
import type { Tasking, Vehicle } from './types'
import './inbox.css'

export type InboxTab = 'inbox' | 'decisions' | 'officer'

/** What the operator is looking at: an intent (`i2`) or an alert (`link:3`). */
export type InboxSelection = { kind: 'intent'; id: number } | { kind: 'alert'; id: string } | null

interface Props {
  decisions: Decisions | null
  vehicles: Vehicle[]
  now: () => number
  tab: InboxTab
  setTab: (t: InboxTab) => void
  selection: InboxSelection
  /** fly to and highlight: the vehicles (bridge uids) and the intent whose preview to frame */
  onFocus: (sel: InboxSelection, uids: number[], intent: Intent | null) => void
  onClose: () => void
  /** a notice from elsewhere in the console to show with the inbox's own (once per `n`) */
  notice?: Notice | null
  /** bring a review item into view (the Situation panel's link), once per `n` */
  focus?: { id: string; n: number } | null
  /** the map's banners (App.tsx), shown under this drawer's header while it is open over the
   *  map's right edge, where they would otherwise sit over its top (bd murmur-76gv) */
  banners?: ReactNode
  /** an order's line saying what request it was proposed for (Awaiting.tsx's TurnCause), or nothing */
  cause?: (i: Intent) => ReactNode
  /** the task layer's orders, for what each vehicle does now (orderWords.ts) */
  taskings?: Tasking[]
  /** a traffic alert's link: show this track (selected, its details open, framed with the vehicle
   *  the alert is about, if any) */
  onTrack?: (trackId: string, uid: number | null) => void
  /** whether a track is in the traffic picture now (a gone one's link is greyed) */
  hasTrack?: (trackId: string) => boolean
  /** a track's name as the map gives it, or null when it is gone */
  trackName?: (trackId: string) => string | null
  /** a traffic picture alert's link: the traffic bar's feeds, opened */
  onPicture?: () => void
  /** the fleet's bases (their names, their wind): an incident's card reads its hub's */
  bases?: FleetBase[]
}

// ---- top bar button -------------------------------------------------------------------------

/** The Inbox's button: one number, what needs the person (needsYou.ts, the same count as the
 *  drawer's "Needs you" tab and list), red only when something urgent is in it and grey for the routine
 *  (inboxBadge.ts), and beside it the time left on the item that expires soonest once that is
 *  under a few minutes; its title says what it counts and what it leaves out. Each item counts
 *  once. */
export function InboxButton({ decisions, open, onClick, hiddenAlerts, now }: { decisions: Decisions | null; open: boolean; onClick: () => void; hiddenAlerts: Set<string>; now: () => number }) {
  const urgent = urgentInbox(decisions?.inbox, hiddenAlerts)
  const left = soonestLeftS(decisions?.inbox.pending ?? [], now())
  return (
    <button className={`ghost platforms-btn ix-btn ${open ? 'on' : ''}`} aria-pressed={open} onClick={onClick}  >
      Inbox
      {left != null && <span className={`ix-left${urgent ? ' urgent' : ''}`}>{leftText(left)}</span>}
    </button>
  )
}

// ---- helpers ----------------------------------------------------------------------------------

const OUTCOME_MARK: Record<Check['outcome'], string> = { pass: 'PASS', fail: 'FAIL', hold: 'HOLD', unverified: 'UNVERIFIED', not_evaluated: 'NOT EVALUATED' }

/** The browser-only caveat, on the Acknowledge button's title where a person who asks sees it. */
const ACK_TITLE = 'Kept in this browser only: not shared with other operators or consoles, and the bridge does not record it.'

// ---- an alert -----------------------------------------------------------------------------------

function AlertCard({
  alert: a,
  now,
  selected,
  muted,
  mem,
  onFocus,
  name,
  vehicle,
  onVehicle,
  onTrack,
  hasTrack,
  trackName,
  onPicture,
}: {
  alert: Alert
  now: () => number
  selected: boolean
  muted: boolean
  mem: ReturnType<typeof useAlertMemory>
  onFocus: () => void
  /** one of ours by its uid, for a detail that names one by uid */
  name: (uid: string) => string
  /** the alert's vehicle is in the fleet (a traffic picture alert has none) */
  vehicle: boolean
  /** select the alert's vehicle and fly to it */
  onVehicle?: () => void
  onTrack?: (trackId: string, uid: number | null) => void
  hasTrack?: (trackId: string) => boolean
  trackName?: (trackId: string) => string | null
  onPicture?: () => void
}) {
  const traffic = TRAFFIC_ALERT_KINDS.has(a.kind)
  const tracks = traffic ? alertTracks(a) : []
  // a traffic alert about no vehicle of ours: its track (by the map's name for it), or the picture
  // (none for the picture itself: its kind says it)
  const who = a.callsign ?? (vehicle || !traffic ? `vehicle ${a.uid}` : tracks.length ? (trackName?.(tracks[0]) ?? tracks[0]) : '')
  const age = Math.max(0, now() / 1000 - a.since_t)
  return (
    <article className={`ix-alert ${a.severity} ${selected ? 'sel' : ''} ${muted ? 'muted-card' : ''}`} tabIndex={0} onClick={onFocus} onKeyDown={(e) => e.key === 'Enter' && e.target === e.currentTarget && onFocus()}>
      <header className="ix-head">
        <strong className="ix-akind">{ALERT_LABEL[a.kind] ?? a.kind}</strong>
        {who && <span className="ix-who">{who}</span>}
        <span className="ix-age" title="since">since {fmtWorldZ(a.since_t * 1000)}</span>
      </header>
      <OfficerNote triage={a.triage} />
      {/* the consequence first, the bridge's detail with its epoch stamps in words under More */}
      <Bluf
        className="ix-adetail"
        parts={alertParts({ kind: a.kind, detail: plainText(a.detail, { nameOf: name, when: agoWhen(now()) }) }, `${Math.max(1, Math.round(age / 60))} min`, who)}
      />
      <IssueLines alert={a} />
      {a.intent_id != null && <p className="ix-link"><span className="ix-id">i{a.intent_id}</span></p>}
      <footer className="ix-actions" onClick={(e) => e.stopPropagation()}>
        {muted ? (
          <>
            <span className="muted ix-muted">acknowledged</span>
            <button className="ghost" onClick={() => mem.wake(a)}>
              Restore
            </button>
          </>
        ) : (
          <>
            {traffic ? (
              <>
                {tracks.map((id, k) => {
                  const there = hasTrack?.(id) ?? true
                  return (
                    <button key={id} className="ghost" disabled={!there || !onTrack} title={there ? `Select ${id} on the map and open its details` : `${id} is no longer in the traffic picture`} onClick={() => onTrack?.(id, vehicle ? a.uid : null)}>
                      {k === 0 ? 'Show track' : `Show ${id}`}
                    </button>
                  )
                })}
                {vehicle && (
                  <button className="ghost" onClick={onVehicle ?? onFocus} title="Select the vehicle and fly to it">
                    Show vehicle
                  </button>
                )}
                {a.kind === 'traffic_picture_degraded' && onPicture && (
                  <button className="ghost" onClick={onPicture} title="Open the traffic bar's feeds on the map: each feed and receiver, the silent ones first">
                    Show feeds
                  </button>
                )}
              </>
            ) : (
              <button className="ghost" onClick={onFocus}>
                Fly to
              </button>
            )}
            <span className="spacer" />
            <button className="ghost" title={ACK_TITLE} onClick={() => mem.ack(a)}>
              Acknowledge
            </button>
          </>
        )}
      </footer>
    </article>
  )
}

/** An issue's three parts: what happened, what automation already did, the recommended response
 *  (an issue automation raised says them itself; a derived alert's come from its kind). */
function IssueLines({ alert: a }: { alert: Alert }) {
  const r = a.issue ?? ALERT_RESPONSE[a.kind]
  if (!r) return null
  return (
    <dl className="ix-issue">
      {a.issue && (
        <>
          <dt>What happened</dt>
          <dd>{a.issue.problem}</dd>
        </>
      )}
      <dt>What automation did</dt>
      <dd>{r.did}</dd>
      <dt>Recommended</dt>
      <dd>{r.recommend}</dd>
    </dl>
  )
}

// ---- a group alert ------------------------------------------------------------------------------


// ---- decisions list -----------------------------------------------------------------------------

/** Who decided an order, in plain words (the route chip's replacement). */
function routeWords(i: Intent): { text: string; tone: string } {
  const v = verdict(i)
  const a = i.approval
  if (v.route === 'auto') return { text: 'sent at once', tone: 'auto' }
  if (v.route === 'rules') return { text: 'stopped by a check', tone: 'rules' }
  if (a) return a.by === i.source.id ? { text: `self-approved by ${a.by}`, tone: 'self' } : { text: `approved by ${a.by}`, tone: 'human' }
  if (v.decided === 'rejected') return { text: 'rejected by a person', tone: 'human' }
  if (v.decided === 'expired') return { text: 'expired undecided', tone: 'human' }
  return { text: 'waiting for a person', tone: 'human' }
}

function DecisionRow({ intent: i, vehicles }: { intent: Intent; vehicles: Vehicle[]; now: () => number }) {
  const [open, setOpen] = useState(false)
  const v = verdict(i)
  const t0 = i.created_t
  const approver = i.approval
  const route = routeWords(i)
  const ask = askSentence({ kind: i.kind, params: i.params as Record<string, unknown>, uids: i.uids, group: i.group }, askFleet(vehicles), true)
  return (
    <li className={`ix-dec ${open ? 'open' : ''}`}>
      <button className="ix-dec-row" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="ix-dec-top">
          <strong className="ix-dec-ask">{ask}</strong>
          <span className="ix-id">i{i.id}</span>
          <span className={`ix-state ${i.state}`}>{i.state.replace('_', ' ')}</span>
        </span>
        <span className="ix-dec-sub">
          <span className={`ix-route ${route.tone}`}>{route.text}</span>
          <Proposer source={i.source} />
          <span className="right">{fmtWorldZ(t0)}</span>
        </span>
      </button>
      {open && (
        <div className="ix-dec-body">
          {v.wait_s != null && <p className="ix-note-shown">A person decided it in {Math.round(v.wait_s)} s.</p>}
          {i.rationale && (
            <div className="ix-rationale">
              <RationaleText rationale={i.rationale} source={i.source} />
            </div>
          )}
          <ol className="ix-trail">
            {i.history.map((h, k) => (
              <li key={k}>
                <span className="ix-t">+{Math.round((h.t - t0) / 1000)} s</span>
                <b>{h.state.replace('_', ' ')}</b>
              </li>
            ))}
          </ol>
          {approver?.note && <p className="ix-note-shown">Note from {approver.by}: {approver.note}</p>}
          {i.uids.length > 1 && i.checks.some((c) => c.uid == null) && <RefusalFold checks={i.checks} uids={i.uids} vehicles={vehicles} />}
          {!(i.uids.length > 1 && i.checks.some((c) => c.uid == null)) && i.checks.some((c) => c.outcome !== 'pass') && (
            <ul className="ix-dec-checks">
              {i.checks
                .filter((c) => c.outcome !== 'pass')
                .map((c, k) => (
                  <li key={k} className={c.outcome}>
                    {OUTCOME_MARK[c.outcome]} {c.rule}: 
                  </li>
                ))}
            </ul>
          )}
        </div>
      )}
    </li>
  )
}

function DecisionsList({ intents, vehicles, now }: { intents: Intent[]; vehicles: Vehicle[]; now: () => number }) {
  if (!intents.length) return <p className="empty">No orders yet.</p>
  return (
    <ul className="ix-decs">
      {intents.map((i) => (
        <DecisionRow key={i.id} intent={i} vehicles={vehicles} now={now} />
      ))}
    </ul>
  )
}

// ---- the panel --------------------------------------------------------------------------------------

interface Notice {
  n: number
  text: string
  tone: 'ok' | 'bad'
}

const NO_INTENTS: Intent[] = []

/** An Inbox item with where it sorts (inboxOrder.ts) and its card. */
interface Item {
  key: string
  rank: number
  sort: number | null
  node: ReactNode
}

export function InboxPanel({ banners, decisions, vehicles, now, tab, setTab, selection, onFocus, onClose, notice, focus, mem, hidden, cause, taskings, onTrack, hasTrack, trackName, onPicture, bases }: Props & { mem: ReturnType<typeof useAlertMemory>; hidden: Set<string> }) {
  // a card chosen from outside this drawer (a line at the head of another, the task panel's link)
  // is brought into view; one clicked here already is (`nearest` leaves it be)
  const selId = selection?.kind === 'intent' ? selection.id : null
  useEffect(() => {
    if (selId != null) document.querySelector(`.ix [data-intent-id="${selId}"]`)?.scrollIntoView({ block: 'nearest' })
  }, [selId, tab])
  const [notices, setNotices] = useState<Notice[]>(notice ? [notice] : [])
  const [noticeSeen, setNoticeSeen] = useState(notice?.n ?? null)
  if (notice && notice.n !== noticeSeen) {
    setNoticeSeen(notice.n)
    setNotices((ns) => [...ns, notice])
  }
  const [operator] = useOperator()
  const pending = decisions?.inbox.pending ?? []
  const intents = decisions?.intents ?? NO_INTENTS
  // a member alert folded into a swarm's alert is shown inside that card, not on its own
  const alerts = (decisions?.inbox.alerts ?? []).filter((a) => !a.folded_into)
  // what needs the person, counted as the Inbox button counts it (needsYou.ts)
  const need = needsYou(decisions?.inbox, hidden)
  const name = (uid: string) => nameOf(vehicles, uid)
  const done = (text: string, tone: 'ok' | 'bad') => setNotices((ns) => [...ns, { n: Date.now() + Math.random(), text, tone }])

  useEffect(() => {
    if (!notices.length) return
    const id = window.setTimeout(() => setNotices((ns) => ns.slice(1)), 9000)
    return () => window.clearTimeout(id)
  }, [notices])

  // what each of ours is doing, in the words of its order
  const orders = useMemo(() => latestOrders(taskings ?? [], new Map<number, OrderIntent>(intents.map((i) => [i.id, i]))), [taskings, intents])
  const doing = (uid: string) => {
    const v = vehicles.find((x) => String(x.uid) === uid)
    return v ? doingWords(v, orders.get(v.uid)) : 'no report'
  }
  const uidsOf = (i: Intent) => i.uids.map(Number).filter((n) => Number.isFinite(n))
  const selectedIntent = (i: Intent) => selection?.kind === 'intent' && selection.id === i.id
  const left = (i: Intent) => (i.expires_t != null ? (i.expires_t - now()) / 1000 : null)

  const items: Item[] = []
  for (const i of pending) {
    items.push({
      key: `i${i.id}`,
      rank: RANK.approval,
      sort: left(i),
      node: (
        <ApprovalCard
          key={`i${i.id}`}
          intent={i}
          vehicles={vehicles}
          now={now}
          selected={selectedIntent(i)}
          onFocus={() => onFocus({ kind: 'intent', id: i.id }, uidsOf(i), i)}
          onDone={done}
          cause={cause?.(i)}
          doing={doing}
        />
      ),
    })
  }
  for (const e of reviewEntries({ review: decisions?.inbox.review, now, onDone: done, focus, nameOf: name, vehicles }, operator)) items.push(e)
  for (const a of alerts) {
    const muted = hidden.has(a.id)
    items.push({
      key: a.id,
      rank: muted ? RANK.acknowledged : alertRank(a.kind, a.severity),
      sort: alertSort(a.severity, a.since_t),
      node: (
        <AlertCard
          key={a.id}
          alert={a}
          now={now}
          selected={selection?.kind === 'alert' && selection.id === a.id}
          muted={muted}
          mem={mem}
          name={name}
          vehicle={typeof a.uid === 'number' && vehicles.some((v) => v.uid === a.uid)}
          onTrack={onTrack}
          hasTrack={hasTrack}
          trackName={trackName}
          onPicture={onPicture}
          onVehicle={() => onFocus({ kind: 'alert', id: a.id }, [a.uid], null)}
          onFocus={() => {
            // a traffic alert: the track with the vehicle; any other: its vehicle
            const t = TRAFFIC_ALERT_KINDS.has(a.kind) ? alertTracks(a)[0] : undefined
            const ours = typeof a.uid === 'number' && vehicles.some((v) => v.uid === a.uid)
            onFocus({ kind: 'alert', id: a.id }, ours || !TRAFFIC_ALERT_KINDS.has(a.kind) ? [a.uid] : [], null)
            if (t && onTrack && (hasTrack?.(t) ?? true)) onTrack(t, ours ? a.uid : null)
          }}
        />
      ),
    })
  }
  // incidents (docs/design/incidents.md): an incident's items fold into one card, which opens
  // them again; an item of another cause at its hub stays on its own
  const incidents = decisions?.inbox.incidents ?? []
  const playsAll = decisions?.inbox.plays ?? []
  const recs = decisions?.inbox.recommended_plays ?? []
  const hubName = (id: string) => bases?.find((b) => b.id === id)?.name || id
  const itemWords = (key: string) => {
    const i = pending.find((x) => `intent:${x.id}` === key)
    if (i) {
      const codes = (i.checks ?? []).filter((c) => c.outcome !== 'pass').flatMap((c) => c.codes ?? [])
      return `${name(String(i.uids[0] ?? ''))}'s ${i.kind.replaceAll('_', ' ')}${codes.length ? ` (${codes.join(', ')})` : ''}`
    }
    const a = alerts.find((x) => `alert:${x.id}` === key)
    return a ? `${a.kind.replaceAll('_', ' ')}, ${name(String(a.uid))}` : key
  }
  const byKey = new Map(items.map((x) => [x.key, x]))
  const folded = new Set<string>()
  const incidentCards: Item[] = []
  for (const inc of incidents) {
    // its cards fold in (the person's items of it); one with none of the person's yet (the
    // officer's, or settled) still says the pattern and offers its play
    const members = inc.items.map(cardKey).flatMap((k) => byKey.get(k) ?? [])
    for (const m of members) folded.add(m.key)
    const base = bases?.find((b) => b.id === inc.hub)
    incidentCards.push({
      key: `inc:${inc.id}`,
      rank: members.length ? Math.min(...members.map((m) => m.rank)) : RANK.alert,
      sort: members.reduce<number | null>((s, m) => (m.sort == null ? s : s == null ? m.sort : Math.min(s, m.sort)), null),
      node: (
        <IncidentCard
          key={`inc:${inc.id}`}
          incident={inc}
          hubName={hubName(inc.hub)}
          weather={base?.weather ?? null}
          plays={playsAll.filter((p) => p.hub === inc.hub)}
          rec={recs.find((r) => r.incident === inc.id) ?? null}
          members={members.map((m) => m.node)}
          others={inc.others.map(itemWords)}
          onDone={done}
        />
      ),
    })
  }
  const shownItems = [...items.filter((x) => !folded.has(x.key)), ...incidentCards]
  // a play in force whose incident has gone (its items withdrawn): said at the head, to be lifted
  const loosePlays = playsAll.filter((p) => !incidents.some((i) => i.hub === p.hub))
  const { needs, info } = inboxOrder(shownItems)
  // the officer's record names vehicles by uid, as text or a number
  const callsignOf = (uid: number | string) => nameOf(vehicles, String(uid))
  // rules 7: an AI watch officer is on this run (on watch now, or it settled something): its log
  const officerCounts = decisions?.inbox.counts.triage
  const officerRun = !!officerCounts && (officerCounts.on_watch || officerCounts.handled > 0 || officerCounts.with_officer > 0)
  // the policy's route, said once for every approval below (not on each card)
  const routes = [...new Set(pending.map((i) => i.policy?.reason).filter((r): r is string => !!r))]

  return (
    <aside className="ix" aria-label="Inbox">
      <header className="ix-top">
        <h2>Inbox</h2>
        <div className="seg">
          <button className={tab === 'inbox' ? 'on' : ''} onClick={() => setTab('inbox')} title={need.says}>
            Needs you{need.total ? ` (${need.total})` : ''}
          </button>
          <button className={tab === 'decisions' ? 'on' : ''} onClick={() => setTab('decisions')}>
            Decisions
          </button>
          {officerRun && (
            <button className={tab === 'officer' ? 'on' : ''} onClick={() => setTab('officer')} title="Every item the AI watch officer triaged: what it settled without you, and what it passed on">
              Officer log{officerCounts?.handled ? ` (${officerCounts.handled})` : ''}
            </button>
          )}
        </div>
        <button className="ghost close" onClick={onClose} aria-label="Close inbox">
          Close
        </button>
      </header>
      {banners && <div className="ix-banners">{banners}</div>}
      {notices.length > 0 && (
        <div className="ix-notices" role="status">
          {notices.map((n) => (
            <p key={n.n} className={n.tone} onClick={() => setNotices((ns) => ns.filter((x) => x !== n))}>
              {n.text}
            </p>
          ))}
        </div>
      )}

      <div className="ix-scroll">
        {tab === 'officer' ? (
          <OfficerLog />
        ) : tab === 'decisions' ? (
          <DecisionsList intents={intents} vehicles={vehicles} now={now} />
        ) : (
          <>
            {/* one list, by consequence, then time left: what asks a person first (keyed, so a card
                that moves keeps what it has open or typed) */}
            {routes.length > 0 && <p className="ix-route-note">{routes.join('; ')}</p>}
            {officerRun && <OfficerDesk now={now} nameOf={callsignOf} full={needs.length === 0} onLog={() => setTab('officer')} />}
            {loosePlays.map((p) => (
              <PlayInForce key={`play${p.id}`} play={p} hubName={hubName(p.hub)} onDone={done} />
            ))}
            <h3 className="ix-h">
              Needs you <span>{needs.length}</span>
            </h3>
            {needs.length === 0 && <p className="empty">Nothing needs you</p>}
            {needs.map((x) => x.node)}
            {info.length > 0 && (
              <>
                <h3 className="ix-h ix-h-quiet">
                  For information <span>{info.length}</span>
                </h3>
                {info.map((x) => x.node)}
              </>
            )}
          </>
        )}
      </div>
    </aside>
  )
}
