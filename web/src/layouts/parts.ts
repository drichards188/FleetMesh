// The console's parts: what App.tsx hands the layout it shows (bd epic murmur-iobd, the layout
// prototypes; `?layout=cockpit|rail|decisions`, layouts/index.ts). App keeps every piece of state,
// every data hook, effect and handler, and builds each part once a render; a layout only says where
// each part sits. A layout is built from this contract alone: it holds none of the console's state,
// asks the bridge nothing, and changes nothing a part shows or asks. Display only.
//
// How a layout is made (Cockpit.tsx is the model: the console as it always was):
// - one file, `layouts/<Name>.tsx`, exporting one component `(props: LayoutProps) => ReactNode`; it
//   returns the root's children (App renders the root, `div.app[data-layout=<name>]`, the
//   CSS grid of index.css);
// - its own CSS, `layouts/<name>.css`, imported by that file. Every layout's CSS loads whatever
//   layout runs, so every rule is scoped: `.app[data-layout=rail] ...`;
// - small components of its own beside it (`layouts/<Name>*.tsx`), built from these parts too.
// A part is a rendered node: place it once, or not at all (a node placed twice is two of it). The
// map is the one call, `map(slot)`: its element is where the floating panels keep to, wherever the
// layout puts it.
import type { ReactNode } from 'react'
import type { Intent } from '../decisions'
import type { DockId } from '../dock'
import type { InboxSelection } from '../InboxPanel'
import type { Reveal } from '../reveal'
import type { Glance, Trouble } from '../topbar'
import type { LngLat, Tasking } from '../types'

/** The top bar's pieces, each rendered, null while it is not shown. Cockpit sets them in
 *  `header.topbar` in this order (the drawer buttons in `nav.dock-tabs`). */
export interface BarParts {
  /** the main menu's button (R24); null without the launcher (`make up`, `make goldengate`) */
  menu: ReactNode
  /** "FLEETMESH", small */
  brand: ReactNode
  /** the glance (R1, R2): ours, ours in trouble (a click shows the next in the Fleet list) */
  glance: ReactNode
  /** the bridge's connection pill (R19), only while the console is not live */
  connection: ReactNode
  /** the Inbox button (R8): the needs-you count, red when urgent, the soonest time left; it
   *  shows the Inbox drawer, or closes it */
  inbox: ReactNode
  /** the watch officer's button (R22), only where the run's AI takes requests */
  officer: ReactNode
  /** the Queue's button (rules 5), only while an order is held for a link or its drawer is open */
  queue: ReactNode
  /** the Sitrep's button, only at group scale or while its drawer is open */
  sitrep: ReactNode
  /** the Views menu (R18): Weather, Links, Platforms, each opening over the map */
  views: ReactNode
  /** the operator's load (delivery.ts `operatorLoad`): items waiting, the oldest one's wait, decisions in the last ten minutes */
  load: ReactNode
  /** the Parcels button (a dock drawer: Cockpit and Rail); the Decisions layout lists Parcels with Fleet and Log */
  parcels: ReactNode
  /** who this console acts as, and the DEV mark in a dev run (R21) */
  operator: ReactNode
  /** the world's time, and in a situation its T+ and time left (R15) */
  clock: ReactNode
  /** the glance's and the run's figures, for a layout that words them itself in place of `glance`
   *  (Cockpit: topbar.ts `glanceWords`, `runStanding`). Optional: `glance` and `run` need none of it */
  figures?: BarFigures
}

/** The figures behind the glance and the run chip, each as the bar's own nodes count it. */
export interface BarFigures {
  /** the glance's figures (topbar.ts): how many of ours */
  glance: Glance
  /** ours in trouble, by the fleet rows' own slot (topbar.ts `troubleOf`: how many, the worst tone,
   *  the hover's words) */
  trouble: Trouble
  /** the glance's own click on the trouble count: the next of ours in trouble shown in the Fleet list */
  showTrouble: () => void
}

/** What a layout decides about the map, passed to `ConsoleParts.map`. Each left out is as Cockpit
 *  has it. */
export interface MapSlot {
  /** the drawer laid over the map's right edge (Cockpit: `dock.drawer`). The map makes room for it
   *  (its zoom buttons and the floating panels keep clear) and a fit of the map
   *  pads for it. Leave it out when the layout puts the drawer beside the map: a fit then keeps
   *  the usual margin. */
  drawer?: ReactNode
  /** whether the alarm banners (R10) sit over the map's top. Default: while no drawer is shown
   *  anywhere (the drawer shown heads with them, `ConsoleParts.banners`) */
  banners?: boolean
  /** whether the Situation chip floats on the map (default true); false when the layout shows
   *  `bar.run`: the map then floats only the open Situation panel */
  runChip?: boolean
  /** whether what the layout shows at the right edge lies beside the map whatever is shown there
   *  (the Inbox kept beside it with no drawer open): a fit then keeps the usual margin, even one
   *  asked from an Inbox card. Default: beside only while a drawer is open and not in `drawer` */
  beside?: boolean
}

/** The right dock (dock.ts): one drawer at a time. */
export interface DockParts {
  /** which drawer is shown, or none */
  open: DockId | null
  /** the drawer shown (its panel, `aside.ix`, absolutely placed over the map's right edge by
   *  dock.css), null while none is; the banners and every order waiting for a person head it
   *  (the Inbox: the cards themselves) */
  drawer: ReactNode
  /** a drawer's button: shows it in place of the one shown, or closes the dock when it is that one */
  toggle: (id: DockId) => void
  /** shows this drawer in place of the one shown (null closes the dock) */
  show: (id: DockId | null) => void
}

/** The few facts a layout arranges by. */
export interface ConsoleFacts {
  /** of ours, how many are selected (the order form commands them) */
  selected: number
  /** something urgent is among what needs the person (the Inbox badge is red) */
  urgent: boolean
  /** a drawer is shown (`dock.open` says which) */
  drawerOpen: boolean
  /** the bridge answers (false: connecting, or down: `bar.connection` shows) */
  live: boolean
  // For a layout that shows one list at a time (Rail.tsx): what each list holds, so its tab can say
  // so while the list is out of sight. Each optional: a layout that shows every list needs none.
  /** the glance's figures (topbar.ts `glanceOf`): how many of ours */
  glance?: Glance
  /** ours in trouble (topbar.ts `troubleOf`): the glance's count, by the rows' own exception slot */
  trouble?: Trouble
  /** the newest request to show rows in the Fleet list (a vehicle clicked on the map, the glance's
   *  trouble count; reveal.ts: a new object each request), null before the first */
  reveal?: { fleet: Reveal | null }
  /** the command log's orders (the bridge's taskings) and the intents they carry out */
  orders?: { taskings: readonly Tasking[]; intents: readonly Intent[] | null }
  /** in a situation, its newest thing to watch for (the Situation chip's line); null when none */
  watch?: string | null
}

/** What a layout may do for the person, each as the console's own controls already do it. None
 *  decides anything: an approval, a signature or a review is made on its card. */
export interface ConsoleActs {
  /** one of ours' details open over the map, the map flying there and following it */
  showVehicle: (uid: number) => void
  /** these vehicles selected (the order form then commands them), the first shown in the Fleet list */
  selectVehicles: (uids: number[]) => void
  /** an Inbox item selected, as a click on its card: its vehicles selected and framed on the map
   *  with the order's preview */
  focusInbox: (sel: InboxSelection, uids: number[], intent: Intent | null) => void
  /** an order's card: in the Inbox while it waits for a person, else in Decisions (the Inbox drawer
   *  is shown) */
  openIntent: (id: number) => void
  /** a review item's card in the Inbox, brought into view (the person decides there) */
  openReview: (id: string) => void
  /** frame these points on the map */
  fit: (points: LngLat[]) => void
}

/** Everything a layout receives. */
export interface ConsoleParts {
  /** the top bar's pieces */
  bar: BarParts
  /** the Fleet list (R1 to R3): `aside.panel.fleet`, grid area `fleet` */
  fleet: ReactNode
  /** the parcels of a delivery run (ParcelsPanel.tsx): `aside.panel.parcels`; the Decisions layout lists it with Fleet and Log */
  parcelsPanel: ReactNode
  /** the command log with the order form (R11, R12): `aside.panel.tasking`; Cockpit puts it in
   *  `.rail` under the fleet list */
  command: ReactNode
  /** the map, `main.mapwrap` (grid area `map`), with everything drawn on it, the floating panels
   *  (Weather, Links, Platforms, the vehicle details, the Situation panel), the
   *  weather bar and the map's own
   *  banners. The floating panels keep to this element, wherever it sits. Call it once a render. */
  map: (slot?: MapSlot) => ReactNode
  /** the right dock: the drawer shown, and the way to show or close one */
  dock: DockParts
  /** the Inbox drawer's content (approvals, reviews, alerts; R8, R9), for a layout that
   *  keeps it outside the dock: the same node `dock.drawer` is while the Inbox is the drawer shown,
   *  so place one or the other, never both. The banners head it, as they head the drawer shown */
  inbox: ReactNode
  /** the facts a layout arranges by */
  facts: ConsoleFacts
  /** what a layout may do for the person */
  act: ConsoleActs
}

/** A layout's props. */
export interface LayoutProps {
  parts: ConsoleParts
}

/** A layout: the root's children, arranged from the parts. */
export type LayoutComponent = (props: LayoutProps) => ReactNode
