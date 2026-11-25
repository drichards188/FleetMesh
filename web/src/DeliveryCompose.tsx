import { useEffect, useMemo, useRef, useState } from 'react'
import { type Intent, tierLabel } from './decisions'
import { ABORT_THEN, COMPOSE_KINDS, type ComposeKind, type ComposeSpec, compose, EMPTY_SPEC, labelOf, parcelChoices, RELEASES, REROUTE_CAUSES, stoppableFor } from './deliveryCompose'
import { reasonLabel } from './deliveryReasons'
import { routing } from './commandModel'
import type { Draft } from './MapView'
import { sendTask, TaskRefused, type TaskRequest } from './useBridge'
import { useDelivery } from './useDelivery'
import { useDeliveryPreview } from './useDeliveryPreview'
import type { Vehicle } from './types'
import { callsign } from './types'
import './delivery.css'

interface Props {
  vehicles: Vehicle[]
  selected: number[]
  draft: Draft
  setDraft: (d: Draft) => void
  intents?: Intent[]
  /** the draft's mode the task form had (given back when a reroute is left) */
  restore: Draft['mode']
  /** the order sent, for the task panel's own result card */
  onSent?: (intentId: number | null) => void
}

const OUTCOME_WORD: Record<string, string> = { hold: 'HOLD', fail: 'REFUSE', unverified: 'UNVERIFIED', not_evaluated: 'NOT EVALUATED' }

/** Compose a delivery order for the selected vehicles: pickup, deliver, reroute, return to hub, land
 *  now, abort, swap battery, with pickers for hubs, drop zones and parcels. The engine's preview
 *  (where policy would send it, and each check's outcome and reason codes) shows before anything is
 *  sent; sending goes through the console's own order path as the operator, so the engine checks the
 *  order and policy routes it like any other. Nothing here approves anything. */
export function DeliveryCompose({ vehicles, selected, draft, setDraft, intents, restore, onSent }: Props) {
  const feed = useDelivery()
  const [spec, setSpec] = useState<ComposeSpec>(EMPTY_SPEC)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [sent, setSent] = useState<string | null>(null)
  const only = selected.length === 1 ? selected[0] : null
  const s: ComposeSpec = { ...spec, uids: selected, route: spec.kind === 'reroute' ? draft.route : [] }
  const parcels = feed.parcels.parcels
  const stoppable = useMemo(() => stoppableFor((intents ?? []).map((i) => ({ id: i.id, kind: i.kind, state: i.state, uids: i.uids })), only), [intents, only])
  const world = useMemo(
    () => ({
      parcels: parcels.map((p) => ({ id: p.id, state: p.state, from: p.from, to: p.to })),
      hubs: feed.places.hubs.map((h) => ({ id: h.id, open: h.open })),
      dropZones: feed.places.drop_zones.map((z) => ({ id: z.id, closed: z.closed })),
      stoppable,
    }),
    [parcels, feed.places, stoppable],
  )
  const composed = compose(s, world)
  // a reroute's points come from the map's clicks, through the draft's transit mode; leaving it gives
  // the draft back as the task form had it
  const live = useRef({ draft, setDraft, restore })
  useEffect(() => {
    live.current = { draft, setDraft, restore }
  })
  useEffect(() => {
    if (spec.kind !== 'reroute') return
    live.current.setDraft({ ...live.current.draft, mode: 'transit', center: null, route: [] })
    return () => live.current.setDraft({ ...live.current.draft, mode: live.current.restore, route: [] })
  }, [spec.kind])
  const first = composed.requests[0] ?? null
  const preview = useDeliveryPreview(first as TaskRequest | null)
  const answer = preview.answer
  const route = answer ? routing(answer, tierLabel) : null
  const set = (patch: Partial<ComposeSpec>) => {
    setSent(null)
    setError(null)
    setSpec((p) => ({ ...p, ...patch }))
  }
  const choose = (kind: ComposeKind) => {
    // a parcel picked for another kind may not fit this one
    set({ kind, parcel: '', target: null })
  }
  const parcelList = parcelChoices(spec.kind, parcels, only)
  const vehicle = only != null ? vehicles.find((v) => v.uid === only) : undefined
  // a picked parcel names its own hub and drop zone when the person has not chosen
  const pickParcel = (id: string) => {
    const p = parcels.find((x) => x.id === id)
    set({ parcel: id, ...(spec.kind === 'pickup' && p ? { hub: p.from } : {}), ...(spec.kind === 'deliver' && p ? { dropZone: p.to } : {}) })
  }
  const send = async () => {
    if (composed.requests.length === 0) return
    setBusy(true)
    setError(null)
    setSent(null)
    let last: number | null = null
    let n = 0
    try {
      for (const r of composed.requests) {
        const a = await sendTask(r as TaskRequest)
        n++
        last = a.intent_id ?? null
      }
      setSent(`${n} ${labelOf(spec.kind).toLowerCase()} order${n === 1 ? '' : 's'} sent to the engine: it checks them and policy routes them (the Inbox shows any that wait for you).`)
      onSent?.(last)
      if (spec.kind === 'reroute') setDraft({ ...draft, route: [] })
    } catch (e) {
      if (e instanceof TaskRefused) setError(`Refused by the rules: ${e.message}`)
      else setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }
  const hint = COMPOSE_KINDS.find((k) => k.kind === spec.kind)?.hint
  const held = answer?.checks.filter((c) => c.outcome !== 'pass') ?? []
  return (
    <fieldset className="dc" aria-label="Delivery order">
      <div className="seg wide" role="group" aria-label="Delivery order kind">
        {COMPOSE_KINDS.map((k) => (
          <button key={k.kind} className={spec.kind === k.kind ? 'on' : ''} aria-pressed={spec.kind === k.kind} title={k.hint} onClick={() => choose(k.kind)}>
            {k.label}
          </button>
        ))}
      </div>
      <p className="hint">{hint}</p>
      {(spec.kind === 'pickup' || spec.kind === 'deliver') && (
        <label>
          Parcel
          <select value={spec.parcel} onChange={(e) => pickParcel(e.target.value)} aria-label="Parcel">
            <option value="">Choose a parcel</option>
            {parcelList.map((p) => (
              <option key={p.id} value={p.id}>
                {p.id} ({p.state})
              </option>
            ))}
          </select>
        </label>
      )}
      {(spec.kind === 'pickup' || spec.kind === 'return_to_hub' || spec.kind === 'swap_battery') && (
        <label>
          Hub{spec.kind === 'return_to_hub' ? ' (optional)' : ''}
          <select value={spec.hub} onChange={(e) => set({ hub: e.target.value })} aria-label="Hub">
            <option value="">{spec.kind === 'return_to_hub' ? "The vehicle's own way home" : 'Choose a hub'}</option>
            {feed.places.hubs.map((h) => (
              <option key={h.id} value={h.id}>
                {h.name} ({h.packs} packs{h.open ? '' : ', closed'})
              </option>
            ))}
          </select>
        </label>
      )}
      {spec.kind === 'deliver' && (
        <>
          <label>
            Drop zone
            <select value={spec.dropZone} onChange={(e) => set({ dropZone: e.target.value })} aria-label="Drop zone">
              <option value="">Choose a drop zone</option>
              {feed.places.drop_zones.map((z) => (
                <option key={z.id} value={z.id}>
                  {z.id} ({z.kind}, {z.surface}
                  {z.closed ? ', closed' : ''})
                </option>
              ))}
            </select>
          </label>
          <label>
            Release
            <select value={spec.release} onChange={(e) => set({ release: e.target.value as ComposeSpec['release'] })} aria-label="Release">
              <option value="">The engine&apos;s default</option>
              {RELEASES.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </label>
        </>
      )}
      {spec.kind === 'reroute' && (
        <>
          <div className="row">
            <span className="muted">
              {draft.route.length} point{draft.route.length === 1 ? '' : 's'} on the map
            </span>
            <button className="ghost" disabled={!draft.route.length} onClick={() => setDraft({ ...draft, route: draft.route.slice(0, -1) })}>
              Undo
            </button>
            <button className="ghost" disabled={!draft.route.length} onClick={() => setDraft({ ...draft, route: [] })}>
              Clear
            </button>
          </div>
          <label>
            Why
            <select value={spec.cause} onChange={(e) => set({ cause: e.target.value as ComposeSpec['cause'] })} aria-label="Why reroute">
              {REROUTE_CAUSES.map((c) => (
                <option key={c} value={c}>
                  {c.replace('_', ' ')}
                </option>
              ))}
            </select>
          </label>
        </>
      )}
      {spec.kind === 'abort' && (
        <>
          <label>
            Order to stop
            <select value={spec.target ?? ''} onChange={(e) => set({ target: e.target.value ? Number(e.target.value) : null })} aria-label="Order to stop">
              <option value="">{only == null ? 'Select one vehicle' : stoppable.length ? 'Choose an order' : 'None running on it'}</option>
              {stoppable.map((i) => (
                <option key={i.id} value={i.id}>
                  i{i.id} {i.kind.replace(/_/g, ' ')} ({i.state.replace(/_/g, ' ')})
                </option>
              ))}
            </select>
          </label>
          <label>
            Then
            <select value={spec.then} onChange={(e) => set({ then: e.target.value as ComposeSpec['then'] })} aria-label="Then">
              {ABORT_THEN.map((t) => (
                <option key={t} value={t}>
                  {t.replace(/_/g, ' ')}
                </option>
              ))}
            </select>
          </label>
        </>
      )}
      {spec.kind === 'swap_battery' && (
        <label>
          Pad (optional)
          <input type="text" value={spec.pad} onChange={(e) => set({ pad: e.target.value })} placeholder="the hub's own choice" aria-label="Pad" />
        </label>
      )}
      {composed.problems.length > 0 && (
        <ul className="dc-problems" aria-label="What is missing">
          {composed.problems.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      )}
      {first && (
        <div className="dc-preview" aria-label="The engine's preview" aria-live="polite">
          {preview.error && <span className="stop">{preview.error}</span>}
          {!answer && !preview.error && <span className="muted">checking...</span>}
          {answer && route && (
            <span className={route.tone === 'pass' ? 'go' : route.tone === 'wait' ? 'wait' : 'stop'}>
              <b>{route.tone === 'fail' ? 'The rules would refuse it' : route.tone === 'pass' ? 'It would go at once' : 'It would wait for you'}</b>
              {composed.requests.length > 1 && ` (previewed for ${callsign(vehicle ?? { uid: first.uids[0], kind: null })}; the same order is checked for each of the ${composed.requests.length})`}
            </span>
          )}
          {answer && route && <span className="muted">{route.text}</span>}
          {held.map((c, k) => (
            <span key={k}>
              <b>{OUTCOME_WORD[c.outcome] ?? c.outcome}</b> {c.rule.replace(/_/g, ' ')}
              {(c.codes ?? []).map((code) => (
                <code key={code} title={reasonLabel(code)}>
                  {code}
                </code>
              ))}
              <span className="muted"> {c.detail}</span>
            </span>
          ))}
        </div>
      )}
      <button className="primary" disabled={busy || composed.requests.length === 0} onClick={send} title="Sent as the operator: the engine checks it and policy routes it; it may wait in the Inbox for a person">
        {busy ? 'Sending...' : composed.requests.length > 1 ? `Send ${labelOf(spec.kind)} to ${composed.requests.length} vehicles` : `Send ${labelOf(spec.kind)}`}
        {route && !busy ? ` (${route.short})` : ''}
      </button>
      {error && <p className="error">{error}</p>}
      {sent && <p className="dc-sent">{sent}</p>}
    </fieldset>
  )
}

/** The composer under the task form, folded open by default in a delivery run. */
export function DeliveryOrders(props: Props) {
  const feed = useDelivery()
  const [open, setOpen] = useState<boolean | null>(null)
  const shown = open ?? feed.active
  return (
    <details className="dc-wrap" open={shown} onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}>
      <summary>Delivery orders</summary>
      {shown && <DeliveryCompose {...props} />}
    </details>
  )
}
