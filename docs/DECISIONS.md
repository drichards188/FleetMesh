# Decisions

One short entry each. The reason matters more than the choice. The 2026-11-24 entries were
written while the maintainer was away, from the instructions given for the cut, so the reasons are
the writer's reading of the intent; the maintainer should correct any that misstate it.

## 2026-11-24: History starts clean, no first commit until the cut is done

The tree was copied from another project and is still being cut down. A commit made now would
record half-removed modules, a build that does not pass and names that are about to change. Starting
the history after the cut means every commit is about FleetMesh, can be read as a delivery
project, and builds.

## 2026-11-24: Port by compiler-driven cut from the earlier project at f7b1874, with a name catalog

We took the earlier project at commit f7b1874, removed what has no delivery meaning, and let the
compiler list what broke, repeating until it built. A name catalog (CATALOG.tsv) records, for each
module and name, whether it was kept, renamed or removed. The maintainer was away, so the catalog
decisions were applied from suggestions, not reviewed one by one. They are reviewable in CATALOG.tsv,
and any entry can be reversed by restoring from f7b1874.

## 2026-11-24: GNSS interference is modelled only as effects on the fleet's own receivers

Scenario events say what a receiver experiences (no fix, a position that walks off, degraded
accuracy) inside a circle and a time window. No vehicle is ever a source of jamming or spoofing, and
the model carries no instructions for producing either. This is enough to test how a supervisor and
the rules cope with a degraded or untrustworthy position, which is the delivery question.

## 2026-11-24: Cellular link classes and height falloff

Delivery fleets will mostly use public cellular networks, not private radios. Tower antennas point
down, so a drone's coverage weakens with altitude: full strength to 120 m above the tower's ground,
none at 300 m (draft values). That matters for drones that fly at 80 to 120 m, and it makes
coverage a function of position and height that the lost-link rule can predict.

## 2026-11-24: Automatic vehicle-side link failover, with draft parameters

A vehicle with two radios switches itself: when the main link's loss averaged over 3 s passes 50%
(or it is unavailable), it moves to the backup; it returns only after 10 s of healthy main link; each
switch is a 2 s gap. An operator cannot react in seconds and should not have to, and the switch
depends only on position, events and the clock, so replay and reruns are the same. The numbers are
drafts: fast enough to be useful, slow enough not to flap. They are read from the main link's class
so a platform can change them.

## 2026-11-24: Four delivery platforms mapped to ArduCopter, Rover, Rover boat and ArduPlane

Delivery drone, car, boat and plane run on ArduCopter, Rover (car frame), Rover (boat frame) and
ArduPlane. The SITL builds, stand-in behaviours, parameters and failsafes already exist for these,
and a delivery fleet needs these four kinds. No new autopilot work is needed to start.

## 2026-11-24: The satellite remnant is kept for GNSS and SATCOM availability for now

`crates/fleetmesh-space` keeps the pure parts (orbit propagation, dilution of precision, access
windows, the scenario clock); its catalog, tasking queries and the collection and cover orders are
gone. A delivery model of SATCOM and GNSS availability may reuse the pure parts. Two
`FLEETMESH-REVIEW` placeholders (fixed SATCOM parameters, fixed GNSS sky) mark where. If nothing
uses the crate by the first commit, delete it.

## 2026-11-24: Lost tracks are replaced by lost link

The earlier project tracked contacts and had a ledger of lost ones. A delivery operator does not lose
contacts; they lose links to their own vehicles. The idea that carries over is the ledger of what is
owed back (a vehicle's own failsafe settings, an order that may be running) and a deliberate, logged
way to close it. The concept is kept for links; the track code is not.

## 2026-11-24: Rules reset to version 1

The earlier project's rules version described behaviour (non-delivery task kinds, identification, space
tasking) that FleetMesh does not have, and its logs do not replay here. Starting at 1 says so in
every log. From the first commit, new behaviour is gated on a named `*_SINCE` constant and added to
replay in the same change (docs/design/rules-versioning.md).

## 2026-11-24: The murmur- to fleetmesh- rename is one final pass

Crates, environment variables and paths keep the `murmur-` prefix until the end. Renaming during the
cut would bury real changes in churn and make the catalog hard to review. One mechanical pass before
the first commit, with the build as the check, is cheaper and safer.

## 2026-11-24: The licence is AGPL-3.0-or-later, with reuse notes in RELATED-PROJECTS.md

FleetMesh runs as a networked service for people other than its authors; AGPL keeps modified
versions that are served to others open. It combines with MIT, BSD and Apache code and with GPL-3.0
(GPLv3 section 13). GPL-2.0-only code cannot be combined and may run only as a separate process.
Licences marked "Other" need their text read before code is copied. The notes are in
docs/RELATED-PROJECTS.md, and are not legal advice.

## 2026-11-25: The delivery orders and rules are rules version 1, with no `*_SINCE` gate

The nine order kinds, `Outcome::Hold` and the delivery checks (geofence, energy, payload, weather, separation, drop zone, navigation integrity, lost link) are all version 1: nothing older exists to reproduce before the first commit, so nothing is gated, and replay recomputes every one from the inputs logged by value.

## 2026-11-25: The engagement authority is renamed consequential

The earlier project named the authority to approve the gravest orders for engagement. Delivery has no
engagement; what it has is orders whose effect on people or property is hard to undo (`deliver`,
`abort` over people, `land_now` outside a drop zone). The authority, its roster variable
(`FLEETMESH_CONSEQUENTIAL_OPERATORS`) and the console and log wording say "consequential", which is
also the word the research question uses.

## 2026-11-25: The outside-objects picture and the expended-effects ledger are removed; preload stays as the lost-link reader

Tracks (the picture of objects outside the fleet) and the ledger of expended effects have no delivery
meaning, and keeping them would invite extension. They are gone from the engine, the bridge and the
console. The preload reader stays, as the lost-link reader (`decision/lost_link_read.rs`): it reads the
failsafe settings a vehicle carries and says what the vehicle will do when its link drops, which is
the lost-link idea of the 2026-11-24 entry.

## 2026-11-25: Towers added at Fort Mason and Pier 39; hub-marina moved to the scenarios.md location

The cellular model needs towers to show height falloff and the tower-outage event in docs/design/scenarios.md, so
two were added, at Fort Mason and Pier 39. `hub-marina` moved to the location in
docs/design/scenarios.md (Marina Green side, 37.8057, -122.4370), so the design document and the
hub file agree and the drone and car routes start where the document says.

## 2026-11-25: Weather is served to the console from the scenario's own sampler

The console draws weather from the same sampler the scenario and the rules read (the replayable
archived surface weather), not from a separate feed. What the operator sees is then what the weather
check used, at the scenario clock, and a replay shows the same picture.

## 2026-11-25: The delivery orders and rules shipped as rules v1

See the entry above on the version: the nine order kinds, `Outcome::Hold` and the delivery checks are
rules version 1, shipped with the first working tree, with no `*_SINCE` gate. Later behaviour is gated
on a named constant and added to replay in the same change.

## 2026-11-25: The rename from murmur to fleetmesh is done

One pass, as planned on 2026-11-24: crates (`crates/fleetmesh-*`), package, library and binary names,
`MURMUR_*` environment variables (now `FLEETMESH_*`), the `X-Murmur-Operator`, `X-Murmur-Agent` and
`X-Murmur-Sim-Token` headers (now `X-FleetMesh-*`), the MCP server name, console storage keys and
debug globals, log, pid and stamp file names, Makefile, scripts, Python and docs. There are no
compatibility aliases: nothing is released. Identifiers of the earlier project's bd issues quoted in
comments (for example `murmur-iobd`) are kept as references to that tracker, as is the removed crate
`murmur-contacts` where code cites it. Stored operator credentials are now looked for under
`~/.config/fleetmesh/`, so a credentials file made for the earlier project must be moved there.

## 2026-11-25: Routine deliveries need no person: standing approved delivery points, automation first (rules 2)

The maintainer: "we need to change our procedures to match real life. a human only intervenes for
when an issue comes up." Under rules 1 every delivery to a zone other than a hub pad waited for a
person's confirmation of the zone within the hour, and the dispatcher asked for one per parcel; in
a live harbour-city run every delivery except those to hub pads sat waiting on a person. That
measures a procedure no delivery operator uses.

What real operators do (researched 2026-11-25; the supervision ratios are reported claims, not
FAA primary sources):

- Nobody authorises individual drops; delivery spots are approved ahead of time. Amazon Prime Air
  surveys obstacle-free delivery zones and marks reachable delivery points in them
  (aboutamazon.com/news/operations/mk30-drone-amazon-delivery-packages). Wing delivers to a spot
  the customer chose, about 6 by 6 ft with nothing overhead, changed only when the customer picks
  another (wing.com/news/how-do-you-get-package-from-drone-to;
  techcrunch.com/2023/06/08/how-curbside-pickup-caused-wing-to-rethink-its-approach-to-drone-delivery).
- The aircraft checks the spot itself at drop time. Amazon's MK30 hovers at about 12 to 13 ft,
  detects people, pets, cars and obstacles, and knows when not to deliver
  (aboutamazon.com/news/transportation/amazon-delivery-drones-safety-testing). Zipline's droid
  confirms the zone is clear with downward cameras (spectrum.ieee.org/zipline-drone-delivery).
  Wing's tether releases on its own, and lets go if someone pulls it.
- Remote pilots supervise many flights at once and step in for exceptions (weather, air traffic,
  faults, lost link): Wing up to 50 drones per pilot (aviationweek.com, on its Australia
  expansion), Manna 10 to 50, UPS Flight Forward 3, Matternet 20 with a visual observer.
- The proposed FAA Part 108 creates an operations supervisor (final safety authority) and a flight
  coordinator (tactical oversight); neither approves individual drops
  (pilotinstitute.com/part-108-explained).
- NASA Langley simulations found more than about 4 aircraft per supervisor problematic unless
  automation handles routine tasks (aerospaceamerica.aiaa.org, on NASA's operator limits). That is
  the research question: whether judgement stays meaningful depends on what automation absorbs.

What changed, as rules 2 (`STANDING_APPROVALS_SINCE`, docs/design/rules-versioning.md):

- **Standing approved delivery points.** Each drop zone in the places data carries `approval`:
  `survey` (surveyed and marked in advance), `customer` (the customer's chosen spot) or
  `withdrawn` (approved once, not now); a hub pad is approved with its hub. A delivery to an
  approved point passes the confirmation part of the drop-zone rule with no time limit. Every
  other check of the rule still applies (open or closed, the surface, slope, descent gusts, a
  fresh look at a populated zone). The field is optional, so a zone without it is a new point.
- **`confirm_drop_zone` keeps its lock and changes meaning.** It approves a new delivery point
  (one not on the standing list, or one whose approval was withdrawn), and that approval stands
  from then on; it needs no delivery waiting for it. The dispatcher asks once per zone, before it
  loads any parcel for it, never per parcel or per hour. A deliver held for `DZ_NOT_APPROVED` that
  a person approves approves the point too.
- **The drop-time look is automation's.** Before departure the dispatcher waits while the zone
  does not look clear (as the rule reads the look) and looks again; over the zone the vehicle
  looks again before it lets go (the task layer's drop look: a copter holds its low hover and the
  release is commanded only when the zone is clear; a car or boat looks before it releases; a
  plane cannot hover and keeps the look before departure). Not clear for `depart_wait_s` (120 s)
  at the hub or `hover_wait_s` (120 s) over the zone: the parcel is kept, the vehicle comes home,
  the parcel is tried again after `retry_after_s` (600 s) up to `max_tries` (2), and the person is
  told by an issue alert, not asked.
- **Holds automation waits out** (`WAIT_OUT` in the dispatcher): traffic near the hub or the route
  (`SEP_HUB_ZONE`, `SEP_MARGINAL`, `SEP_TRAFFIC_UNKNOWN`), a stale or unread fact
  (`ENERGY_BATTERY_UNKNOWN`, `WX_UNKNOWN`, `NAV_TRUST_UNKNOWN`, `LINK_PLAN_UNKNOWN`,
  `INPUT_UNKNOWN`) and the look (`DZ_PEOPLE_NEAR`, `DZ_OBSERVATION_STALE`). The dispatcher dry-runs
  each of its orders first (`intent.preview`, so no intent and no inbox item) and waits and tries
  again; after `hold_retry_max_s` (300 s) the order goes as it is and the hold, now persistent, is
  the person's. A hub out of charged packs is waited out the same way. No threshold of any check
  moved and no check was removed: weather beyond limits, navigation integrity, lost link, landing
  away from a hub, abort over people, leaving the approved area, unknown endurance, battery
  reserve, geofences and payload all reach a person as before.
- **Routine noise found in live runs, fixed by automation, not by a person:** a vehicle already on
  a hub's pads loads where it stands (it used to take off and hover over the hub's centre point,
  where loaded drones then held each other's departures with `SEP_HUB_ZONE`); a return to a hub
  that fails on its way out (a mission upload with no answer) is proposed again, up to 3 times,
  before the person is told (`return_failed`); a delivery refused only for a predicted conflict
  with moving traffic is proposed again after a short delay, as the separation rule's own fix
  says, instead of failing the parcel; a parcel within the payload rule's hold band of a drone's
  limit (`PAYLOAD_NEAR_MAX`) is offered to the vehicles that carry it with room to spare (the
  assigner's input is filtered, each group logged as its own plan; a parcel nothing free carries
  so is offered to all, and the person judges the margin); and a multirotor climbing or descending
  in place no longer reads as `NOT MOVING`.
- **Issue alerts.** An issue automation raised (`drop_not_clear`, `zone_not_approved`,
  `return_failed`, and the
  navigation monitor's action) says what happened, what automation already did and the response it
  recommends; the console words the same three parts for the alerts the bridge derives from a
  vehicle's state, and a held order's card says what automation did before it asked.
- **The old procedure is kept as a comparison setting**, because the study's question is the
  attention cost of each design: `[rules.delivery.dropzone] standing_approvals = false`
  (`sim/policy-strict.toml`, run with `POLICY=sim/policy-strict.toml`) restores rules 1's per-hour
  confirmation of every zone but a hub pad, with the dispatcher asking once per zone while one is
  pending. The drop-time look stays automatic in both. `eval/arms.py`'s arms are proposer and
  supervisor pairs with no policy dimension, so the setting is a policy file, not an arm.

The version moves from 1 to 2 and the policy schema from 1 to 2 (the new key). Replay decides a
rules 1 log under rules 1 (it needs `--rules 1` or `--min-rules 1`, as any older log does) and a
rules 2 log under rules 2; `RULES_MARKERS` flags a log that claims rules 1 but carries a field rules
2 added.

## 2026-11-25: Other traffic: automation keeps clear, a person hears of issues (rules 3)

The fleet shares the airspace and the water with traffic it does not control, and knows it only as
a private company could: what its receivers hear and its own sensors see (docs/design/traffic.md).
The maintainer's direction: the fleet should keep track of that traffic "as well as a private
company would be able to", and "a human only intervenes for when an issue comes up".

What changed, as rules 3 (`TRAFFIC_SEPARATION_SINCE`, policy schema 3):

- **The picture is a decision input.** The bridge runs the tracker on the simulator's feeds (or a
  real receiver's) and every delivery decision reads the picture, cut by the engine to the tracks
  and receivers its checks read and logged by value, so replay decides on the same picture with no
  simulator and no tracker. The picture is never the simulator's truth.
- **The separation rule keeps clear of it** (delivery-rules.md section 8.1): well clear of manned
  aircraft (draft 2,000 ft and 250 ft, ASTM F3442; they have the right of way, 14 CFR 107.37),
  clear of other drones and of anything that broadcasts nothing (a bearing-only line as the line it
  is), a boat's 0.25 nm closest approach to a ship under way and out of its way ahead (COLREGs rule
  9), along the tracker's prediction widened by its uncertainty. Every conflict is a hold, never a
  refusal: waiting or another route clears it, and a person may judge their own order.
- **Automation handles it.** The traffic holds are waited out and never sent to the inbox: a
  departure waits on the ground and is offered a route round the conflict after 30 s; a delivery or
  return under way is held where it is (a drone hovers, a boat stands off) or, when the traffic
  comes at it, re-planned round it. Each response is logged.
- **A person only for issues**, each saying what happened, what automation did and what to do: a
  conflict that outlasts automation's five minutes (`traffic_conflict`), an aircraft that broadcasts
  nothing near a drone (`traffic_noncooperative_near`), a broadcast the company's own sensors
  contradict (`traffic_broadcast_disagrees`), the picture degraded (`traffic_picture_degraded`, a
  feed or receiver not heard; automation holds the departures that depended on it).

Why holds and not refusals: a predicted conflict minutes or a minute ahead is uncertain and is
cleared by time or a detour; a refusal would make the dispatcher re-propose (an intent each time)
and would leave a person no way to judge their own order. Why the vertical cap and the 60 s horizon:
the tracker's constant-velocity prediction grows as a random walk (1.5 km horizontally and 0.8 km
vertically after two minutes for ADS-B), which would ground every drone under any aircraft; real
detect-and-avoid looks about a minute ahead. Why under-way vehicles may leave a minimum: holding a
drone next to the thing it should leave is the worst response.

Found and fixed on the way (the tracker's integrity check flagged about 15 ordinary vessels in every
harbour-city frame against a boat's radar: vessels moving alike, ferries on one route, sailboats in
one wind): a broadcast is tied to what a sensor sees by motion only when it moves fast (an
aircraft), else only when the sensor's view split off from it; a marine radar is compared only
within half its range for the vessel's class; offsets under 500 m (1 km for a slow mover) are the
sensors' own errors. The detect-and-avoid's ranges by target size were shorter than its reach, so
aircraft it saw became bearing lines of their own. To make a wrong broadcast near the waterfront
visible for what it is, harbour-city and wind-event switch on the Pier hub's ground radar (a
bearing alone cannot contradict a position); its returns at the surface are vessels.

Scenario data, fixed rather than any threshold: harbour-city's cars load at their own kerb on
Marina Boulevard (routed to the drones' hub point they drove through landed drones and the
separation rule refused them), and both Pier 39 scenarios' boats at a dock in the marina basin (the
hub point is on the pier, and the domain rule refused a boat's pickup on land). A scenario's demand
may give a hub its own zones (`zones_by_hub`).

Rules 1 and 2 logs replay under their own rules; `RULES_MARKERS` flags a log that claims rules 2 but
carries the picture or the traffic figures.
