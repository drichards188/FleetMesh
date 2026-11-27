import { useEffect, useMemo, useState } from 'react'
import { dueText, failureText, filterCounts, filterParcels, isLate, PARCEL_FILTERS, parcelPlaceKey, parcelStateLabel, type ParcelFilter } from './delivery'
import type { DeliveryFeed } from './useDelivery'
import './delivery.css'

interface Props {
  feed: DeliveryFeed
  /** a vehicle's callsign by its uid as the parcels name it */
  nameOf: (uid: string) => string
  /** show a vehicle: its details open and the map on it */
  onVehicle: (uid: number) => void
  /** frame the map on a place (a parcel's hub or drop zone) */
  onPlace: (at: [number, number]) => void
  onClose?: () => void
  banners?: React.ReactNode
  /** only the parcels at one place (its key, delivery.ts parcelPlaceKey): the map's mark there was
   *  clicked */
  place?: string | null
  /** back to every parcel */
  onAllPlaces?: () => void
  /** the list is shown (true) or gone (false): a place asked for lasts while it is shown (keep it
   *  the same function across renders, a state setter) */
  onShown?: (shown: boolean) => void
}

/** A place by its key, in words: "At Crissy Field hub", "On DRONE 11", "Delivered at dz-wind-17". */
function placeWords(key: string, hubName: (id: string) => string, nameOf: (uid: string) => string): string {
  const [kind, id] = key.split(/:(.*)/s)
  if (kind === 'hub') return `At ${hubName(id)}`
  if (kind === 'vehicle') return `On ${nameOf(id)}`
  return `Delivered at ${id}`
}

/** The parcels of a delivery run: each one's state, origin hub, destination, weight, due time and
 *  vehicle, with filters for waiting, assigned, in transit, delivered and late. Read-only: a click
 *  shows a vehicle or a place and sends nothing. Opens from the top bar beside Fleet and Log, or
 *  from a parcel mark on the map with only that place's parcels. */
export function ParcelsPanel({ feed, nameOf, onVehicle, onPlace, onClose, banners, place = null, onAllPlaces, onShown }: Props) {
  useEffect(() => {
    onShown?.(true)
    return () => onShown?.(false)
  }, [onShown])
  const [filter, setFilter] = useState<ParcelFilter>('all')
  // another place's mark was clicked: all its parcels first, whatever filter was on
  const [placeShown, setPlaceShown] = useState(place)
  if (place !== placeShown) {
    setPlaceShown(place)
    setFilter('all')
  }
  const { parcels, places } = feed
  const elapsed = parcels.elapsed_s
  const here = useMemo(() => (place ? parcels.parcels.filter((p) => parcelPlaceKey(p) === place) : parcels.parcels), [parcels.parcels, place])
  const counts = useMemo(() => filterCounts(here, elapsed), [here, elapsed])
  const shown = useMemo(() => filterParcels(here, filter, elapsed), [here, filter, elapsed])
  const hubs = useMemo(() => new Map(places.hubs.map((h) => [h.id, h])), [places.hubs])
  const zones = useMemo(() => new Map(places.drop_zones.map((z) => [z.id, z])), [places.drop_zones])
  return (
    <aside className="panel parcels" aria-label="Parcels">
      <header className="panel-head">
        <h2>Parcels</h2>
        <span className="muted" title={parcels.scenario?.title ?? undefined}>
          {parcels.parcels.length ? `${counts.delivered} of ${counts.all} delivered` : feed.active ? 'none yet' : 'no delivery run'}
        </span>
        {onClose && (
          <button className="ghost mini" onClick={onClose} aria-label="Close parcels">
            ✕
          </button>
        )}
      </header>
      {banners}
      {place && (
        <div className="pc-place">
          <span>
            {placeWords(place, (id) => hubs.get(id)?.name ?? id, nameOf)}: <b>{here.length}</b> {here.length === 1 ? 'parcel' : 'parcels'}
          </span>
          {onAllPlaces && (
            <button className="link" onClick={onAllPlaces}>
              All parcels
            </button>
          )}
        </div>
      )}
      <div className="pc-filters" role="group" aria-label="Filter parcels">
        {PARCEL_FILTERS.map((f) => (
          <button key={f.key} className={`pc-filter${filter === f.key ? ' on' : ''}${f.key === 'late' && counts.late > 0 ? ' late' : ''}`} aria-pressed={filter === f.key} title={f.title} onClick={() => setFilter(f.key)}>
            {f.label} <b>{counts[f.key]}</b>
          </button>
        ))}
      </div>
      <div className="pc-list">
        {shown.length === 0 && <p className="empty">{place && !here.length ? 'No parcels here now' : parcels.parcels.length ? 'No parcels under this filter' : feed.active ? 'No parcels yet' : 'This run has no parcels: start a delivery scenario (make start SCENARIO=harbour-city)'}</p>}
        {shown.map((p) => {
          const late = isLate(p, elapsed)
          const hub = hubs.get(p.from)
          const zone = zones.get(p.to)
          return (
            <article key={p.id} className={`pc-card ${p.state}${late ? ' late' : ''}`} data-parcel={p.id}>
              <header>
                <b className="pc-id">{p.id}</b>
                <span className={`pc-state ${p.state}`}>{parcelStateLabel(p.state)}</span>
                {p.priority > 2 && <span className="pc-prio" title={`priority ${p.priority}`}>priority {p.priority}</span>}
                <span className={`pc-due${late ? ' late' : ''}`}>{dueText(p, elapsed)}</span>
              </header>
              <p className="pc-route">
                <button className="link" onClick={() => hub && onPlace(hub.at)} disabled={!hub} title="The origin hub">
                  {hub?.name ?? p.from}
                </button>
                <span aria-hidden="true"> → </span>
                <button className="link" onClick={() => zone && onPlace(zone.at)} disabled={!zone} title="The destination drop zone">
                  {p.to}
                </button>
                <span className="muted"> · {p.mass_kg} kg{p.class !== 'normal' ? ` · ${p.class}` : ''}</span>
              </p>
              <p className="pc-veh">
                {p.vehicle ? (
                  <>
                    Vehicle{' '}
                    <button className="link" onClick={() => onVehicle(Number(p.vehicle))}>
                      {nameOf(p.vehicle)}
                    </button>
                  </>
                ) : (
                  <span className="muted">No vehicle yet{p.tries > 0 ? ` (${p.tries} ${p.tries === 1 ? 'try' : 'tries'})` : ''}</span>
                )}
              </p>
              {p.state === 'failed' && p.failure && (
                <p className="pc-why" title={p.failure.intent ? `order i${p.failure.intent}` : undefined}>
                  Why: {failureText(p)}
                </p>
              )}
            </article>
          )
        })}
      </div>
    </aside>
  )
}
