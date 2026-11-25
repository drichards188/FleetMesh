import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { emuCause } from './linkEmu'
import { exceptionOf, type ExceptionContext, NO_EXCEPTIONS } from './vehicleException'
import { type Reveal, scrollSpanIntoView, useFlash } from './reveal'
import { symbolSvg } from './symbols'
import './nodeLoss.css'
import './fleet.css'
import { AddToGroup } from './GroupsSection'
import { domainChoices, kindsOf, rangeBetween } from './commandModel'
import type { GroupsApi } from './useGroups'
import { doingNow, latestOrders, type OrderIntent } from './orderWords'
import { type BaseGroup, EMPTY_FLEET, type FleetDoc, type FleetFilter, type FleetItem, type FleetStateId, ITEM_H, layoutItems, NO_FILTER, organise, stateDef, STATE_IDS, STATE_WORD } from './fleetModel'
import type { Tasking, Vehicle } from './types'
import { callsign, isStale } from './types'

interface Props {
  vehicles: Vehicle[]
  /** the fleet by home base and state (`GET /api/fleet`, or the console's own coarser view) */
  fleet?: FleetDoc
  selected: Set<number>
  onToggle: (uid: number) => void
  /** right click: deselect this vehicle */
  onDeselect: (uid: number) => void
  /** row body click: show this vehicle's details */
  onDetails: (uid: number) => void
  detailsUid: number | null
  onSelect: (uids: number[]) => void
  /** vehicles just selected from outside the list: scroll to the first and mark it */
  reveal?: Reveal | null
  /** the orders' ops (a member on its order's active op is on station), and the intents they
   *  carry out: a row's words when the fleet view has none for it (orderWords.ts) */
  taskings?: Tasking[]
  intents?: readonly OrderIntent[]
  /** vehicles with an alert of their own (not folded into their group's): exceptions, listed on
   *  their own */
  alerted?: ReadonlySet<number>
  /** what each row's exception slot is made from (vehicleException.ts): rules 30's standings and
   *  the platform catalog's endurance */
  exceptions?: ExceptionContext
  /** the operator's own groups: the selection can be added to one from here */
  groups?: GroupsApi
}

interface RowProps {
  v: Vehicle
  selected: boolean
  open: boolean
  flash: boolean
  onToggle: (uid: number, shift: boolean) => void
  onDeselect: Props['onDeselect']
  onDetails: Props['onDetails']
  /** what its exception slot is made from (vehicleException.ts) */
  exc: ExceptionContext
  /** what it is doing: its state's why from the fleet view, or the words of its order */
  doing: string
}

/** Two controls: the left strip selects the vehicle for tasking; the rest opens its details (what
 *  it is, its height, speed and link numbers are there, one click away). Two lines: its callsign and
 *  its one exception (vehicleException.ts, the only colour on the row: silent, link down,
 *  no GNSS, low battery), then what it is doing. */
const Row = memo(function Row({ v, selected, open, flash, onToggle, onDeselect, onDetails, exc, doing }: RowProps) {
  const stale = isStale(v)
  const x = exceptionOf(v, exc, emuCause)
  return (
    <div
      className={`vrow ${selected ? 'sel' : ''} ${stale ? 'stale' : ''} ${open ? 'open' : ''} ${flash ? 'flash' : ''}`}
      onContextMenu={(e) => {
        e.preventDefault()
        onDeselect(v.uid)
      }}
    >
      <button
        className="vrow-select"
        aria-pressed={selected}
        aria-label={`${selected ? 'Deselect' : 'Select'} ${callsign(v)} for tasking`}
        title={selected ? 'Selected for tasking (click or right-click to deselect; shift-click selects a range)' : 'Select for tasking (shift-click selects a range)'}
        onClick={(e) => onToggle(v.uid, e.shiftKey)}
      >
        <span className="vrow-box" aria-hidden>
          {selected ? '✓' : ''}
        </span>
      </button>
      <button className="vrow-body" aria-expanded={open} title="Show details" onClick={() => onDetails(v.uid)}>
        <span className="vrow-sym" dangerouslySetInnerHTML={{ __html: symbolSvg(v, { stale, size: 16 }).svg }} />
        <span className="vrow-main">
          <span className="vrow-name">
            <span className="vrow-call">{callsign(v)}</span>
            {x && (
              <span className={`vrow-exc ${x.tone}`} title={x.title} data-exception={x.kind}>
                {x.text}
              </span>
            )}
          </span>
          <span className="vrow-mode" title={doing}>
            {doing}
          </span>
        </span>
      </button>
    </div>
  )
})

/** A base's counts by state, problems first: one chip each, coloured by state. */
function StateChips({ counts, doc }: { counts: BaseGroup['counts']; doc: FleetDoc }) {
  return (
    <span className="fb-chips">
      {STATE_IDS.filter((s) => (counts[s] ?? 0) > 0).map((s) => (
        <span key={s} className={`fb-chip fs-${s}`} title={`${counts[s]} ${stateDef(doc, s).label.toLowerCase()}: ${stateDef(doc, s).means}`}>
          {counts[s]} {STATE_WORD[s] ?? s}
        </span>
      ))}
    </span>
  )
}

const OVERSCAN_PX = 400

/** first item whose bottom is below y (items are sorted by top) */
function firstBelow(items: FleetItem[], y: number) {
  let lo = 0
  let hi = items.length
  while (lo < hi) {
    const mid = (lo + hi) >> 1
    const it = items[mid]
    if (it.top + ITEM_H[it.kind] <= y) lo = mid + 1
    else hi = mid
  }
  return lo
}

/** The Fleet list, organised as delivery operators organise a fleet (docs/design/
 *  fleet-organisation.md): what needs the person first, whatever its base; then each site and its
 *  bases, each base with its vehicles' counts by state and, opened, its vehicles by state. Filters
 *  by type and by state. Windowed: every item has a fixed height (fleetModel.ts ITEM_H, kept in
 *  step with fleet.css), so only the items in view are in the DOM, whatever the fleet size. */
export function FleetPanel({ vehicles, fleet = EMPTY_FLEET, selected, onToggle, onDeselect, onDetails, detailsUid, onSelect, reveal = null, taskings, intents, exceptions = NO_EXCEPTIONS, groups }: Props) {
  // the Select menu's choices: all, each domain present, each kind of platform, each base
  const domains = useMemo(() => domainChoices(vehicles), [vehicles])
  const kinds = useMemo(() => kindsOf(vehicles).map((k) => ({ ...k, id: `kind:${k.id}` })), [vehicles])
  const baseChoices = useMemo(() => {
    const present = new Set(vehicles.map((v) => v.uid))
    return fleet.bases
      .map((b) => ({ id: `base:${b.id}`, label: b.name || b.id, uids: fleet.vehicles.filter((f) => f.base === b.id && present.has(f.uid)).map((f) => f.uid) }))
      .filter((c) => c.uids.length > 0)
  }, [fleet, vehicles])
  const [filter, setFilter] = useState<FleetFilter>(NO_FILTER)
  // bases opened or closed by the person (the rest: closed, or open under a filter)
  const [open, setOpen] = useState<Map<string, boolean>>(() => new Map())
  const list = useRef<HTMLDivElement>(null)
  const [scroll, setScroll] = useState({ top: 0, height: 800 })
  // each vehicle's latest order, for a row the fleet view has no words for
  const orders = useMemo(() => latestOrders(taskings ?? [], new Map((intents ?? []).map((i) => [i.id, i]))), [taskings, intents])
  const organised = useMemo(() => organise(fleet, vehicles, filter), [fleet, vehicles, filter])
  // the state filter's counts: over the type filter alone
  const stateCounts = useMemo(() => (filter.state == null ? organised.counts : organise(fleet, vehicles, { ...filter, state: null }).counts), [fleet, vehicles, filter, organised])
  const { items, height } = useMemo(() => layoutItems(organised, open, filter), [organised, open, filter])
  const fleetBy = useMemo(() => new Map(fleet.vehicles.map((f) => [f.uid, f])), [fleet])
  const baseName = useMemo(() => new Map(fleet.bases.map((b) => [b.id, b.name || b.id])), [fleet])
  // the list element exists only once there are vehicles
  const hasList = vehicles.length > 0

  // the first revealed vehicle in list order; scroll just enough to show it
  const itemsRef = useRef(items)
  itemsRef.current = items
  const firstRevealed = (ids: string[]) => {
    const want = new Set(ids)
    const it = itemsRef.current.find((i) => i.kind === 'row' && want.has(String(i.v.uid)))
    return it && it.kind === 'row' ? String(it.v.uid) : null
  }
  const flash = useFlash(reveal, firstRevealed)

  // the rows take these, the same functions every render, so that a row whose vehicle did not
  // change is not rendered again; they call the latest callbacks (murmur-4xq3)
  const latest = useRef({ onToggle, onDeselect, onDetails, onSelect, selected })
  latest.current = { onToggle, onDeselect, onDetails, onSelect, selected }
  // shift-click: every vehicle from the last one toggled to this one, in the order the list shows
  // them (the rows listed now: a closed base's vehicles are not in the range)
  const anchor = useRef<number | null>(null)
  const act = useMemo(
    () => ({
      onToggle: (uid: number, shift: boolean) => {
        if (shift && anchor.current != null) {
          const order = [...new Set(itemsRef.current.flatMap((i) => (i.kind === 'row' ? [i.v.uid] : [])))]
          const range = rangeBetween(order, anchor.current, uid)
          latest.current.onSelect([...new Set([...latest.current.selected, ...range])])
        } else latest.current.onToggle(uid)
        anchor.current = uid
      },
      onDeselect: (uid: number) => latest.current.onDeselect(uid),
      onDetails: (uid: number) => latest.current.onDetails(uid),
    }),
    [],
  )
  // a vehicle revealed from outside (the map, an alert): its base opens if it is closed, then the
  // list scrolls to its row
  const handled = useRef<Reveal | null>(null)
  useEffect(() => {
    const el = list.current
    if (!el || !reveal || handled.current === reveal) return
    const id = firstRevealed(reveal.ids)
    const it = itemsRef.current.find((i) => i.kind === 'row' && String(i.v.uid) === id)
    if (it) {
      handled.current = reveal
      scrollSpanIntoView(el, it.top, ITEM_H.row)
      return
    }
    const base = reveal.ids.map((u) => fleetBy.get(Number(u))?.base).find((b) => b != null)
    if (base != null && !open.get(base)) setOpen((m) => new Map(m).set(base, true))
    else handled.current = reveal
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reveal, items])

  useEffect(() => {
    const el = list.current
    if (!el) return
    let raf = 0
    const read = () => {
      raf = 0
      setScroll((s) => (s.top === el.scrollTop && s.height === el.clientHeight ? s : { top: el.scrollTop, height: el.clientHeight }))
    }
    const onScroll = () => {
      if (!raf) raf = requestAnimationFrame(read)
    }
    read()
    el.addEventListener('scroll', onScroll, { passive: true })
    const ro = new ResizeObserver(onScroll)
    ro.observe(el)
    return () => {
      el.removeEventListener('scroll', onScroll)
      ro.disconnect()
      cancelAnimationFrame(raf)
    }
  }, [hasList])

  const from = firstBelow(items, scroll.top - OVERSCAN_PX)
  const shown: FleetItem[] = []
  for (let i = from; i < items.length && items[i].top < scroll.top + scroll.height + OVERSCAN_PX; i++) shown.push(items[i])

  const toggleType = (t: string) =>
    setFilter((f) => {
      const types = new Set(f.types)
      if (types.has(t)) types.delete(t)
      else types.add(t)
      return { ...f, types }
    })
  const doing = (v: Vehicle, section: 'problem' | 'base') => {
    const f = fleetBy.get(v.uid)
    const words = f?.why || doingNow(v, orders.get(v.uid), { silent: isStale(v) })
    if (section !== 'problem' || !f) return words
    // listed apart from its base: where it belongs, after why
    const where = f.base != null ? baseName.get(f.base) : null
    return where ? `${words} · ${where}` : words
  }

  return (
    <aside className="panel fleet" data-tutorial="fleet">
      <header className="panel-head">
        <h2>Fleet</h2>
        {domains.length > 0 && (
          <select
            className="fleet-pick"
            aria-label="Select vehicles for tasking: all, a domain, a kind or a base"
            value=""
            onChange={(e) => {
              const c = [...domains, ...kinds, ...baseChoices].find((x) => x.id === e.target.value)
              if (c) onSelect(c.uids)
            }}
          >
            <option value="">Select...</option>
            <optgroup label="Domain">
              {domains.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.label} ({c.uids.length})
                </option>
              ))}
            </optgroup>
            {kinds.length > 1 && (
              <optgroup label="Kind">
                {kinds.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.label} ({c.uids.length})
                  </option>
                ))}
              </optgroup>
            )}
            {baseChoices.length > 1 && (
              <optgroup label="Base">
                {baseChoices.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.label} ({c.uids.length})
                  </option>
                ))}
              </optgroup>
            )}
          </select>
        )}
      </header>
      {vehicles.length > 0 && (
        <div className="fleet-filters" role="group" aria-label="Filter the fleet list">
          <div className="ff-types">
            <button className={`ff-chip ${filter.types.size === 0 ? 'on' : ''}`} aria-pressed={filter.types.size === 0} onClick={() => setFilter((f) => ({ ...f, types: new Set() }))}>
              All
            </button>
            {organised.types.length > 1 &&
              organised.types.map((t) => (
                <button key={t.id} className={`ff-chip ${filter.types.has(t.id) ? 'on' : ''}`} aria-pressed={filter.types.has(t.id)} onClick={() => toggleType(t.id)} title={`Show only the ${t.id}s (with any other type picked)`}>
                  {t.id} <span>{t.n}</span>
                </button>
              ))}
          </div>
          <select
            className="ff-state"
            aria-label="Show one state"
            value={filter.state ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, state: (e.target.value || null) as FleetStateId | null }))}
          >
            <option value="">All states</option>
            {STATE_IDS.filter((s) => (stateCounts[s] ?? 0) > 0 || filter.state === s).map((s) => (
              <option key={s} value={s}>
                {stateDef(fleet, s).label} ({stateCounts[s] ?? 0})
              </option>
            ))}
          </select>
        </div>
      )}
      {fleet.derived && (
        <p className="fleet-derived muted" title="The bridge running has no fleet view (/api/fleet): bases are where each vehicle's autopilot home is, states come from its link, its order and where it is. A restarted bridge gives the dispatcher's states.">
          Coarse states: this bridge has no fleet view yet
        </p>
      )}
      {selected.size > 0 && (
        <div className="fleet-sel" data-testid="selection">
          <span aria-live="polite">
            <b>{selected.size}</b> selected
          </span>
          <button className="ghost" onClick={() => onSelect([])} title="Clear the selection (Escape)">
            Clear
          </button>
        </div>
      )}
      {groups && <AddToGroup api={groups} selected={selected} />}
      {vehicles.length === 0 ? (
        <p className="empty">Waiting for vehicles</p>
      ) : items.length === 0 ? (
        <p className="empty">None match the filter</p>
      ) : (
        <div className="vlist fleet-list" ref={list}>
          <div className="vlist-inner" style={{ height }}>
            {shown.map((it) => {
              switch (it.kind) {
                case 'head':
                  return (
                    <h3 key={it.key} className={`vlist-head ${it.key === 'h:problems' ? 'fl-needs' : ''}`} style={{ top: it.top }} title={it.title}>
                      {it.title} {it.count != null && <span>{it.count}</span>}
                    </h3>
                  )
                case 'base':
                  return (
                    <div key={it.key} className="vlist-item fl-base-item" style={{ top: it.top, height: ITEM_H.base }}>
                      <button
                        className={`fl-base ${it.open ? 'open' : ''}`}
                        aria-expanded={it.open}
                        title={`${it.open ? 'Close' : 'Open'} ${it.base.name}: its vehicles by state`}
                        onClick={() => setOpen((m) => new Map(m).set(it.base.id, !it.open))}
                      >
                        <span className="fl-base-line">
                          <span className="fl-chev" aria-hidden>
                            {it.open ? '▾' : '▸'}
                          </span>
                          <b className="fl-base-name">{it.base.name}</b>
                          <span className="fl-base-n">{it.base.total}</span>
                        </span>
                        <StateChips counts={it.base.counts} doc={fleet} />
                      </button>
                    </div>
                  )
                case 'state':
                  return (
                    <div key={it.key} className={`fl-state fs-${it.state}`} style={{ top: it.top, height: ITEM_H.state }} title={stateDef(fleet, it.state).means}>
                      <span className="fl-dot" aria-hidden />
                      {stateDef(fleet, it.state).label} <span>{it.count}</span>
                    </div>
                  )
                case 'row':
                  return (
                    <div key={it.key} className={`vlist-item ${it.section === 'base' ? 'fl-in-base' : ''}`} style={{ top: it.top }}>
                      <Row
                        v={it.v}
                        selected={selected.has(it.v.uid)}
                        open={detailsUid === it.v.uid}
                        flash={flash === String(it.v.uid)}
                        onToggle={act.onToggle}
                        onDeselect={act.onDeselect}
                        onDetails={act.onDetails}
                        exc={exceptions}
                        doing={doing(it.v, it.section)}
                      />
                    </div>
                  )
              }
            })}
          </div>
        </div>
      )}
    </aside>
  )
}
