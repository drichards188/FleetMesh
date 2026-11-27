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

## 2026-11-25: The fleet is organised by home base, and each vehicle goes back to its own (rules 4)

The maintainer: "There also needs to be a way to organize the fleet. It's a mess of individual
nodes right now. Look up how these nodes would actually be organized and implement that." The
console listed 54 vehicles flat (grouped only by domain) and drew 54 icons, piled at each hub. The
dispatcher sent every vehicle back to the hub nearest it: in a live harbour-city run 13 of 22 returns
went to another base than the vehicle's own (drones to the car kerb, a plane to the boat dock, boats
towards a land hub the domain rule refuses).

How real delivery fleets are organised (researched 2026-11-25):

- **Bases.** Every vehicle belongs to a home base, and the base is the unit of organisation. Wing
  runs "nests" at stores: an enclosure where drones park and charge, with an autoloader where staff
  load orders (dronexl.co/2026/10/06/wing-walmart-bentonville-drone-nest-permit;
  wing.com/news/wing-and-walmart-announce-world-s-largest-drone-delivery-expansion-ever). Zipline
  runs hubs serving about a 10-mile radius in hub-and-spoke mode, or a dock-to-dock network in which
  its software sends each aircraft to the dock that suits it, for charging or a pickup
  (freightwaves.com/news/zipline-drone-delivery-just-got-a-massive-upgrade). Robotaxi fleets run
  depots where vehicles are cleaned, inspected and charged to be "deployment-ready"
  (axios.com/2026/04/29/robotaxis-depots-land-infrastructure-charging).
- **Fleet software's hierarchy.** DJI FlightHub 2 organises an organisation, then projects, then
  devices and docks, and its "grid deployment" dispatches the most suitable nearby dock for a task
  (enterprise.dji.com/flighthub-2/faq; enterprise-insights.dji.com/blog/new-update-flighthub2-april).
- **Operational state matters more than identity.** What operators watch is uptime: how many
  vehicles are ready, out on a task, returning, charging or swapping, or out of service, per base.
- **Supervision.** A central operations centre watches many sites; operators hand vehicles off to
  one another, and NASA's m:N work studies dividing vehicles among operators
  (skydio.com/remote-flight-deck; ntrs.nasa.gov/citations/20240001820;
  wing.com/news/wing-unveils-first-of-its-kind-remote).

What changed (docs/design/fleet-organisation.md):

- **Home base.** Each vehicle has one: the hub its fleet file names for its unit (`home_hub`,
  carried by the manifest), else the hub it was launched at. harbour-city names its four (the
  Marina's drone pads and car kerb, the Pier's drone pads and boat dock); a scenario may group bases
  into sites (`[[site]]` in its dispatch file) for the console. Sites are display only.
- **Operational state.** The bridge derives one state per vehicle from what it already knows (the
  dispatcher's jobs, the orders running and waiting, link, navigation, battery, alerts and issues):
  needs attention, offline, out of service, holding, ready, loading, outbound, delivering, returning,
  charging, on another order. `GET /api/fleet` serves them with each base's counts. There is no
  maintenance state, since nothing in a run takes a vehicle in for service. Read-only, never logged.
- **Console.** The Fleet list puts what needs a person first, whatever its base, then each site's
  bases with their counts by state, each opening to its vehicles by state; filters by type and
  state. On the map, the vehicles parked at their own base collapse into one marker per base (per
  site zoomed out) with its counts; a click or zooming in opens it; anything away from its base,
  under way, in trouble, selected or followed stays on its own.
- **Dispatch, rules 4 (`HOME_BASE_SINCE`).** The assigner costs a trip's way back to the vehicle's
  home base (the nearest hub when the battery would not cover the way home), so the vehicle based
  where the parcel waits wins over a visitor; the plan's input logs each candidate's `home`, and
  replay re-runs the assigner under the log's rules. The dispatcher sends a vehicle home after a
  delivery when a dry run says the way home would go at once (else the nearest hub, as before), and
  sends a visitor idle for 60 s at another hub home. No check, threshold or policy key changed; the
  policy schema stays 3. In the headless harbour-city run every return (15 of 15) went home, and the
  log replays.

Rules 1 to 3 logs replay under their own rules (`--min-rules`); `RULES_MARKERS` flags a
`dispatch.plan` that carries a `home` under a header that claims rules 3.

## 2026-11-25: Other traffic is drawn in standard symbology, with civil identities only

The maintainer asked for traffic icons "like murmur had for its contacts" so that a user can
"quickly and easily understand what they are seeing"; the console's outline pictograms were hard to
read at a glance. Each track is now a standard map symbol (MIL-STD-2525D / APP-6D number codes,
drawn by milsymbol, MIT, as the earlier project drew its contacts): a frame whose colour and shape
say how far it is identified, and a glyph for what kind it is, from the civilian function codes
only and not over-specific (every airplane is civilian fixed wing). Why the standard rather than
pictograms of our own: it keeps who-is-it apart from what-is-it, it is learnt once and read on
other traffic displays, and its frames stay legible at the sizes a busy map allows. Our own
vehicles keep their round delivery icons, so the fleet and the traffic never look alike.

Only the neutral, unknown and pending identities are used, never friend or hostile (or assumed
friend or suspect): neutral (green) for traffic that broadcasts (ADS-B, AIS, Remote ID) and says
who it is, unknown (yellow) for what only our sensors see or what broadcasts no identity, pending
(hollow) while a track is tentative; a coasting or stale track's frame is dashed (status
anticipated). The traffic is the public's: a side would claim what a delivery company cannot know
and has no business implying. The console's words avoid the standard's terms: the Key says
"identified, broadcasting" and "unidentified or not broadcasting". Detail: docs/design/traffic.md,
section 9.

## 2026-11-25: Hubs sequence their own traffic, and a normal hour runs with no person (rules 5 and 6)

The maintainer: routine work runs without a person, and "a human only intervenes for when an issue
comes up". Read-only checks of a live harbour-city hour (rules 3) found 19 parcels delivered, 14
failed with no reason recorded, 116 ready and waiting; the 4 boats never moved; routine returns and
pickups in the inbox ("18 and 1 meet within 100 m of a hub at +300 s: a person decides who waits");
pickups held for `PAYLOAD_NEAR_MAX`; and a `traffic_picture_degraded` alert from the start. A
measured hour under rules 4 (this tree before the change, 30 minutes) gave 16 delivered (11 by drone,
5 by plane, none by car or boat), 9 failed with no reason, 55 ready, and 29 orders sent to the inbox:
18 pickups held for `PAYLOAD_NEAR_MAX` (most also `SEP_HUB_ZONE`), 6 car pickups held for
`WX_FLOOD_RISK`, 3 returns held for `SEP_HUB_ZONE`, 2 deliveries for `ENERGY_MARGIN_THIN`; 13 of 54
vehicles needed attention, the 4 boats among them.

The main causes found, and what changed:

- **Every return landed on the hub's own point**, on top of the vehicles there, and a drone more
  than the hub's radius from that point flew to it to load and then hovered there at cruise height
  while its delivery waited; the separation rule then saw every departure and arrival "meet" it.
  From rules 5 (`HUB_SEQUENCING_SINCE`) every vehicle parks on its own stand at its base (where it
  was launched), lands there again, and a visitor gets a free pad; a pickup on the stands loads where
  the vehicle stands; the order rule takes a point within the hub's radius.
- **The prediction of the fleet's own vehicles.** A vehicle with no route was carried along its
  velocity for 300 s (a parked or hovering drone's noise through the hub; a delivering drone 3.6 km
  past its zone, into routes it would never cross), and the uncertainty grew 0.5 m/s for the whole
  horizon even between two vehicles standing still. From rules 5 the bridge gives the rule each
  vehicle's dispatched route; a vehicle flies it at cruise and rests at its end, a still one stays
  where it is, the uncertainty stops growing once both of a pair are at rest, a drone with a route
  to fly counts at its cruise height (one lifting off beside a car is not a vehicle on the ground),
  and a vehicle parked on a hub's stand is the hub's for an order on, leaving or reaching that hub
  (cars 6 m apart in their kerb bays held each other's departures for as long as the neighbour
  stayed parked, and a car coming home was held by the cars parked at its kerb).
- **Sequencing is automation's.** A hub-zone meeting still holds `SEP_HUB_ZONE` (no minimum moved);
  its words say the hub sequences it, and the dispatcher waits it out, with `SEP_MARGINAL` and a
  refusal only for another of our vehicles' route (`SEP_ROUTE_CONFLICT`, whose own fix is a short
  delay), never sending the order to a person and never spending a parcel's try on it. A routine
  return never waits for a person: whatever automation waits out is waited out. A wait over 5
  minutes is an issue alert (`fleet_conflict`, `return_held`) while automation goes on waiting. Why
  not a person: who lands or leaves first at a hub is a vertiport's or a nest's sequencing in real
  operations, never an operator's decision.
- **Payload room before assignment.** A parcel no free vehicle carried within the hold band was
  offered to all, a drone took it, and its pickup waited for a person. The dispatcher now offers a
  parcel only to vehicles with room in the wind now (the payload rule's own derate), carries it
  across a site to a base that has one (the Marina's near-limit parcels go by car from the kerb), or
  keeps it waiting; one no vehicle of its site can carry within the margin for 10 minutes is one
  issue per parcel (`parcel_no_vehicle`), never a pickup to approve. No payload threshold moved.
- **Cars never worked.** The weather rule's flood check read every sample the world had, and the
  boat dock's seabed (4.3 m under a 1 m tide) held every car's order `WX_FLOOD_RISK` for a person.
  From rules 6 (`FLOOD_ALONG_ROUTE_SINCE`) a car's flood check and a boat's depth read the samples
  where the vehicle goes. And a car or a boat that reached its drop zone waited for ever for a
  gripper release that only comes after its mission: the task layer waited for an aircraft's release
  item on vehicles whose missions carry none.
- **The boats never moved.** They were not moored, so the tide carried them off their dock (100 m
  in five minutes) and out of the hub; their dock zones stood on land (Sausalito's 3 m up, Angel
  Island's 85 m up the hillside) and the straight run from the dock crossed Pier 39. The boats are
  tied up at berths in open water off the pier (moored again when they disarm at their berth), the
  zones are at Sausalito's waterfront and Angel Island's East Garrison pier, and a boat parcel is
  due in 90 minutes; coastal-boat's Sausalito mooring stood on the hillside too.
- **Demand that fits the bases**: the Pier's drone pads took 4 kg parcels nothing there can carry,
  and a tenth of the demand went to four 3 m/s boats that make one trip an hour. A base's parcels are
  now drawn within what its vehicles carry (`mass_by_hub`), the dock takes 1 in 40, the kerb 7 in 40.
- **Failure reasons.** Every attempt that falls short is kept on the parcel (stage, how its order
  ended, the refusing checks' codes and words or the task layer's failed step); a failed parcel
  carries the end that made automation give it up, `GET /api/parcels` serves it and the console's
  Parcels panel shows it.
- **The picture's start.** For its first 60 s of the scenario's clock a picture not yet whole is
  starting, not a fault (departures that need it wait, no one is told); re-reading the simulator's
  `/sensors` each ten minutes no longer resets which receivers were heard (every receiver read "not
  heard since the start" for a second at 1200, 1800 and 3000 s); the issue clears itself when the
  picture is whole again.
- **A vehicle not heard is left to its own failsafe**: the dispatcher proposed a vehicle with a lost
  link three returns in three seconds, each voided, and a delivery on stale facts that spent its
  parcel's last try.

Rules 4 and older logs replay under their own rules; `RULES_MARKERS` flags a log claiming rules 4
that carries the fleet's routes. Rules 5 and rules 6 are separate bumps (one change, one bump,
rules-versioning.md). Detail: docs/design/delivery-rules.md sections 7 and 8.2, dispatch.md,
traffic.md section 10, fleet-organisation.md section 6.

## 2026-11-26: The AI watch officer triages what reaches the person (rules 7)

The maintainer: deterministic code raises the alerts and holds, the AI watch officer triages them,
and only what it cannot confidently handle reaches the person's Inbox, with its recommendation;
the person audits everything else in the order log. The question this serves is the project's own:
whether one person's judgment over consequential decisions stays meaningful at scale. Management by
exception (rules 2 to 6) already keeps routine work from the person; at thousands of vehicles the
exceptions alone are more than one person can read. The officer is the second filter, and phase 8
measures it as an arm against the person alone.

What changed:

- **What the officer may handle** is a structural test, not the model's confidence: the response
  is already decided (by the policy, or the conservative default), it is cheap to undo, it accepts
  no new risk for the fleet or anyone outside it, and its inputs can be trusted. The policy's
  `[officer.handle]` says which alerts it may settle and which of the dispatcher's held orders it
  may turn down; its own doubt only sends more to the person, never less.
- **The officer approves nothing**, as no agent does. Turning down a held order is the dispatcher
  withdrawing its own proposal at the officer's word (bd fleetmesh-ic4.2), never an agent deciding
  another's order, and never a safety order (`land_now`, `abort`, `return_to_hub`, `hold`).
- **Nothing waits on a silent officer.** Each item's deadline is the bridge's, set by the policy's
  `[officer.deadline_s]` when it appears (an order reaches the person at least `before_expiry_s`
  before its approval expires); an item not triaged by then reaches the person marked untriaged,
  and every item it holds does as soon as it makes no MCP call for `absent_after_s`. An emergency
  landing, an abort and the most time-critical alerts never wait for it (`[officer.bypass]`).
- **No sampled review** (the maintainer's choice): what the officer handled is listed in
  `GET /api/triage` and logged, for the person to audit by hand; phase 8 measures its misses
  against the scenarios' reference decisions.
- **Rules 7** (`OFFICER_TRIAGE_SINCE`, policy schema 4): `[officer]` in the policy, and the
  `triage.watch`, `triage.routed`, `triage.verdict`, `triage.released` and `triage.closed` events.
  No decision's checks or routing changed: an order the officer holds stays `pending_approval` and
  expires as before. Replay re-runs each item's routing on its logged inputs, checks that every
  verdict is the policy's officer's and within `[officer.handle]`, and flags an item kept from the
  person past its deadline or past the officer's going off watch. A run under older rules logs no
  `[officer]` (`Policy::as_logged`), and a triage event in such a log is flagged.

Detail: docs/design/watch-officer.md.

## 2026-11-27: The watch officer runs from the menu, and a turn-down is the dispatcher's own

Building on rules 7 (the entry above), with no rules change:

- **Turning an order down** (`triage_item` `turned_down`) works within `[officer.handle.turn_down]`:
  the dispatcher withdraws its own held pickup or deliver with the officer's reason, and its next
  proposal waits the scenario's `retry_after_s` with the turn-down counted as one of the parcel's
  tries, so a parcel that never improves fails after its tries (a `delivery_failed` issue) rather
  than being offered for ever. Reroutes are left out: the dispatcher's jobs hold pickups and
  deliveries only. The gust code the policy names is the soft band's, `WX_GUST_LIMIT_MARGINAL`.
- **The deadlines are 60 s for a held order and 90 s for an alert** (`sim/policy.toml`): the first
  live officer's turn with its tool calls took 4 to 8 s, but a busy wake takes longer, and an
  order still reaches the person at least 15 s before its approval expires.
- **The officer is started by `scripts/up.sh`** with `FLEETMESH_OFFICER=claude|codex`
  (`make start SCENARIO=... AI=claude`, or the menu's AI choice, offered only for a CLI installed
  here): its CLI and login are checked before anything starts, it starts once the bridge answers,
  and it is not among the processes whose exit stops the stack, so an officer that stops (a
  budget, a usage limit) leaves the person with every item. Its CLI never inherits a parent Claude
  Code session's markers. The menu no longer offers a faster clock (it sped tidewater's clock, not
  the deliveries: bd fleetmesh-uef).
- **A deadlock fixed.** Reading the Inbox took the triage lock and then the decision lock while
  the decision loop took them the other way round, and a live bridge hung (it would not stop).
  The decision lock is now always taken first; a test reads the Inbox while the loop ticks.

## 2026-11-27: Scenarios start on their event

- **The wind-event arc runs in its first quarter hour.** Its gusts pass the drones' soft limit at
  about 55 s (was 22 minutes), the hard limit at about 8 minutes and fall back under the soft limit
  at about 15, and the run is 25 minutes (was 70): a person started it and then waited 20 minutes
  for anything to happen. Its other traffic's issues come within the arc too.
- **harbour-city's scripted events moved 9 minutes earlier** (the street fair from a minute in, the
  crowd at a Fort Mason zone at 6 minutes), its hour kept; the medical helicopter moved 4, to 6
  minutes, when drones are out for automation to keep clear of it.
- **coastal-boat is unchanged**: its events follow its boats (bd fleetmesh-2m9).
- A test keeps the first two starting on something within two minutes.

## 2026-11-27: A hub is a mark of its own, with its details

- **A hub is a house on a short stem over its point**, our fleet's navy and blue so it never reads
  as other traffic, a shape nothing else on the map has (as an installation's frame sets a base
  apart from units in military symbology). Its roof is the colour of its state: blue while all is
  well, amber while the wind is in the soft band or vehicles hold, red while grounded or someone
  is needed, grey while closed. The vehicles parked there collapse into it (their number on its
  walls), a red count on its eave says how many of its own need the person, and beside it one
  label: its name and a line, what needs the person first ("2 need you · Marginal: gusts 12.5 m/s
  · 18 holding"). Every hub has its mark at every zoom (before, a hub was a blue disc that read as
  a selection, under a second, same-named label for its parked vehicles). The stem keeps the
  vehicles parked on the point from covering it.
- **A click on a hub opens its details**: a few plain sentences first (whether it can fly, what
  needs you, what its vehicles and parcels do), then who needs you and why (each a link), the
  wind against the limits as a gauge, its vehicles by state, its parcels (a link to the list at
  the hub), pads and packs. Showing its parked vehicles one by one moved there from the mark's
  click. A site's mark, zoomed out, zooms in to its hubs.
- **The bridge says each hub's wind** (`weather` of a base in `GET /api/fleet`): the gust and mean
  wind there and the limits of the strictest platform based there (the weather rule's own
  `weather_limits`, now public). Display only, so no rules version.
