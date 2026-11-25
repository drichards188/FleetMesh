// The link emulator's view of one vehicle's link (crates/fleetmesh-linkemu), served by the bridge
// at /api/links/{uid} (crates/fleetmesh-bridge/src/emu.rs). The fleet row only needs the brief
// that rides on each vehicle's link (`link.emu`); the details card polls the full view.

import { useEffect, useState } from 'react'
import type { ActiveLink } from './types'

export interface EmuDir {
  pkts: number
  dropped: number
  loss: number | null
  by_cause: Record<string, number>
  /** mean one-way delay the emulator added, ms */
  latency_ms: number | null
}

export interface EmuSatcom {
  class: string
  /** `static`: the profile's own figures (no satellite availability model in this build) */
  mode: 'pending' | 'live' | 'static'
  static_reason: string | null
  up: boolean | null
  latency_ms: number
  latency_source: 'space' | 'profile'
  handovers: number
}

export interface EmuLink {
  uid: number
  class: string
  /** the main link's profile */
  profile: string
  /** which radio carries the traffic now: the main link, or the backup after an automatic failover */
  active_link?: ActiveLink
  /** the profile carrying the traffic now: `profile` on the main link, `backup_profile` on the backup */
  active_profile?: string
  /** the backup radio's profile; null or absent where the vehicle has none */
  backup_profile?: string | null
  /** how many times the vehicle has switched between its radios */
  link_switches?: number
  /** seconds spent on each radio so far */
  time_on_main_s?: number
  time_on_backup_s?: number
  state: 'ok' | 'degraded' | 'down'
  cause: string | null
  blocked: string | null
  events: string[]
  /** the ground station carrying the link now (several are networked: the best path's), its
   *  slant range and how often the link moved between stations; null where the profile uses
   *  none (SATCOM) */
  station?: string | null
  station_range_m?: number | null
  station_switches?: number
  los: { state: 'off' | 'unknown' | 'clear' | 'obstructed'; obstruction_m: number | null } | null
  satcom: EmuSatcom | null
  nominal: {
    loss: number
    latency_ms: number
    jitter_ms: number
    bandwidth_bps: number
    propagation_mps: number | null
    terrain: boolean
  } | null
  recent: { window_s: number; up: EmuDir; down: EmuDir }
  total: { up: EmuDir; down: EmuDir }
  emu_rtt_ms: number | null
}

export interface EmuAnswer {
  enabled: boolean
  ok: boolean
  age_s: number | null
  error: string | null
  link?: EmuLink
  detail?: string
}

/** What the emulator is doing to a link, in words. */
export function emuCause(cause: string | null | undefined): string {
  switch (cause) {
    case null:
    case undefined:
      return ''
    case 'loss':
      return 'channel loss'
    case 'event_loss':
    case 'event':
      return 'scripted event'
    case 'jam':
      return 'radio interference'
    case 'cut':
      return 'scripted cut'
    case 'range':
      return 'out of range'
    case 'terrain':
      return 'terrain blocks line of sight'
    case 'queue':
      return 'queue full (bandwidth)'
    case 'no_sat':
      return 'no satellite in view'
    case 'handover':
      return 'switching between the main and the backup link'
    case 'no_peer':
      return 'vehicle not heard yet'
    case 'socket':
      return 'socket error'
    default:
      return cause
  }
}

export function fmtDuration(s: number): string {
  if (s < 90) return `${Math.round(s)} s`
  if (s < 5400) return `${Math.round(s / 60)} min`
  return `${(s / 3600).toFixed(1)} h`
}

/** Polls /api/links/{uid} while mounted. null until the first answer. */
export function useEmuLink(uid: number, everyMs = 2000): EmuAnswer | null {
  const [ans, setAns] = useState<{ uid: number; a: EmuAnswer } | null>(null)
  useEffect(() => {
    let live = true
    const load = async () => {
      try {
        const res = await fetch(`/api/links/${uid}`)
        const body = (await res.json()) as EmuAnswer
        if (live) setAns({ uid, a: body })
      } catch {
        if (live) setAns({ uid, a: { enabled: false, ok: false, age_s: null, error: 'bridge unreachable' } })
      }
    }
    void load()
    const t = window.setInterval(load, everyMs)
    return () => {
      live = false
      window.clearInterval(t)
    }
  }, [uid, everyMs])
  return ans && ans.uid === uid ? ans.a : null
}
