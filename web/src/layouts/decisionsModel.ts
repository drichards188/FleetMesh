// The decisions layout's own small rules (Decisions.tsx; bd epic murmur-iobd, direction 3 of the
// layout prototypes): which reference list lies over the map's right edge, and the words of the
// column's quiet state. Display only. Pure (no browser): decisionsModel.check.ts runs it under node.
/** The reference lists this layout opens on demand over the map's right edge: the Fleet list and
 *  the command log with the order form. */
export type RefId = 'fleet' | 'log' | 'parcels'

/** In the top bar's order. */
export const REF_IDS: readonly RefId[] = ['fleet', 'log', 'parcels']

export const REF_LABEL: Readonly<Record<RefId, string>> = { fleet: 'Fleet', log: 'Log', parcels: 'Parcels' }

/** Each button's title: what the list is for. */
export const REF_TITLE: Readonly<Record<RefId, string>> = {
  fleet: 'Fleet: each of ours, what it is doing, and any in trouble',
  log: 'Log: the orders sent and how each is going; with vehicles selected, the order form for them',
  parcels: 'Parcels: each parcel of a delivery run, its state, hub, destination, weight, due time and vehicle',
}

/** A reference button: shows its list in place of the one shown, or closes it when it is that one. */
export function toggleRef(shown: RefId | null, id: RefId): RefId | null {
  return shown === id ? null : id
}

/** What lies over the map's right edge, one at a time: a drawer of the dock's (any but the Inbox,
 *  which is this layout's column and never lies over the map), else the reference list shown,
 *  else nothing. */
export function overMap(dockOpen: string | null, ref: RefId | null): 'dock' | 'ref' | null {
  if (dockOpen != null && dockOpen !== 'inbox') return 'dock'
  return ref ? 'ref' : null
}

/** A line of the quiet glance: its number (shown as data) and the words after it, or words alone. */
export interface GlanceLine {
  n: number | null
  words: string
}

/** Ours, as the column's quiet state says it: "22 vehicles, none in trouble". */
export function oursLine(ours: number, trouble: number): GlanceLine {
  if (ours <= 0) return { n: null, words: 'None in the fleet yet' }
  const noun = ours === 1 ? 'vehicle' : 'vehicles'
  return { n: ours, words: trouble > 0 ? `${noun}, ${trouble} in trouble` : `${noun}, none in trouble` }
}
