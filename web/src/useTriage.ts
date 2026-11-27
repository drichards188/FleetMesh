import { useEffect, useState } from 'react'
import type { TriageState } from './officerDesk'

/** The officer's record (`GET /api/triage`), read every 3 s while shown. */
export function useTriage(): { state: TriageState | null; error: string | null } {
  const [state, setState] = useState<TriageState | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let stop = false
    let timer: number | undefined
    const load = async () => {
      try {
        const res = await fetch('/api/triage', { cache: 'no-store' })
        if (!res.ok) throw new Error(`the bridge answered ${res.status}`)
        const v = (await res.json()) as TriageState
        if (!stop) {
          setState(v)
          setError(null)
        }
      } catch (e) {
        if (!stop) setError((e as Error).message)
      }
      if (!stop) timer = window.setTimeout(load, 3000)
    }
    void load()
    return () => {
      stop = true
      window.clearTimeout(timer)
    }
  }, [])
  return { state, error }
}
