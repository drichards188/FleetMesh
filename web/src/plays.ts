// Adopting and lifting a play (rules 8, docs/design/incidents.md): the person's acts, under their
// session (`POST /api/plays`, `POST /api/plays/{id}/lift`). The words are playWords.ts.
import { operatorFetch, operatorHeaders } from './auth'
import { getOperator } from './decisions'
import type { Play, PlayAsk } from './playWords'

async function post(path: string, body: unknown): Promise<unknown> {
  const res = await operatorFetch(path, { method: 'POST', headers: { 'content-type': 'application/json', ...operatorHeaders(getOperator()) }, body: JSON.stringify(body ?? {}) })
  const b = (await res.json().catch(() => ({}))) as { detail?: string }
  if (!res.ok) throw new Error(b.detail ?? `the bridge answered ${res.status}`)
  return b
}

/** The person adopts a play (their act, logged under their session). */
export const adoptPlay = (ask: PlayAsk) => post('/api/plays', ask) as Promise<Play>

/** The person lifts a play. */
export const liftPlay = (id: number) => post(`/api/plays/${id}/lift`, {})
