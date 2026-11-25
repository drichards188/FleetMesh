// The decisions layout (`?layout=decisions`; bd epic murmur-iobd, direction 3 of the layout
// prototypes): what needs the person leads. The Inbox is the standing column on the left, its
// list by consequence then time (Needs you, then For information, folded), each card as built;
// selecting one frames the map on its evidence, and the map is the stage on the right. Fleet and
// the command log are reference, opened on demand from the top bar (or the map: the glance's
// trouble count opens Fleet) as drawers over the map's right edge, one at a time with the dock's
// own (Queue, Groups, Sitrep, Officer). Display only (parts.ts): it holds no state of the
// console's and decides nothing.
import { type KeyboardEvent, type MouseEvent, type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { DecisionsQuiet } from './DecisionsQuiet'
import { overMap, REF_IDS, REF_LABEL, REF_TITLE, type RefId, toggleRef } from './decisionsModel'
import type { LayoutProps } from './parts'
import './decisions.css'

/** The Inbox's "For information" heading, which this layout makes the fold's toggle. */
const INFO_HEAD = '.ix-h-quiet'

export function Decisions({ parts }: LayoutProps) {
  const { bar, dock, facts } = parts
  const { open, show } = dock
  // the reference list shown over the map's right edge: the Fleet list stands there full height
  // from the start, until the person closes it or a drawer of the dock's takes the edge; it
  // returns when they let go
  const [ref, setRef] = useState<RefId | null>('fleet')
  // "For information", below Needs you: folded until the person opens it or a card in it is chosen
  const [infoOpen, setInfoOpen] = useState(false)
  const col = useRef<HTMLDivElement>(null)
  // the Inbox's own list, where the quiet state goes
  const [list, setList] = useState<HTMLElement | null>(null)
  const chosen = useRef<Element | null>(null)
  const scrollTo = useRef<Element | null>(null)

  // The Inbox is the column, never a drawer: the dock rests on it rather than on nothing, so the
  // console treats the Inbox as shown beside the map (a fit of the map keeps its usual margin, and
  // what opens "the Inbox" lands in the column).
  useEffect(() => {
    if (open == null) show('inbox')
  }, [open, show])
  // One drawer over the map at a time: a drawer of the dock's that opens takes the list's place
  // (state adjusted while rendering when the prop it follows changes)
  const [dockWas, setDockWas] = useState(open)
  if (open !== dockWas) {
    setDockWas(open)
    if (open != null && open !== 'inbox') setRef(null)
    else if (dockWas != null && dockWas !== 'inbox') setRef('fleet')
  }

  /** a reference list shown (null: none), any drawer of the dock's closed for it */
  const showRef = useCallback(
    (id: RefId | null) => {
      setRef(id)
      if (id) show('inbox')
    },
    [show],
  )

  // a list opened since a drawer of the dock's (a drawer that opens closes the list, above): the
  // drawer closes for it
  useEffect(() => {
    if (ref && open != null && open !== 'inbox') show('inbox')
  }, [ref, open, show])

  // What the parts render inside them, read after each of the console's renders (it renders at
  // least twice a second): the Inbox's list, its fold's heading, a card chosen below the fold.
  useLayoutEffect(() => {
    const c = col.current
    // the Inbox's list (it can be mounted afresh after a crash)
    const l = c?.querySelector<HTMLElement>('.ix-scroll') ?? null
    if (l !== list) setList(l)
    // the fold's toggle is the Inbox's own heading: a button to the keyboard and to a reader
    const head = c?.querySelector<HTMLElement>(INFO_HEAD)
    if (head) {
      const title = infoOpen ? 'Fold what is for information only' : 'Show what is for information only'
      if (head.tabIndex !== 0) head.tabIndex = 0
      if (head.getAttribute('role') !== 'button') head.setAttribute('role', 'button')
      if (head.getAttribute('aria-expanded') !== String(infoOpen)) head.setAttribute('aria-expanded', String(infoOpen))
      if (head.title !== title) head.title = title
    }
    // a card below the fold newly chosen (a review link): the fold opens and the card comes into view; folding again later is the person's
    const now = c?.querySelector(`${INFO_HEAD} ~ .sel, ${INFO_HEAD} ~ .rv-flash`) ?? null
    if (now !== chosen.current) {
      chosen.current = now
      if (now && !infoOpen) {
        scrollTo.current = now
        setInfoOpen(true)
      }
    }
    if (infoOpen && scrollTo.current) {
      scrollTo.current.scrollIntoView({ block: 'nearest' })
      scrollTo.current = null
    }
  }, [parts, infoOpen, list, show])

  /** the fold's heading, by click */
  const onColumnClick = (e: MouseEvent) => {
    if ((e.target as Element).closest?.(INFO_HEAD)) setInfoOpen((o) => !o)
  }
  /** and by Enter or Space while it has the focus (Space then is not the map's) */
  const onColumnKey = (e: KeyboardEvent) => {
    if ((e.key !== 'Enter' && e.key !== ' ') || !(e.target as Element).matches?.(INFO_HEAD)) return
    e.preventDefault()
    e.stopPropagation()
    setInfoOpen((o) => !o)
  }

  // the reference list over the map's right edge, with its own close
  const shown = overMap(open, ref)
  let drawer: ReactNode = null
  if (shown === 'dock') drawer = dock.drawer
  else if (shown === 'ref' && ref)
    drawer = (
      <section className="ix dc-ref" aria-label={REF_LABEL[ref]} data-ref={ref}>
        {ref === 'log' ? parts.command : ref === 'parcels' ? parts.parcelsPanel : parts.fleet}
        <button className="ghost dc-ref-close" onClick={() => showRef(null)} aria-label={`Close ${REF_LABEL[ref]}`}>
          Close
        </button>
      </section>
    )

  return (
    <>
      <header className="topbar">
        {bar.menu}
        {bar.brand}
        {/* the glance; its trouble count shows the next one in trouble in the Fleet list, so it opens Fleet */}
        <span
          className="dc-glance"
          onClickCapture={(e) => {
            if ((e.target as Element).closest?.('.trouble')) showRef('fleet')
          }}
        >
          {bar.glance}
        </span>
        {bar.connection}
        <span className="spacer" />
        {/* every list and drawer that opens over the map's right edge, one at a time; the Inbox is
            the column, so it has no button here */}
        <nav className="dock-tabs dc-tabs" aria-label="Lists and drawers over the map's right edge, one at a time">
          {REF_IDS.map((id) => (
            <button
              key={id}
              className={`ghost platforms-btn ix-btn ${ref === id && shown === 'ref' ? 'on' : ''}`}
              aria-pressed={ref === id && shown === 'ref'}
              onClick={() => showRef(toggleRef(shown === 'ref' ? ref : null, id))}
              title={REF_TITLE[id]}
            >
              {REF_LABEL[id]}
            </button>
          ))}
          <span className="dc-tabs-gap" aria-hidden="true" />
          {bar.queue}
          {bar.sitrep}
          {bar.officer}
        </nav>
        {bar.load}
        {bar.views}
        {bar.operator}
        {bar.clock}
      </header>
      {/* the main column: what waits on the person alone, then the Inbox as a standing column */}
      <div className="dc-col" ref={col} data-info={infoOpen ? 'open' : 'folded'} onClick={onColumnClick} onKeyDown={onColumnKey}>
        {parts.inbox}
        {list && createPortal(<DecisionsQuiet facts={facts} onOpen={showRef} />, list, 'dc-quiet')}
      </div>
      {/* the stage: the banners head the column (the Inbox shows them), the run is in the bar */}
      {parts.map({ drawer, banners: false, runChip: false })}
    </>
  )
}
