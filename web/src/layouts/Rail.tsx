// The rail layout (`?layout=rail`, bd epic murmur-iobd; direction 2 of the console's layout
// prototypes): map first. The map takes all but two edges. On the left, one rail holds the Fleet
// list and the command log as tabs, one list at a time, each tab saying on its face
// what its list holds and what in it wants a look, and folding to its tabs (RailLists.tsx). On the
// right, only Needs you, the Inbox's content, folding to a strip with its count; the other drawers
// open from the top bar over it, one at a time (RailNeeds.tsx). The top bar is one thin status
// line. The floating panels keep to the map's element, which is whatever the two edges leave.
// Anchors: Apple Maps' sidebar over a full-bleed map, VS Code's activity bar, a ship's ECDIS display.
//
// The person's tab and what is folded are remembered in this browser (railTabs.ts). Display only
// (parts.ts): this file holds no console state, asks the bridge nothing, and every card decides as
// it does in the Inbox; only where things sit, and which list is in front, is its own.
import { type MouseEvent, useEffect, useMemo, useState } from 'react'
import type { LayoutProps } from './parts'
import { RailLists } from './RailLists'
import { commandFace, fleetFace, orderTally, parseRailState, RAIL_DEFAULT, railStateText, type RailState, revealed, unseenOrders } from './railTabs'
import './rail.css'

/** Where this browser keeps the rail's state. */
const KEY = 'fleetmesh.layout.rail'

function remembered(): RailState {
  try {
    return parseRailState(localStorage.getItem(KEY))
  } catch {
    return RAIL_DEFAULT
  }
}

export function Rail({ parts }: LayoutProps) {
  const { bar, dock, facts } = parts
  const [s, setS] = useState(remembered)
  useEffect(() => {
    try {
      localStorage.setItem(KEY, railStateText(s))
    } catch {
      // storage refused (a private window, blocked site data): the state lasts this page only
    }
  }, [s])

  // a request to show a row (a vehicle clicked on the map) brings its list forward (railTabs.ts
  // `revealed`), in this render, so the list is in sight when it scrolls
  const rv = facts.reveal
  const [asked, setAsked] = useState(() => rv?.fleet ?? null)
  if (rv && rv.fleet !== asked) {
    setAsked(rv.fleet)
    if (rv.fleet) setS((x) => revealed(x))
  }
  // the glance's trouble count shows the next of ours in trouble in the Fleet list: the list shows
  const onStatus = (e: MouseEvent) => {
    if (e.target instanceof Element && e.target.closest('.trouble')) setS((x) => ({ ...x, tab: 'fleet', open: true }))
  }

  // the command log's mark: orders that failed since the log was last in front of the person
  const taskings = facts.orders?.taskings
  const intents = facts.orders?.intents ?? null
  const tally = useMemo(() => (taskings ? orderTally(taskings, intents) : null), [taskings, intents])
  const reading = s.open && s.tab === 'command'
  const [seen, setSeen] = useState<ReadonlySet<string>>(() => new Set())
  if (reading && tally && tally.failures.some((f) => !seen.has(f.key))) setSeen(new Set([...seen, ...tally.failures.map((f) => f.key)]))
  const faces = [fleetFace(facts.glance, facts.trouble), commandFace(tally, reading || !tally ? 0 : unseenOrders(tally.failures, seen))]

  // Needs you. Whatever asks for the Inbox (its button, a card opened from elsewhere) opens the dock's Inbox: here that unfolds Needs you, and the dock is left free for the
  // other drawers, which lie over Needs you while one is open
  const askedInbox = dock.open === 'inbox'
  const drawer = dock.open != null && !askedInbox ? dock.drawer : null
  const needsShown = !drawer && (s.needs || askedInbox)
  if (askedInbox && !s.needs) setS((x) => ({ ...x, needs: true }))
  const { show } = dock
  useEffect(() => {
    if (askedInbox) show(null)
  }, [askedInbox, show])
  return (
    <>
      {/* one thin status line: the run, the glance, then what waits and the drawers, then who and when */}
      <header className="topbar">
        <span className="rl-top-run">
          {bar.menu}
          {bar.brand}
        </span>
        <span className="rl-top-status" onClickCapture={onStatus}>
          {bar.glance}
          {bar.connection}
        </span>
        <span className="spacer" />
        <span className="rl-top-acts">
          {bar.inbox}
          <nav className="rl-drawers" aria-label="Drawers over the right edge, one at a time">
            {bar.queue}
            {bar.sitrep}
            {bar.officer}
            {bar.parcels}
          </nav>
          {bar.load}
          {bar.views}
        </span>
        <span className="rl-top-who">
          {bar.operator}
          {bar.clock}
        </span>
      </header>
      {/* the banners head Needs you or the drawer over it; with both folded away, the map's top */}
      {parts.map({ banners: !needsShown && !drawer, runChip: false, beside: true })}
      <RailLists
        state={s}
        set={setS}
        faces={faces}
        selected={facts.selected}
        fleet={parts.fleet}
        command={parts.command}
      />
    </>
  )
}
