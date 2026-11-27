# Delivery plan

Status as of 2026-11-25. FleetMesh is a working draft assembled from a pruned copy of an earlier
project (commit f7b1874). Decisions made on the way are in [DECISIONS.md](DECISIONS.md). Work items are tracked in
bd (`bd ready`); this file says what the phases are and how they depend on each other.

The research question the plan serves: does human judgment over consequential decisions stay
meaningful as the fleet scales? Every phase either makes the fleet more like a delivery fleet or
makes the answer measurable.

## Phase summary

| # | phase | status | design |
| --- | --- | --- | --- |
| 1 | Vehicle-side port | done | [link-and-gnss.md](design/link-and-gnss.md), [WEATHER.md](WEATHER.md) |
| 2 | Engine, bridge and console port | done | [delivery-orders.md](design/delivery-orders.md), [rules-versioning.md](design/rules-versioning.md) |
| 3 | Delivery orders and rules | done (rules v2: standing approvals, management by exception) | [delivery-orders.md](design/delivery-orders.md), [delivery-rules.md](design/delivery-rules.md) |
| 4 | Dispatch, parcels, hubs | done, batching not built; home bases (rules 4); stands and hub sequencing (rules 5); payload room before assignment | [dispatch.md](design/dispatch.md), [fleet-organisation.md](design/fleet-organisation.md) |
| 5 | Keep-out prediction | next | [keep-out.md](design/keep-out.md) |
| 6 | Link and GNSS delivery models | next | [link-and-gnss.md](design/link-and-gnss.md) |
| 7 | Scenarios and console delivery UI | done (harbour-city, wind-event, coastal-boat) | [scenarios.md](design/scenarios.md) |
| 8 | Evaluation and attention cost | next | [attention-cost.md](design/attention-cost.md) |
| 9 | Fast forward | optional, after 8 | [FASTFORWARD.md](FASTFORWARD.md) |
| 10 | Rename and first commit | done (first commit 5c1003c) | [DECISIONS.md](DECISIONS.md) |
| 11 | Other traffic and the company's picture of it | phases 1 to 3 done (rules 3: separation from other traffic, automation first); console layer by the console | [traffic.md](design/traffic.md) |
| 12 | External interfaces (identity broadcasts, UTM, ATC and VTS, city and responders) | backlog | [external-interfaces.md](design/external-interfaces.md) |
| 13 | AI watch officer triages the Inbox | step 1 built (rules 7: the triage queue, deadlines, replay); steps 2 to 6 in bd (fleetmesh-ic4) | [watch-officer.md](design/watch-officer.md) |

## 1. Vehicle-side port (done)

What is in the tree and clean: `crates/fleetmesh-weather` (replayable archived surface weather),
`crates/fleetmesh-linkemu` (the link emulator: link classes, cellular with height falloff, main and
backup failover, terrain line of sight), `crates/fleetmesh-space` (the satellite remnant: SGP4, DOP,
access, scenario clock), `crates/fleetmesh-standins` (lightweight stand-in vehicles with GNSS
reception, navigation truth against estimate, failsafes and battery), `tidewater/` (real terrain,
tide, current and wind for the Golden Gate and the Bay region), `sim/platforms.toml` (four delivery
platforms), `sim/links.toml`, `sim/areas/`, `sim/fleets/`, `sim/link-events/`.

Known gaps carried forward, each marked `FLEETMESH-REVIEW` in the code: SATCOM runs on fixed static
parameters (no availability model); GNSS uses a fixed sky (14 satellites, HDOP 1.0). Phase 6. See
Open items for the full list.

Exit: done. Further changes belong to the phases that use it.

## 2. Engine, bridge and console port (done)

Port the decision engine (`fleetmesh-decide`: intents, queue, authority, audit, replay), the bridge
(`fleetmesh-bridge`: REST, MCP, task layer, WebSocket) and the console from the earlier project, cut
to what a delivery fleet needs. The cut is compiler-driven (DECISIONS.md): remove what has no
delivery meaning, let the compiler list what breaks, repeat. The state machine, the authority
types (agents cannot approve), the hash-chained log and replay are kept as they are.

Needs: nothing. Blocks phases 3, 4, 5, 8.

Exit: `make check` passes; the bridge starts with the delivery demo fleet; the console shows
vehicles and an inbox; an order proposed by an agent over MCP waits in the inbox; an operator
approves it; the log replays with no mismatch. Inherited task kinds are gone. The rules version is 1.

## 3. Delivery orders and rules (done)

Replace the inherited task kinds with the nine delivery kinds, and the inherited checks with the
delivery rules: geofences and temporary restrictions, energy with wind, payload, weather, separation,
drop zone, navigation integrity, lost link. Add `Outcome::Hold`. Write the policy file and the rules
config. Build the inbox card from the logged checks.

Design: [delivery-orders.md](design/delivery-orders.md), [delivery-rules.md](design/delivery-rules.md).

Needs: phase 2 far enough that the engine builds. Blocks 4, 5, 7, 8.

Exit: every kind proposes, checks, routes and dispatches against stand-ins; each rule has tests at
its boundaries; replay re-derives every decision; defaults match the design documents or the
documents are updated in the same change.

## 4. Dispatch (done, except batching)

Parcels, hubs, drop zones, the deterministic greedy assigner with energy and wind costs, the plan
log event, and the proposals the dispatcher makes as an agent. The fleet is organised by home base
(rules 4, [fleet-organisation.md](design/fleet-organisation.md)): every vehicle returns to its base,
the assigner costs the way back to it, and the console lists and maps the fleet by base and by
operational state. From rules 5 each vehicle parks on its own stand and the hub sequences its own
arrivals and departures (no person decides who lands first); rules 6 reads the ground and the
water where an order goes (DECISIONS.md, 2026-11-25). v2 (optional): an adapter to VROOM
or OR-Tools as a second proposer.

Design: [dispatch.md](design/dispatch.md).

Needs: phase 3 (orders and the shared energy model). Blocks 7 and 8.

Exit: a seeded demand run assigns all parcels with the greedy assigner, replay re-derives the plans,
and the assigner scales to the thousand-vehicle test.

## 5. Keep-out prediction (next)

The monitor that predicts route entries into areas and weather cells and proposes `hold` and
`reroute`. Option A integration (geofences from Flight Blender) later and only if a scenario needs
it.

Design: [keep-out.md](design/keep-out.md).

Needs: phase 3 (area set with temporary restrictions, the geofence sampler). Blocks scenario 2.

Exit: predictions and stages as designed, logged and replayed; scenario 1's street-fair restriction
produces the expected proposals.

## 6. Link and GNSS delivery models (next)

Replace the two placeholders: a delivery SATCOM availability model and a delivery GNSS availability
model (sky view). Give the bridge the link and navigation display in
[link-and-gnss.md](design/link-and-gnss.md), section 8. Resolve the plane's main link.

Design: [link-and-gnss.md](design/link-and-gnss.md).

Needs: phase 2 for the bridge display. Blocks scenario 3 in full (a scripted version can start
without the models).

Exit: no `FLEETMESH-REVIEW` placeholder left in `sat.rs` and `gnss.rs`; the operator sees link and
navigation state with ages; scenario 3's failover and interference run end to end.

## 7. Scenarios (done)

The three first scenarios as files under `sim/scenarios/`, a `scenario check` command, scripted
reference decisions. Fleet files take a scale factor so the same scenario runs from 10 to 3,000
vehicles.

Design: [scenarios.md](design/scenarios.md).

Needs: phases 3, 4, 5 and (for scenario 3) 6. Blocks 8.

Exit: each scenario runs headless for its duration with a scripted operator, its log replays, and
its reference table is produced.

## 8. Evaluation and attention cost (next)

The attention-cost measure: the new log events, the replay tool that computes the report, the
scripted operator, the scaling sweep, the harness for sessions with people. The `eval/` directory
holds the harness; the headless simulator produces golden episodes.

Design: [attention-cost.md](design/attention-cost.md).

Needs: phases 3, 4 and 7. Blocks any claim about people.

Exit: a scaling sweep report from the scripted operator at six fleet sizes, reproducible from the
logs; a pilot protocol for human sessions written down.

## 9. Fast forward (optional)

A world clock with a rate that changes during a run, dropping to 1x whenever something needs the
operator. Useful for long scenarios and for sweeps. Never in a credentials run, so never in a study
with people.

Design: [FASTFORWARD.md](FASTFORWARD.md).

Needs: phases 2 and 8. Not on the critical path.

## 10. Rename and first commit

The rename is done (2026-11-25, [DECISIONS.md](DECISIONS.md)): crates, `FLEETMESH_*` environment
variables, headers, paths and text. What is left is the first commit. Nothing is committed before
the cut is done, so the history starts clean. The commit message and DECISIONS.md say what the tree
was cut from.

Needs: the open items below settled or filed; `make check` green on a fresh checkout.

Exit: `git log` shows one clean first commit, linear history after it, `make check` green on a
fresh checkout, `docs/` carries no reference to removed features.

## 13. AI watch officer (in progress)

Deterministic code raises the alerts and holds; the AI watch officer triages them; only what it
cannot confidently handle reaches the person, with its recommendation; the rest is in the audit
trail. Rules 7 (2026-11-26, DECISIONS.md) built the bridge's side: the triage queue, the
deterministic deadlines and the officer's absence, the `triage_item` verdict tool, the person's
filtered Inbox, `GET /api/triage`, and replay of every step. Next: the dispatcher's side of turning
an order down, the officer's wake table and prompt, the menu's AI choice, the console's cards and
audit view, and phase 8's two arms.

Design: [watch-officer.md](design/watch-officer.md). Needs: phases 2 to 4. Feeds phase 8.

## Open items

- SITL gripper and params are untested: the stand-ins are exercised, ArduPilot's own builds are not.
- Batching (several parcels on one vehicle, see [dispatch.md](design/dispatch.md)) is not built.
- The coverage predictor's backup-link counters are not finished.
- Five `FLEETMESH-REVIEW` placeholders remain in the code: SATCOM runs on static parameters
  (`crates/fleetmesh-linkemu/src/sat.rs`) and GNSS uses a fixed sky (`crates/fleetmesh-standins/src/gnss.rs`);
  three neutral defaults for what a start carries (`crates/fleetmesh-bridge/src/decision/mod.rs`,
  `crates/fleetmesh-decide/src/nodes.rs`, `crates/fleetmesh-decide/src/replay.rs`).
- `tidewater/data/cache` is a link to the earlier project's cache (`~/murmur/tidewater/data/cache`); it
  needs its own copy or a fetch before the repository stands alone.
- The catalog decisions in `CATALOG.tsv` (many applied as `auto-judged`) need the maintainer's review.
- The research citations in [attention-cost.md](design/attention-cost.md) need checking.
- External interfaces a real delivery operator's command and control speaks, and FleetMesh does
  not yet: the fleet's own identity broadcasts (Remote ID, ADS-B Out and transponder codes, AIS),
  UTM strategic deconfliction (ASTM F3548 with the InterUSS DSS), NOTAM, TFR and METAR inputs,
  simulated ATC and VTS counterparts, drop-zone video and remote assistance, first-responder
  geofencing directives and curb data, and link authorization; and control real fleet software
  exercises that FleetMesh does not (fleet and zone pauses, vehicle-initiated guidance requests,
  flight termination). The researched, ordered backlog is
  [external-interfaces.md](design/external-interfaces.md).

## Ordering and risks

- The critical path is 2, 3, 4, 7, 8. Phases 5 and 6 can run beside 4.
- Rules start at version 1 and stay at 1 until the first commit. After it, any behaviour change is
  gated ([rules-versioning.md](design/rules-versioning.md)). That favours getting the rules
  settled before the first commit.
- The largest risk is the cut: cutting too little leaves meaningless modules (they invite
  extension); cutting too much removes machinery that delivery needs (the queue for unreachable
  vehicles, group orders, the lost-link ledger idea). Prefer cutting, and restore from the earlier
  project's commit when something turns out to be needed.
- A second risk is a measure that cannot be reproduced. Keep every metric a pure function of the log.
- Nothing here is connected to anything real. No phase changes that.

## Not in this plan

Real vehicles, real airspace data, regulatory approval, payments or customer interfaces, and any
vehicle other than the four delivery platforms. Multi-operator supervision (more than one person on
one fleet) is a later question; the engine's roster supports it, and no scenario uses it.
