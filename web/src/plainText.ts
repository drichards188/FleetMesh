// Engine and bridge free text in a person's words, formatted around it in the console. The engine's
// own strings stay exactly as logged (replay, fixtures and digests read them); the console shows
// them with callsigns for our vehicles' uids and times in words for epoch stamps. Only well-formed
// tokens are rewritten ("our vehicle 1", "t=1795533619054"); anything else passes through as it
// came. Pure: plainText.check.ts.

/** An epoch stamp as the engine (ms) or the bridge (s) writes it, in ms. */
export function epochMs(n: number): number {
  return n >= 1e11 ? n : n * 1000
}

export interface PlainContext {
  /** one of ours by its uid as the engine names it ("1" -> "DRONE 1") */
  nameOf: (uid: string) => string
  /** an epoch time (ms) in words, with the preposition the text had before it ("at", "since"),
   *  or null: "2 min 01 s ago", "at T+3:45" */
  when: (ms: number, prep: 'at' | 'since' | null) => string
}

/** an epoch stamp: "at t=1795533619054", "since t=1795533627.2" (nine digits or more: never a
 *  small number of seconds) */
const EPOCH = /\b(?:(at|since) )?t=(\d{9,}(?:\.\d+)?)/g
const OUR_VEHICLE = /\bour vehicle (\d+)\b/g

/** `text` with its epoch stamps and "our vehicle <uid>" in words. */
export function plainText(text: string, ctx: PlainContext): string {
  return text
    .replace(EPOCH, (_m, prep: string | undefined, n: string) => ctx.when(epochMs(Number(n)), (prep as 'at' | 'since' | undefined) ?? null))
    .replace(OUR_VEHICLE, (_m, uid: string) => ctx.nameOf(uid))
}



/** m:ss (h:mm:ss past an hour) of `s` seconds, never negative. */
export function clockText(s: number): string {
  const n = Math.max(0, Math.round(s))
  const h = Math.floor(n / 3600)
  const m = Math.floor((n % 3600) / 60)
  const ss = String(n % 60).padStart(2, '0')
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`
}

/** A `when` for PlainContext on a situation's clock: "at T+3:45" from its start (`zeroS`, epoch s). */
export function situationWhen(zeroS: number): PlainContext['when'] {
  return (ms, prep) => `${prep ?? 'at'} T+${clockText(ms / 1000 - zeroS)}`
}

/** A `when` for PlainContext relative to now: "2 min ago" (or "in 2 min" for a time ahead). */
export function agoWhen(nowMs: number): PlainContext['when'] {
  return (ms, prep) => {
    const s = Math.round((nowMs - ms) / 1000)
    const a = Math.abs(s)
    const span = a < 90 ? `${a} s` : `${Math.round(a / 60)} min`
    const rel = s >= 0 ? `${span} ago` : `in ${span}`
    return prep === 'since' ? `${rel}` : rel
  }
}
