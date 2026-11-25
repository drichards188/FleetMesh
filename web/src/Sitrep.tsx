import { useEffect, useMemo, useRef, useState } from 'react'
import { STATE_ORDER, STATE_WORDS, type LatestOp, type VehicleState } from './exceptions'
import type { OperatorGroup } from './groups'
import { ANY, filterNodes, nodeRows, windowOf, worstFirst, type NodeFilter, type NodeRow } from './sitrep'
import type { Tasking, Vehicle } from './types'
import './sitrep.css'

/** The Sitrep, at group scale only: each group's strength against its minimum, and every node with
 *  the filters that find trouble (state, unheard only, low battery), drilling to its details. The
 *  headline is the top bar's glance line; what waits for a person is in the Inbox. Assembled from the feeds the console already holds, once a second (never per
 *  frame); long lists are summarised and expanded on demand, and the node list is virtualised. It
 *  sends nothing and decides nothing: a row opens an existing panel, and the only buttons that
 *  change anything change what you look at (a selection, a filter). */

interface Props {
  vehicles: Vehicle[]
  taskings: Tasking[]
  groups: OperatorGroup[]
  /** open a vehicle's details */
  onVehicle: (uid: number) => void
  /** select these vehicles (the order form then commands them); a view change, no order */
  onSelect: (uids: number[]) => void
  onClose: () => void
  banners?: React.ReactNode
}

/** The latest of `value`, handed on once every `ms` (and at once the first time): the report is
 *  built from the ~1 Hz picture, not from each 4 Hz update. */
function useSnapshot<T>(value: T, ms: number): T {
  const [snap, setSnap] = useState(value)
  const latest = useRef(value)
  useEffect(() => {
    latest.current = value
  }, [value])
  useEffect(() => {
    const id = window.setInterval(() => setSnap(latest.current), ms)
    return () => window.clearInterval(id)
  }, [ms])
  return snap
}

const ROW_H = 26


export function Sitrep(p: Props) {
  // one coherent picture a second: the vehicles, the orders
  const vehicles = useSnapshot(p.vehicles, 1000)
  const taskings = useSnapshot(p.taskings, 1000)
  const groups = useSnapshot(p.groups, 1000)

  const [filter, setFilter] = useState<NodeFilter>(ANY)
  const [focus, setFocus] = useState<string | null>(null)
  const [scroll, setScroll] = useState(0)
  const list = useRef<HTMLDivElement>(null)

  const model = useMemo(() => {
    const latest = new Map<number, LatestOp>()
    for (const t of [...taskings].sort((a, b) => a.id - b.id)) for (const o of t.ops) latest.set(o.uid, { state: o.state, task: t.task })
    const ctx = { latest }
    const by = new Map(vehicles.map((v) => [v.uid, v]))
    const ofUid = new Map<number, string[]>()
    for (const g of groups) for (const u of g.members) ofUid.set(u, [...(ofUid.get(u) ?? []), g.name])
    const nodes = nodeRows(vehicles, ctx, ofUid)
    return { nodes, by }
  }, [vehicles, taskings, groups])

  const filtered = useMemo(() => worstFirst(filterNodes(model.nodes, { ...filter })), [model.nodes, filter])
  const view = windowOf(filtered.length, scroll, 300, ROW_H)


  return (
    <aside className="ix srp" aria-label="Situation report">
      <div className="ix-top">
        <h2>Sitrep</h2>
        <button className="ghost close" onClick={p.onClose} aria-label="Close the situation report">
          ×
        </button>
      </div>
      {p.banners && <div className="ix-banners">{p.banners}</div>}
      <div className="ix-scroll srp-body">

        <section aria-label="Nodes">
          <h3 className="ix-h">
            Nodes <span>{filtered.length}</span>
            <small>
              of {model.nodes.length}
            </small>
          </h3>
          <div className="srp-filters">
            <input type="search" placeholder="name" aria-label="Filter by name" value={filter.text} onChange={(e) => setFilter({ ...filter, text: e.target.value })} />
            <select aria-label="State" value={filter.state} onChange={(e) => setFilter({ ...filter, state: e.target.value as VehicleState | 'all' })}>
              <option value="all">any state</option>
              {STATE_ORDER.map((s) => (
                <option key={s} value={s}>
                  {STATE_WORDS[s]}
                </option>
              ))}
            </select>
            <label>
              <input type="checkbox" checked={filter.unheard} onChange={(e) => setFilter({ ...filter, unheard: e.target.checked })} /> unheard only
            </label>
            <label>
              <input type="checkbox" checked={filter.low} onChange={(e) => setFilter({ ...filter, low: e.target.checked })} /> low battery
            </label>
            {(focus || filter !== ANY) && (
              <button
                className="ghost mini"
                onClick={() => {
                  setFilter(ANY)
                  setFocus(null)
                }}
              >
                Clear
              </button>
            )}
          </div>
          <div className="srp-list" ref={list} onScroll={(e) => setScroll(e.currentTarget.scrollTop)} role="table" aria-label="Nodes" aria-rowcount={filtered.length}>
            <div style={{ height: filtered.length * ROW_H, position: 'relative' }}>
              {filtered.slice(view.from, view.to).map((r, k) => (
                <NodeLine key={r.uid} r={r} top={(view.from + k) * ROW_H} onOpen={p.onVehicle} />
              ))}
            </div>
          </div>
        </section>
      </div>
    </aside>
  )
}

function NodeLine({ r, top, onOpen }: { r: NodeRow; top: number; onOpen: (uid: number) => void }) {
  return (
    <button role="row" className={`srp-node ${r.state}`} style={{ top, height: ROW_H }} onClick={() => onOpen(r.uid)} title={r.groups.length ? `Open its details. In ${r.groups.join(', ')}` : 'Open its details'}>
      <b className="srp-nname">{r.name}</b>
      <span className={`srp-nstate ${r.state === 'lost' ? 'bad' : ''}`}>{STATE_WORDS[r.state]}</span>
      <span className={r.lowBattery ? 'warn' : 'dim'}>{r.battery == null ? '–' : `${Math.round(r.battery)}%`}</span>
      <span className="srp-norder">{r.order}</span>
    </button>
  )
}
