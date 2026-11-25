import { useEffect, useRef, useState } from 'react'
import { useAuth } from './auth'
import { fmtDuration, StepUpNeeded, stepsUp, useOperator, useOperators } from './decisions'
import { FLASH_MS, focusInView } from './reveal'
import { StepUpField } from './StepUp'
import { stepUpProof, stepUpReady, useStepUp } from './stepUpState'
import { confirmNodeLost, dismissNodeFlag, revokeNodeLoss } from './nodeLossActs'
import { confirmEffects, regionLines, type ReviewNodeLoss, type ReviewNodeLost } from './nodeLoss'
import type { Vehicle } from './types'

import './nodeLoss.css'

/** The inbox's review (rules 30): what the loss of one of ours asks of a person, none of it
 *  stopping anything in play. Each item is one card (ledger 2 section 4): the ask first, the
 *  evidence in words in view, what each answer does, buttons naming their outcomes, a note and the
 *  credential behind the click. The Inbox sorts them with everything else (inboxOrder.ts);
 *  `reviewEntries` hands it each card with its rank. */

export interface Props {
  /** the inbox's review items (decisions.inbox.review) */
  review?: { node_losses?: ReviewNodeLoss[]; nodes_lost?: ReviewNodeLost[] }
  /** bridge clock, ms */
  now: () => number
  onDone: (text: string, tone: 'ok' | 'bad') => void
  /** bring this item into view and focus it (the Situation panel's link) */
  focus?: { id: string; n: number } | null
  /** one of ours by its uid as a person names it (its callsign) */
  nameOf?: (uid: string) => string
  vehicles?: Vehicle[]
}



/** A card the Situation panel points to: in view, focused and marked a moment, on
 *  each new request (`n`). */
function useFocused(focusN: number | undefined) {
  const ref = useRef<HTMLElement | null>(null)
  const [flash, setFlash] = useState(false)
  useEffect(() => {
    if (!focusN) return
    focusInView(ref.current)
    // marked on the next frame (as reveal.ts's useFlash), so a repeat restarts it
    const raf = requestAnimationFrame(() => setFlash(true))
    const t = window.setTimeout(() => setFlash(false), FLASH_MS)
    return () => {
      cancelAnimationFrame(raf)
      window.clearTimeout(t)
    }
  }, [focusN])
  return { ref, flash }
}

/** The note a decision carries, behind a click (an optional one; a required one is always shown). */
function NoteField({ note, setNote, busy, label, placeholder, required }: { note: string; setNote: (s: string) => void; busy: boolean; label: string; placeholder: string; required?: boolean }) {
  const [open, setOpen] = useState(false)
  if (!open && !required) {
    return (
      <button type="button" className="ghost mini ap-addnote" onClick={() => setOpen(true)}>
        Add a note
      </button>
    )
  }
  return <input className="ix-note" value={note} maxLength={300} onChange={(e) => setNote(e.target.value)} placeholder={placeholder} aria-label={label} disabled={busy} autoFocus={!required} />
}







/** Rules 30 (docs/NODE-LOSS.md section 5, decisions D1 and D2): one of ours presumed lost by
 *  the engine, or silent and flagged by hand. The ask first ("DRONE 1 has been silent 47 s: is it
 *  lost?"), the evidence, what confirming does (in view: it is the decision), then Confirm lost and,
 *  for a hand flag, Dismiss. Only an authorised operator confirms it (the bridge
 *  refuses anyone else, and every agent), on exactly the silence shown, saying why where nothing
 *  presumes it lost, with the credential again in a credentials run. A presumption is the engine's
 *  and goes when the vehicle is heard again or is confirmed lost. Confirming is a second click: it
 *  asks for the note and the credential first. */
export function NodeLossItem({ n, now, onDone, focusN }: { n: ReviewNodeLoss; now: () => number; onDone: Props['onDone']; focusN?: number; nameOf?: Props['nameOf'] }) {
  const { ref, flash } = useFocused(focusN)
  const [open, setOpen] = useState<'none' | 'confirm' | 'dismiss'>('none')
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [operator] = useOperator()
  const auth = useAuth()
  const ops = useOperators()
  // decision D1: only an authorised operator confirms (the bridge enforces it)
  const authority = auth.auth === 'credentials' ? auth.consequential === true : ops.consequential.includes(operator)
  // removing a keep-out protection: the credential again in a credentials run
  const stepUp = stepsUp()
  const [step, setStep] = useStepUp()
  const [stepErr, setStepErr] = useState(false)
  const who = n.callsign ?? `vehicle ${n.uid}`
  const since = (t: number) => `${fmtDuration(Math.max(0, now() / 1000 - t))} ago`
  const noteOk = !n.needs_note || note.trim().length > 0
  const confirm = async () => {
    if (stepUp && !stepUpReady(step)) {
      setStepErr(false)
      return
    }
    setBusy(true)
    setStepErr(false)
    try {
      const proof = stepUp ? await stepUpProof(step, { act: 'node_lost', uid: n.uid, since: n.since }) : {}
      await confirmNodeLost(n.uid, n.since, note, proof)
      onDone(`${who} confirmed lost: out of every keep-out, its orders ended`, 'ok')
      setOpen('none')
    } catch (e) {
      setStepErr(e instanceof StepUpNeeded)
      onDone(`${who}: confirming the loss refused: ${(e as Error).message}`, 'bad')
    } finally {
      setBusy(false)
      setStep({ password: '' })
    }
  }
  const dismiss = async () => {
    setBusy(true)
    try {
      await dismissNodeFlag(n.uid, note)
      onDone(`${who}: the hand flag dismissed`, 'ok')
    } catch (e) {
      onDone(`${who}: dismiss refused: ${(e as Error).message}`, 'bad')
    } finally {
      setBusy(false)
    }
  }
  const where = regionLines(n.region?.parts ?? [])
  return (
    <article
      ref={ref}
      tabIndex={-1}
      className={`ix-card rv-card nl-card${flash ? ' rv-flash' : ''}`}
      aria-label={`${n.state === 'presumed_lost' ? 'Presumed lost' : 'Flagged as possibly lost'}: ${who}`}
      data-review-id={n.id}
    >
      <header>
        <b>{who}</b> <span className={`nl-tag${n.state === 'silent' ? ' silent' : ''}`}>{n.state === 'presumed_lost' ? 'PRESUMED LOST' : 'SILENT, FLAGGED'}</span>
      </header>
      <p className="rv-basis">
        <b>
          {who} has been silent {n.silent_s == null ? 'since it started (no position reported)' : fmtDuration(n.silent_s)}: is it lost?
        </b>
      </p>
      {n.evidence.length > 0 && (
        <ul className="nl-lines" aria-label="Evidence">
          {n.evidence.map((e, i) => (
            <li key={i}>{e.detail}</li>
          ))}
        </ul>
      )}
      {n.since_t != null && <p className="rv-means">Its last report was {since(n.since_t)}.</p>}
      <ul className="nl-lines" aria-label="What confirming does">
        {confirmEffects(n).map((l, i) => (
          <li key={i}>{l}</li>
        ))}
      </ul>
      {!authority && n.actions.includes('confirm') && <p className="eg-alarm">Only an authorised operator confirms one of ours lost: the bridge will refuse you.</p>}
      {n.refused && <p className="eg-alarm">Not confirmable now: {n.refused}.</p>}
      {(where.length > 0 || n.last_position) && (
        <details className="ap-fold" onClick={(e) => e.stopPropagation()}>
          <summary>Where it was last, and where the keep-outs count it</summary>
          {n.last_position && <p className="ap-where">Last position {n.last_position[1].toFixed(5)}, {n.last_position[0].toFixed(5)}</p>}
          {where.length > 0 && (
            <ul className="nl-lines" aria-label="Where the keep-outs count it">
              {where.map((l) => (
                <li key={l}>{l}</li>
              ))}
            </ul>
          )}
        </details>
      )}
      {open === 'none' && (
        <div className="rv-actions">
          {n.actions.includes('confirm') && (
            <button className="ap-btn" disabled={busy} onClick={() => setOpen('confirm')} title="An authorised operator confirms it">
              Confirm lost
            </button>
          )}
          {n.actions.includes('dismiss') && (
            <button className="ap-btn" disabled={busy} onClick={() => setOpen('dismiss')} title="Take the hand flag off the list (any operator)">
              Dismiss
            </button>
          )}
        </div>
      )}
      {open === 'confirm' && (
        <div className="rv-actions">
          <NoteField
            note={note}
            setNote={setNote}
            busy={busy}
            label="Why it is confirmed lost"
            placeholder={n.needs_note ? 'Why it is lost: nothing presumes it (required, logged)' : 'Note (optional, logged)'}
            required={n.needs_note}
          />
          <button className="ap-btn" disabled={busy || !noteOk || (stepUp && !stepUpReady(step))} onClick={() => void confirm()}>
            Confirm {who} lost
          </button>
          <button className="ghost mini" disabled={busy} onClick={() => setOpen('none')}>
            Cancel
          </button>
          {stepUp && (
            <div className="tk-stepup">
              <StepUpField
                passwordLabel="Consequential: your password to confirm the loss"
                purpose={`to confirm ${who} lost`}
                ariaLabel="Your password, to confirm this loss"
                value={step}
                onChange={setStep}
                busy={busy}
                bad={stepErr}
                onEnter={() => noteOk && void confirm()}
              />
            </div>
          )}
        </div>
      )}
      {open === 'dismiss' && (
        <div className="rv-actions">
          <input className="ix-note" value={note} maxLength={300} onChange={(e) => setNote(e.target.value)} placeholder="Why (optional, logged)" aria-label="Note for the dismissal" disabled={busy} />
          <button className="ap-btn" disabled={busy} onClick={() => void dismiss()}>
            Dismiss the flag
          </button>
          <button className="ghost mini" disabled={busy} onClick={() => setOpen('none')}>
            Cancel
          </button>
        </div>
      )}
    </article>
  )
}

/** Rules 30: one of ours confirmed lost, a settled state with a way back: what is true now, what
 *  would bring it back (any report from it, or an operator's revoke, saying why), and the engine's
 *  records behind a fold. Any operator revokes it (it only restricts: it counts in every keep-out
 *  again at once). */
export function NodeLostItem({ n, now, onDone, focusN }: { n: ReviewNodeLost; now: () => number; onDone: Props['onDone']; focusN?: number }) {
  const { ref, flash } = useFocused(focusN)
  const [revoking, setRevoking] = useState(false)
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const who = n.callsign ?? `vehicle ${n.uid}`
  const revoke = async () => {
    setBusy(true)
    try {
      await revokeNodeLoss(n.uid, note)
      onDone(`${who}: the loss revoked: it counts in every keep-out again`, 'ok')
      setRevoking(false)
    } catch (e) {
      onDone(`${who}: revoking the loss refused: ${(e as Error).message}`, 'bad')
    } finally {
      setBusy(false)
    }
  }
  const closed = Object.keys(n.closed)
  return (
    <article ref={ref} tabIndex={-1} className={`ix-card rv-card nl-card nl-settled${flash ? ' rv-flash' : ''}`} aria-label={`Confirmed lost: ${who}`} data-review-id={n.id}>
      <header>
        <b>{who}</b> <span className="nl-tag lost">LOST</span>
        <span className="muted">
          {' '}
          confirmed by {n.by} {fmtDuration(Math.max(0, now() / 1000 - n.t))} ago
        </span>
      </header>
      <p className="rv-means">
        {n.note ? `"${n.note}". ` : ''}Out of every keep-out; it takes no order. Any report from it revokes the loss.
      </p>
      {(closed.length > 0 || n.from || n.not_held || n.since_t != null) && (
        <details className="ap-fold" onClick={(e) => e.stopPropagation()}>
          <summary>What it closed</summary>
          <p className="ap-where">
            Silent {n.since_t == null ? 'since it started' : `since its report ${fmtDuration(Math.max(0, now() / 1000 - n.since_t))} ago`}.
            {closed.length ? ` Its owed lost-link values (${closed.join(', ')}) closed.` : ''}
            {n.from ? ` Carried over from ${n.from}.` : ''}
            {n.not_held ? ` It holds no vehicle now: ${n.not_held}.` : ''}
          </p>
        </details>
      )}
      {!revoking ? (
        <div className="rv-actions">
          <button className="ap-btn" disabled={busy} onClick={() => setRevoking(true)} title="It counts in every keep-out again at once and takes orders (any operator)">
            Revoke the loss
          </button>
        </div>
      ) : (
        <div className="rv-actions">
          <span className="rv-warn">Revoke the loss of {who}? It counts in every keep-out again at once.</span>
          <input className="ix-note" value={note} maxLength={300} onChange={(e) => setNote(e.target.value)} placeholder="Why (required, logged)" aria-label="Why the loss is revoked" disabled={busy} />
          <button className="ap-btn" disabled={busy || !note.trim()} onClick={() => void revoke()}>
            Revoke the loss of {who}
          </button>
          <button className="ghost mini" disabled={busy} onClick={() => setRevoking(false)}>
            Cancel
          </button>
        </div>
      )}
    </article>
  )
}


