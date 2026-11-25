# Links and GNSS: what exists

Status: describes the code as ported on 2026-11-24 (`crates/fleetmesh-linkemu`,
`crates/fleetmesh-standins`, `crates/fleetmesh-space`, `sim/links.toml`, `sim/platforms.toml`). This is
a reference for implementers of the bridge, the rules and the console, not a plan to build
something new. Two parts of the ported code are placeholders marked `FLEETMESH-REVIEW`; section 7
covers them.

## 1. Purpose

Delivery vehicles lose links and lose position. The rules ([delivery-rules.md](delivery-rules.md),
sections 9 and 11) and the operator's picture both depend on what the emulator and the stand-ins
actually do. This document writes down:

- the link classes and which delivery platform uses which;
- the cellular model and its height falloff;
- automatic main and backup failover and its parameters;
- the GNSS interference event kinds and what a receiver does with them;
- the two placeholders;
- what the bridge should show an operator.

## 2. The link emulator in one page

The emulator (`fleetmesh-linkemu`) sits between each vehicle and the bridge, one UDP socket and task
per vehicle. Packets are never modified. Per packet and per direction it decides: blocked (no
airtime used), queued behind a bandwidth cap, lost (a Gilbert-Elliott burst chain plus scripted and
range, height and terrain loss), delayed (latency plus uniform jitter), or duplicated. It uses
tidewater's DEM for terrain line of sight, with a profile per 250 m cell and altitude band, sampled
once a second, so the packet path never waits for HTTP (`/stats` counts packets decided before the
first answer as `unknown_pkts`).

A vehicle's profile is chosen in order: an explicit `[platform_profile]` entry, then for SATCOM the
platform's `space_class` through `[space_class_profile]`, then the platform's `link` class from
`sim/platforms.toml`. Profiles may `extends` another. Everything numeric in `sim/links.toml` is a
first-draft value; those marked GUESS are invented starting points.

The emulator reports per link on `GET /stats/<uid>` and a summary on `GET /stats`
(default `127.0.0.1:8766`). Drop causes are named: `loss`, `event_loss`, `jam`, `cut`, `range`,
`terrain`, `gate`, `queue`, `no_peer`, `socket`, `no_sat`, `handover`.

## 3. Link classes

| class | what it models | key numbers (first draft) |
| --- | --- | --- |
| `los` | digital datalink to a ground station mast | loss 0.5%, 15 ms, 500 kbps; ok to 15 km, max 25 km; terrain ramp 20 m; lost-link timer 5 s |
| `mesh` | relay network, several hops | loss 2%, 25 ms, 300 kbps; ok to 5 km, max 12 km; timer 10 s |
| `cellular` | LTE or 5G from tower sites (suburban) | loss 1%, 60 ms plus or minus 20, 5 Mbps; ok to 3 km, max 8 km; height ok 120 m, max 300 m; timer 10 s |
| `cellular_urban` | short cells, dense sites | extends cellular: ok 1 km, max 2.5 km, loss 2%, 70 ms |
| `cellular_rural` | long cells, few sites | extends cellular: ok 8 km, max 25 km, 50 ms |
| `satcom` (GEO, WGS-class) | geostationary bent pipe | loss 1%, 300 ms, 256 kbps; timer 120 s |
| `satcom_ku_geo` | commercial Ku-band GEO | extends satcom |
| `satcom_leo` | Starlink-class LEO | loss 0.3%, 25 ms static, 2 Mbps; handover outage 100 ms; timer 30 s |
| `satcom_iridium` | Iridium-like, narrow | 700 ms, 22 kbps; handover outage 250 ms; timer 120 s |
| `acoustic`, `acoustic_surface`, `tether` | underwater and cable classes | not used by any delivery platform; left in place until pruned |

Range loss is 0 up to `range_ok_km`, then rises as `((d - ok) / (max - ok))^exp` to 1 at
`range_max_km`. Terrain `ramp` makes loss rise from 0 at a grazing ray to 1 at `terrain_ramp_m`
(20 m) of obstruction; `block` drops everything. The lost-link timer in each profile is written into
the vehicle's parameters by `sim/swarm.py` (`FS_GCS_TIMEOUT`, or `FS_LONG_TIMEOUT` on a plane); the
emulator itself does not use it.

### Platform mapping (`sim/platforms.toml`)

| platform | autopilot | main link | backup link |
| --- | --- | --- | --- |
| `delivery_drone` | ArduCopter | cellular | los |
| `delivery_car` | Rover, car frame | cellular | mesh |
| `delivery_boat` | Rover, boat frame | cellular | satcom_leo |
| `delivery_plane` | ArduPlane | los | satcom_leo |

Two inconsistencies for the maintainer (section 9): `sim/links.toml` says the plane is beyond line
of sight and on SATCOM, while `platforms.toml` has it on `los` main with SATCOM as backup; and the
plane has no `space_class` in the catalog, so its SATCOM profile comes from the default
`[space_class_profile]` choice, not from the catalog.

### Ground stations and towers

Stations are `[[ground_station]]` tables (name, lat, lon, `alt_m` or `"dem"`). For radio profiles
(a range model, a terrain check or a propagation delay) the link goes through whichever station
gives the best path at that moment: fewest packets lost, then nearest, then first listed. A switch
between stations is immediate and costs nothing. For cellular, each tower is a ground station, so
coverage is the union of tower footprints. SATCOM links use no station. The shipped file lists the three
towers of the Golden Gate delivery scenarios (Crissy Field, Fort Mason, Pier 39).

## 4. Cellular and height falloff

Cellular antennas point down. A vehicle's loss from height is 0 up to `height_ok_m` (120 m, the
usual drone ceiling), rising linearly to 1 at `height_max_m` (300 m), measured above the carrying
tower's ground (`height_loss` in `model.rs`). It is combined with range loss as independent
effects: `loss = 1 - (1 - range_loss)(1 - height_loss)`, and then terrain loss on top. Consequences
for delivery:

- a delivery drone at its typical 80 m has full strength; at the 120 m ceiling it is at the edge;
- a plane at `typical_alt_m` 120 m is already at the limit, which is part of why it carries `los`
  as its main link;
- a boat at sea level has no height loss, only range and terrain.

Congestion in dense areas has no profile parameter. Use a scripted event on an area and class
(example in `sim/links.toml`: `downtown congestion`, extra loss 5% and 40 ms over a 1.5 km circle).

Predicted coverage (`coverage.rs`, `route_answer`) uses the same functions as the packet path. It
gives up, degraded (loss at least 0.2) or down (loss at least 0.95 or fully blocked) along a route
sampled every 50 m, and walks the vehicle's lost-link timer heartbeat by heartbeat at the vehicle's
speed to say whether the failsafe would trip. It does not predict scripted events, SATCOM
handovers, queueing or latency. The `lost_link` rule uses this answer.

## 5. Main and backup failover

Each vehicle with a `backup_link` has two radios. Failover is automatic and on the vehicle's side:
it follows from position, scripted events and the clock, never from an operator, and the same
scenario and seed give the same switch times. The parameters are read from the **main** link's
class:

| parameter | default | meaning |
| --- | --- | --- |
| `failover_loss` | 0.5 | switch to backup when the main link's loss, averaged over the window, is above this, or the main link is unavailable (out of range, above its height limit, blocked) |
| `failover_window_s` | 3 | averaging window |
| `failover_hold_s` | 10 | return to main only after it has been healthy for this long |
| `failover_gap_s` | 2 | each switch is a gap in which nothing passes (cause `handover`) |

Mechanics from `link.rs`: the main link's expected loss is sampled every 100 ms and averaged over
the window; health means the link carries traffic and the average is at or below `failover_loss`.
A switch to the backup happens only if the backup could carry anything (`own_health` is up), so a
dead backup does not cost a gap. A switch back needs the hold time. While a switch gap is open the
state does not change. The backup radio has its own random stream. Time on each link and the
number of switches are reported in the stats.

What failover does not cover: scripted events act on the vehicle, not on one radio, so a comms
interference circle (an `[[event]]` with a circle and `loss` or `cut`, or a circle with neither and
no `gnss` table, which defaults to loss 1.0) degrades both paths. Failover helps with range, height and
terrain, not with an interference zone placed over the vehicle (scenario 3 relies on this to show
that cellular-to-SATCOM failover is not a defence against everything).

An illustrative sequence (times invented) for a boat leaving cellular range and coming back:

```text
t=0   cellular loss rising with range      average 0.2
t=41  window average passes 0.5           main unhealthy, backup satcom_leo is up
t=41  switch: gap 2 s, cause "handover"
t=43  traffic on satcom_leo (25 ms, 2 Mbps), lost-link timer now 30 s
t=190 back in cellular range, healthy      hold timer starts
t=200 hold elapsed                         switch back, gap 2 s
```

Failover is draft: 50% loss over 3 s, a 10 s return hold and a 2 s gap are chosen to be quick
enough that a person is not in the loop and slow enough not to flap. See DECISIONS.md.

## 6. GNSS interference

A vehicle's GNSS receiver is modelled in `crates/fleetmesh-standins/src/gnss.rs`. Three inputs combine
each physics tick: satellite geometry, an elevation mask (`FLEETMESH_GNSS_MASK_DEG`, default 5 degrees),
and interference events. Interference exists only as effects on the fleet's own receivers; no
vehicle is ever a source (decision 3 in DECISIONS.md).

Events are `[[event]]` tables with a `gnss` sub-table in the same file the link emulator reads
(an event with a `gnss` table and a circle leaves the datalink alone unless it also gives `loss` or
`cut`, so a GNSS jammer does not by default cut the command link)
(`sim/link-events/*.toml`, or `FLEETMESH_LINK_EVENTS`). A GNSS event selects by `circle`, `class`,
`uid` or `uids` (not by `profile`, `platform` or `domain`, which the stand-ins do not see). Time is
`at_s` plus `duration_s`, or `at_scenario` with `duration_s` or `until_scenario`.

| effect | fields | what the receiver does |
| --- | --- | --- |
| `jam` | circle (required), `power_w` (omitted: no fix anywhere in the circle), `height_m` (default 10) | noise raises J/S at the antenna by free-space loss, jammer EIRP, antenna pattern, a 4/3-Earth radio horizon. Each satellite's C/N0 falls; tracked while C/N0 stays above 28 dB-Hz, reacquired only above 33. Four tracked is a 3D fix, three a 2D fix, fewer no fix. `h_acc` is HDOP times a UERE that grows as C/N0 falls |
| `spoof` | `walk_mps` (default 1), `bearing_deg` (default 90), `max_offset_m`, optional circle | inside the footprint the receiver is captured. Reported position walks off truth at `walk_mps` on the bearing; satellite count, DOP and fix type stay good, so nothing in the flags shows it. The EKF follows the false position |
| `degrade` | circle (required), `severity` 0..1 (default 1) | cause-free loss of quality: J/S rises by severity times 60 dB. About 0.5 keeps a fix with worse accuracy; about 0.7 and above loses it |

Navigation effects (`nav.rs`, ArduPilot's EKF3 behaviour): with fix, fused GPS; on fix loss the
filter keeps claiming absolute position for 10 s if velocity aiding exists (7 s if not), then
dead-reckons (a plane on airspeed) or loses horizontal position (copter, rover, boat: no horizontal
position, and constant-position mode after 15 s). After the fix returns (10 s) the EKF resets onto
it. When spoofing ends the true fix is more than 25 m from the estimate, so the EKF rejects GPS for
10 s (a glitch) and then resets. Stand-ins keep a truth log of the real position for evaluation; the
truth is never on the MAVLink wire.

Firmware reactions (`failsafe.rs`): a Copter's EKF failsafe lands (or holds altitude); a Rover's
holds; a Plane flies on. The `FS_EKF_*` parameters set these.

## 7. The two FLEETMESH-REVIEW placeholders

### 7.1 SATCOM fixed parameters (`crates/fleetmesh-linkemu/src/sat.rs`, `RealSpace::load`)

The orbit model (catalog, element sets, propagation of named satellites) was cut. `RealSpace::load`
always returns an error ("no satellite availability model in this build"), so every SATCOM link
falls back to its static profile (`SatMode::Static`, reported in the stats as the reason). Effects:

- SATCOM never drops for lack of a satellite (`no_sat` never fires) and has no handovers, so
  `handover_outage_ms` is unused;
- latency is the profile's `latency_ms` (LEO 25 ms), not the slant range plus network allowance;
- availability does not depend on the position or the scenario clock;
- `sim/space/` does not exist, and `fleetmesh-space fetch` is not part of setup.

A delivery SATCOM availability model should replace it: coverage by region and sky view, not
orbits of named satellites, with outages that the scenario can script (a terminal blocked by
a mast, rain fade) and that can be reproduced under replay. Until then scenario 3's SATCOM can
be made to fail only by a scripted `cut` or loss event on the class.

### 7.2 GNSS fixed sky (`crates/fleetmesh-standins/src/gnss.rs`, `Env::load`)

The GPS geometry model was cut, so there is no sky table. Every receiver sees the legacy sky: 14
satellites in view, HDOP 1.0. Only the scripted events change the fix. Effects:

- no urban canyon, no low-satellite or high-DOP periods unless an event scripts them (the
  `degrade` effect is the stand-in for an urban canyon);
- GPS only, as before; no multi-constellation counts;
- altitude does not change the sky.

A delivery model of GNSS availability (sky view from building height and street width, or from
terrain) should replace it. `fleetmesh-space` still contains the pure parts that would support it:
`gnss.rs` (dilution of precision from geometry), `orbit.rs` (SGP4), `access.rs`, `timemap.rs` and
`scenario_clock.rs`. The catalog and the `Space` queries are gone.

## 8. What the bridge should show the operator

Everything below exists as data already or follows directly from it; the console should present it
in operator language, with ages, and not as raw stats.

**Per vehicle (detail panel and on the inbox card when relevant):**

- the link now: class of the radio carrying traffic (cellular, los, satcom_leo, mesh), whether it is
  the main or the backup, its state (up, degraded, down by the 0.2 and 0.95 loss thresholds the
  bridge badges already use), and the serving station name;
- failover: on backup since when, number of switches, time on each; a switch in the last minute
  gets a one-line note ("switched to satellite at 14:02:11, back in about 10 s after cellular
  recovers");
- what the vehicle does if the link drops: the failsafe action and the seconds on the timer (10 s
  cellular, 30 s LEO), and the predicted coverage gap ahead on its route (from `route_answer`);
- latency and loss in plain words ("satellite, about 25 ms, 0.3% loss"), and the age of the last
  heartbeat;
- when a command is queued because the link is down, say so, with the expiry;
- navigation: the trust state from the navigation rule, the fix type, satellites tracked, HDOP,
  `h_acc`, the age of the position, and whether the EKF is dead reckoning. A vehicle that is
  untrusted shows its last good position with an uncertainty circle that grows with time and
  never a confident marker.

**Map:**

- tower footprints (range ok and max, with height falloff as a ceiling), as a coverage layer
  separate from the weather layer;
- interference areas only as symptoms: the console must not show where a jammer is, since an
  operator would not know. It shows affected vehicles (no fix, low C/N0, jump) and, where the
  scenario allows, a marked "suspected interference" area computed from those symptoms. A
  scenario mode for training may draw the true circle, labelled as truth;
- vehicles in failover as a ring colour, not a separate icon set.

**Fleet level:** counts of vehicles by link state and by navigation trust, and the number that have
switched to a backup radio in the last five minutes. This is what an operator needs to see that
several vehicles are failing together (the signature of interference or a tower outage) rather than
one at a time.

**What not to show:** the truth position, a spoofed vehicle's true offset, or the scripted event
names. They are in the truth log for evaluation only. A study that wants to measure whether an
operator notices a spoof cannot also display it.

## 9. Open questions

1. The plane's main link. `links.toml` comments call the plane SATCOM-main; `platforms.toml` says
   `los` main with SATCOM backup. A fixed-wing delivery over a few tens of kilometres is within
   LOS from a mast, and also within cellular reach at low altitude. Pick one and change the other.
2. Does a delivery boat need `cellular` main at all, or should it be `los` near shore? The draft
   follows the platform file (cellular to satcom_leo) because scenario 3 needs the failover.
3. Should failover parameters be per platform, not per link class? An offshore boat may want a
   longer hold than a drone.
4. The GNSS spoofing detection gap: the navigation rule cannot see a slow, careful walk. Is that
   the point (a study of a weakness) or a gap to close with a second position source?
5. Replacement models for the two placeholders: how detailed? Options are a regional table with
   scripted outages, or a coarse sky-view map baked from building heights.
6. Clean up classes no delivery platform uses (`acoustic`, `tether`, Iridium, Ku GEO) once the
   scenarios settle, or keep them as available variants.
