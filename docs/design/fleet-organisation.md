# Fleet organisation: bases, states, views

Status: built 2026-11-25 (rules 4 for the dispatcher's part). The console's Fleet list and map, the
bridge's `GET /api/fleet` and the dispatcher's returns follow this document.

## 1. Purpose

The console showed the fleet as a flat list of vehicles grouped only by domain, and the map as one
icon per vehicle. At 54 vehicles that is already a pile of overlapping icons at each hub, and a
list in which the vehicle that needs a person sits among fifty that do not. Real delivery
operations are not organised that way (DECISIONS.md, 2026-11-25, "The fleet is organised by home
base"): every vehicle belongs to a base, the base is the unit people look at, and what matters
about a vehicle is its operational state (ready, out on a task, returning, charging, in trouble)
more than its identity. This document says how FleetMesh organises its fleet the same way.

## 2. Bases and sites

**Base.** Every vehicle has a home base: a hub of the scenario (`[[hub]]` in its `dispatch.toml`,
or `sim/hubs.toml` without a scenario). It is where the vehicle parks, charges or has its battery
swapped, is loaded, and goes back to after a delivery, as Wing's drones belong to a nest at a store,
Zipline's aircraft to a hub or a dock, and a robotaxi to its depot.

- A fleet file names it per unit (or per swarm): `home_hub = "hub-marina"`. `sim/swarm.py` passes it
  into the manifest (`sim/run/fleet.json`) as the vehicle's `home_hub`.
- Without one, the bridge takes the hub the vehicle was launched at (the manifest's `home`, within
  the hub's catchment: the larger of three times its radius and 150 m), else the hub its autopilot's
  home is at. A vehicle launched away from every hub has no base and is listed under "No base".
- harbour-city's four bases: `hub-marina` (24 drones, 2 planes), `hub-marina-kerb` (8 cars),
  `hub-pier` (16 drones), `hub-pier-dock` (4 boats). wind-event has one (`hub-crissy`), coastal-boat
  two (`hub-pier`, `hub-pier-dock`).

**Site.** Bases that work together at one place, for the console only: harbour-city's Marina (the
drone pads and the car kerb) and Pier 39 (the drone pads and the boat dock). A scenario declares them
in its dispatch file; a hub is in at most one site; nothing the rules or the dispatcher read.

```toml
[[site]]
id = "marina"
name = "Marina"
hubs = ["hub-marina", "hub-marina-kerb"]
```

DJI FlightHub 2 has the same shape one level up (organisation, projects, then devices and docks);
FleetMesh has one organisation, so sites and bases are enough.

## 3. Operational states

The bridge derives each vehicle's state from what it already knows (`decision/fleet.rs`): the
dispatcher's work on it, the orders running and waiting, its link, its navigation trust, its
battery, the inbox's alerts and automation's issues about it. One state per vehicle, the first that
applies in this order:

| state | means |
| --- | --- |
| out of service | an operator confirmed it lost (rules 30), or it reports damage that stops it moving: not available, not expected back in this run |
| offline | no telemetry for longer than its link's stale limit (the inbox's `lost_link`): where it is and what it does are not known now |
| needs attention | something about it needs a person: an order of it waits for a decision, an alert or an issue is open about it (not moving, left its order, a failed order, an unanswered command, a return that failed, a traffic conflict that outlasted automation), its navigation is untrusted or degraded, its battery is under its reserve away from a hub, or it stopped away from a hub with no order |
| holding | automation is waiting something out for it and will go on by itself: a drop zone that does not look clear yet, other traffic, a stale reading, a hold it retries, an order waiting until its link can carry it |
| loading | its pickup is proposed or running, or its parcel is aboard at the hub and its delivery is about to go |
| outbound | its delivery is running and it is more than 80 m from the drop zone |
| delivering | its delivery is running and it is within 80 m of the drop zone: hovering, looking, letting go |
| returning | on its way back to a hub (the dispatcher's return, or any `return_to_hub` or `rtl`), empty or bringing an undelivered parcel back |
| charging | at a hub having its battery swapped, or due a swap (under 45%, the dispatcher's threshold) |
| on another order | running an order that is not the dispatcher's: a person's, another agent's, the navigation monitor's |
| ready | at a hub, its battery at or above 45%, with no order: free for the dispatcher |

What the code cannot tell honestly is left out: there is no maintenance state (nothing in a run
takes a vehicle in for service), and "charging" covers a swap and a vehicle waiting for a charged
pack alike (the hub's pack stock is per hub, not per vehicle). A delivery to a zone the drop-time
look refuses ends with the parcel aboard; the vehicle is then returning, and the parcel's issue is
the parcel's, not the vehicle's.

Each vehicle's state comes with a line saying why ("taking p-0012 to dz-city-04", "its deliver order
waits for your decision", "no telemetry for 14 s") and how long it has been in it.

## 4. `GET /api/fleet`

Read-only, never logged, decides nothing:

```jsonc
{
  "states": [{"id": "attention", "label": "Needs attention", "problem": true, "means": "..."}, ...],
  "sites": [{"id": "marina", "name": "Marina", "hubs": ["hub-marina", "hub-marina-kerb"]}],
  "bases": [{"id": "hub-marina", "name": "Marina Green hub", "at": [-122.437, 37.8057], "radius_m": 80,
             "site": "marina", "pads": 12, "packs": 16,
             "vehicles": 26, "counts": {"ready": 20, "outbound": 4, "charging": 2},  // its own, by state
             "here": 23, "visitors": 1, "parked": {"ready": 21, "charging": 2}}],  // at it now, any base
  "vehicles": [{"uid": 3, "base": "hub-marina", "at_hub": null, "state": "outbound",
                "why": "taking p-0012 to dz-city-04", "since_s": 41, "parcel": "p-0012"}],
  "counts": {"ready": 40, "outbound": 6, ...},
  "home_base_rules": true
}
```

The states are listed problems first (needs attention, offline, out of service), then a delivery's
cycle in order. A console on a bridge without the endpoint derives a coarser view itself (each
vehicle's base the hub its autopilot's home is at; the state from its link, its latest order and
where it is) and says so.

## 5. The console

**Fleet list.** What needs a person comes first, whatever its base: every vehicle that needs
attention, is offline or is out of service, longest waiting first, each with why and its base. Then
each site and its bases; a base shows its vehicle count and its counts by state as coloured chips,
problems first, and opens to list its vehicles under each state. Filters by type (drone, car, boat,
plane, several at once) and by state; under a filter every base with something to show is open and
the rest are left out. Selection (the left strip, shift-click ranges, right-click to deselect),
details (a click on the row), the Select menu (now with each base) and "add to group" work as
before, so a group order goes to a base's vehicles in two clicks.

**Map.** Every hub has one mark of its own at every zoom: a house standing on a short stem over the
hub's point (a shape nothing else on the map has, in the fleet's own navy and blue so it never
reads as other traffic), its roof the colour of the hub's state (blue all well; amber the wind in
the soft band or vehicles holding; red grounded or someone needed; grey closed), a red count on its
eave of its own vehicles that need the person, and beside it its name with one line under it:
what needs the person first, then whether it can fly, then what its vehicles do ("2 need you ·
Marginal: gusts 12.5 m/s · 18 holding", `web/src/hubSummary.ts`). The vehicles parked at their own
base (ready or charging) collapse into the mark, their number on its walls. Zoomed out past zoom
15, the hubs of a site share one mark ("Marina: 2 out · 30 ready"), and a click on it zooms in to
its hubs. From zoom 17.5 every vehicle is drawn on its own and the marks stay. A vehicle away from
its base (a visitor at another hub), under way, in any state but ready or charging, selected,
followed or named by an order waiting for a person always stays on its own, with its route. A base
collapses only when two or more would.

**A hub's details** open on a click on its mark: a few plain sentences first (whether it can fly
and why, how many of its vehicles need the person, what its vehicles do, how many parcels wait
and when the next is due), then who needs the person and why (each a link to the vehicle), the
wind at the hub against the limits of the strictest platform based there (`weather` of a base in
`GET /api/fleet`: held from the soft limit, refused from the hard), its vehicles by state, its
parcels (a link to the Parcels list at the hub), its pads and charged packs, and the way to show
its parked vehicles one by one.

## 6. Dispatch: home bases (rules 4)

Two changes, both about where a vehicle goes back to:

1. **The assigner** (`fleetmesh-decide` `dispatch::assign_under`) costs the way back of each trip
   to the candidate's home base (`input.vehicles[].home` in `dispatch.plan`), or to the hub nearest
   the drop zone when its battery would not cover the way home. A vehicle visiting another hub is
   therefore dearer than one based where the parcel waits, unless the parcel lies on its way home.
   Under rules 3 the way back went to the hub nearest the zone always. Gated on `HOME_BASE_SINCE`
   ([rules-versioning.md](rules-versioning.md)); replay re-runs the assigner under the log's rules
   and flags a `home` field in a log that claims rules 3.
2. **The dispatcher's return** after a delivery (or with a parcel the drop-time look kept) goes to
   the vehicle's home base, when a dry run of that return would go at once or only wait out what
   automation waits out; else to the nearest hub, as before (logged as a `delivery.event` of kind
   `home`). A vehicle idle for 60 s at a hub other than its base, with no parcel waiting there, is
   sent home (at most four a tick), as Zipline's software sends each aircraft to the dock that suits
   it. Both are the dispatcher's proposals, checked and routed like any other.

Why: with the nearest hub, a live harbour-city run sent 13 of 22 returns to another base than the
vehicle's own: drones to the car kerb, a plane to the boat dock, boats towards a land hub the domain
rule refuses. In the headless harbour-city run under rules 4 every return (15 of 15) went home.

3. **Its own stand (rules 5).** At its base a vehicle parks on its own stand, where it was
   launched (a nest's pad, a kerb's bay, a dock's berth), and lands there again; a visitor gets a
   free visitor pad; a pickup on the stands loads where the vehicle stands
   ([dispatch.md](dispatch.md), delivery-rules.md section 8.2). Every stand lies within its base's
   radius (sim/tests/test_scenarios.py checks it), and a boat is tied up at its berth until it
   first casts off and again when it disarms there. Before, every return landed on the hub's own
   point.

## 7. Open questions

1. Rebalancing between bases (moving idle vehicles to where parcels wait) is not built; a vehicle
   based at one hub serves that hub's parcels, and visitors go home.
2. Supervision by base: a later multi-operator study could hand bases, not vehicles, to operators
   (NASA's m:N work divides vehicles among operators; Skydio's and Wing's remote operations centres
   hand vehicles off). The data model allows it; no scenario uses it.
3. A base's state of its own (open, closed for weather, out of packs) is only its `open` flag and
   pack count now.
