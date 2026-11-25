import { useEffect, useState } from 'react'

/** The policy in force as the bridge serves it (`GET /api/policy`, read-only, no identity needed;
 *  docs/SCENARIO-SETTINGS.md): what MCP `get_policy` gives agents (`policy`, the effective `watch`),
 *  with the decision rules it runs under. The console's forms read their defaults here, never a
 *  copy of them. */
export interface PolicyAnswer {
  t: number
  rules_version: number
  /** rules 34: an order to many is a group order, and a transit can be ordered */
  group_orders?: boolean
  policy: unknown
  watch: Record<string, unknown> | null
}

export async function fetchPolicy(): Promise<PolicyAnswer> {
  const res = await fetch('/api/policy')
  const body = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(body.detail ?? `bridge returned ${res.status}`)
  return body as PolicyAnswer
}

/** Rules 34: whether this run decides group orders (an order to many laid out member by member,
 *  and the transit kind): read once; false until it comes or if it cannot be read (the order form
 *  then offers what older rules take). */
export function useGroupOrders(): boolean {
  const [on, setOn] = useState(false)
  useEffect(() => {
    let gone = false
    fetchPolicy()
      .then((p) => !gone && setOn(p.group_orders === true))
      .catch(() => {})
    return () => {
      gone = true
    }
  }, [])
  return on
}
