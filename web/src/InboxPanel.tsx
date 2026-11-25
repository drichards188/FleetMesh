import { type ReactNode, useEffect, useMemo, useState } from 'react'
import type { Alert, Check, Decisions, Intent } from './decisions'
import { ALERT_LABEL, useAlertMemory, useOperator, verdict } from './decisions'
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
import { askFleet, nameOf } from './approvalFleet'
import { Proposer } from './Proposer'
import { askSentence } from './approvalText'
import { doingWords, latestOrders, type OrderIntent } from './orderWords'
import { fmtWorldZ } from './scenarioClock'
import type { Tasking, Vehicle } from './types'
import './inbox.css'

export type InboxTab = 'inbox' | 'decisions'

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
}: {
  alert: Alert
  now: () => number
  selected: boolean
  muted: boolean
  mem: ReturnType<typeof useAlertMemory>
  onFocus: () => void
  /** one of ours by its uid, for a detail that names one by uid */
  name: (uid: string) => string
}) {
  const who = a.callsign ?? `vehicle ${a.uid}`
  const age = Math.max(0, now() / 1000 - a.since_t)
  return (
    <article className={`ix-alert ${a.severity} ${selected ? 'sel' : ''} ${muted ? 'muted-card' : ''}`} tabIndex={0} onClick={onFocus} onKeyDown={(e) => e.key === 'Enter' && e.target === e.currentTarget && onFocus()}>
      <header className="ix-head">
        <strong className="ix-akind">{ALERT_LABEL[a.kind] ?? a.kind}</strong>
        <span className="ix-who">{who}</span>
        <span className="ix-age" title="since">since {fmtWorldZ(a.since_t * 1000)}</span>
      </header>
      {/* the consequence first, the bridge's detail with its epoch stamps in words under More */}
      <Bluf
        className="ix-adetail"
        parts={alertParts({ kind: a.kind, detail: plainText(a.detail, { nameOf: name, when: agoWhen(now()) }) }, `${Math.max(1, Math.round(age / 60))} min`, who)}
      />
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
            <button className="ghost" onClick={onFocus}>
              Fly to
            </button>
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

export function InboxPanel({ banners, decisions, vehicles, now, tab, setTab, selection, onFocus, onClose, notice, focus, mem, hidden, cause, taskings }: Props & { mem: ReturnType<typeof useAlertMemory>; hidden: Set<string> }) {
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
      node: <AlertCard key={a.id} alert={a} now={now} selected={selection?.kind === 'alert' && selection.id === a.id} muted={muted} mem={mem} name={name} onFocus={() => onFocus({ kind: 'alert', id: a.id }, [a.uid], null)} />,
    })
  }
  const { needs, info } = inboxOrder(items)
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
        {tab === 'decisions' ? (
          <DecisionsList intents={intents} vehicles={vehicles} now={now} />
        ) : (
          <>
            {/* one list, by consequence, then time left: what asks a person first (keyed, so a card
                that moves keeps what it has open or typed) */}
            {routes.length > 0 && <p className="ix-route-note">{routes.join('; ')}</p>}
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
