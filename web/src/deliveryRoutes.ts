// Each running delivery's route, for the map: from the vehicle's position through the waypoints the
// task layer is flying (the planner's `via`, then the order's own route) to where the order ends.
// A delivery order is flown as a transit with an `action` (pickup, deliver, home), so the route is
// the tasking's. Display only. Pure: deliveryRoutes.check.ts.
import type { LngLat } from './types.ts'

export type RouteAction = 'pickup' | 'deliver' | 'home' | 'other'

export interface RouteDraw {
  key: string
  uid: number
  action: RouteAction
  path: LngLat[]
  /** the parcel the order carries, if it names one */
  parcel?: string
}

interface TaskingLike {
  id: number
  task: string
  params: { route?: LngLat[]; via?: Record<string, LngLat[]>; action?: string; parcel?: string }
  ops: { uid: number; state: string }[]
}

interface VehicleLike {
  uid: number
  position: LngLat | null
}

/** An op is in play until it has ended. */
const ENDED = new Set(['done', 'failed', 'superseded'])

function actionOf(a: string | undefined): RouteAction {
  return a === 'pickup' || a === 'deliver' || a === 'home' ? a : 'other'
}

/** The route of every delivery tasking with an op in play, one per vehicle. Taskings without an
 *  `action` (an orbit, a plain transit) are not deliveries. */
export function deliveryRoutes(taskings: readonly TaskingLike[], vehicles: readonly VehicleLike[]): RouteDraw[] {
  const at = new Map(vehicles.map((v) => [v.uid, v.position]))
  const out: RouteDraw[] = []
  for (const t of taskings) {
    if (t.task !== 'transit' || !t.params.action) continue
    for (const op of t.ops) {
      if (ENDED.has(op.state)) continue
      const from = at.get(op.uid)
      const via = t.params.via?.[String(op.uid)] ?? []
      const route = t.params.route ?? []
      const path: LngLat[] = [...(from ? [from] : []), ...via, ...route]
      if (path.length < 2) continue
      out.push({ key: `${t.id}:${op.uid}`, uid: op.uid, action: actionOf(t.params.action), path, parcel: t.params.parcel })
    }
  }
  return out
}
