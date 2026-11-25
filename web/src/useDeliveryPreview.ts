import { useEffect, useState } from 'react'
import { operatorFetch, operatorHeaders } from './auth'
import { getOperator, type Check, type PolicyDecision } from './decisions'
import type { TaskRequest } from './useBridge'

/** `POST /api/intents/preview` for a delivery order: the engine's checks and the policy's routing now,
 *  nothing created or logged (the console's own dry run, DryRun.tsx, asks the same). */
export interface PreviewAnswer {
  checks: Check[]
  passed: boolean
  policy: PolicyDecision | null
  would_reach: string
  submission_authorizes?: boolean
}

export interface PreviewState {
  answer: PreviewAnswer | null
  loading: boolean
  error: string | null
}

const IDLE: PreviewState = { answer: null, loading: false, error: null }
const REFRESH_MS = 5000

/** The preview of the order as composed: asked shortly after it changes and again every few
 *  seconds while it stands (the world moves); a changed order drops the old answer. Information
 *  only: it never sends anything. */
export function useDeliveryPreview(req: TaskRequest | null): PreviewState {
  const key = req ? JSON.stringify(req) : null
  const [st, setSt] = useState<PreviewState & { key: string | null }>({ ...IDLE, key: null })
  useEffect(() => {
    if (!key) return
    let ctl: AbortController | null = null
    let timer = 0
    let gone = false
    const ask = async () => {
      ctl = new AbortController()
      setSt((s) => ({ ...(s.key === key ? s : IDLE), key, loading: true }))
      try {
        const res = await operatorFetch('/api/intents/preview', {
          method: 'POST',
          headers: { 'content-type': 'application/json', ...operatorHeaders(getOperator()) },
          body: key,
          signal: ctl.signal,
        })
        const body = await res.json().catch(() => ({}))
        if (!res.ok) throw new Error(body.detail ?? `bridge returned ${res.status}`)
        if (!gone) setSt({ key, answer: body as PreviewAnswer, loading: false, error: null })
      } catch (e) {
        if (!gone && (e as Error).name !== 'AbortError') setSt((s) => ({ ...(s.key === key ? s : IDLE), key, loading: false, error: (e as Error).message }))
      }
      if (!gone) timer = window.setTimeout(ask, REFRESH_MS)
    }
    timer = window.setTimeout(ask, 350)
    return () => {
      gone = true
      ctl?.abort()
      window.clearTimeout(timer)
    }
  }, [key])
  return key && st.key === key ? st : IDLE
}
