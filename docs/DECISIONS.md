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
