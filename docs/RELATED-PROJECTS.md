# Related open-source projects

Projects that could supplement or replace parts of FleetMesh, recorded so we reach for them
before building the same thing. Surveyed 2026-11-24: licences and last-push dates came from the
GitHub API on that date; capabilities come from each project's README and docs. Nothing was
cloned or run, so treat every fit below as a lead to evaluate, not a decision. Items marked
(unverified) could not be confirmed.

## What nothing else covers

No open-source project found combines more than two or three of FleetMesh's capabilities, and
none covers its core as a package:

- orders routed by policy to execute at once or wait for a person, through an operator inbox;
- a deterministic engine checking every order, with a hash-chained audit log and replay;
- AI agents that can propose and explain orders but structurally cannot approve them (existing
  MCP servers for autopilots gate per tool, not through a policy engine with a human approver);
- operator attention and workload measured as the fleet scales.

Also uncovered: link emulation coupling terrain line of sight, cellular, SATCOM and relay mesh
to many SITL vehicles; GNSS jamming and spoofing as range-dependent effects on every receiver;
tide and current from real data driving boat SITL; mixed air, ground and surface SITL at
thousands of vehicles; and delivery operations on real autopilots. These stay ours to build.

## By FleetMesh area

### Airspace, geofences and keep-out prediction (keep-out spec, not yet written)

- **Flight Blender** (OpenUTM). https://github.com/openutm/flight-blender. Apache-2.0, pushed
  2026-11-12, small community. Remote ID service provider, ASTM F3548 USS-to-USS strategic
  coordination, ED-269 geofences, conformance monitoring, traffic feed aggregation; works with
  the InterUSS DSS. Could supply geofences, strategic deconfliction of planned routes and Remote
  ID, which would replace much of what the keep-out spec would otherwise define for the fleet's
  own routes. Integration: run as an HTTP service.
- **InterUSS DSS** plus `interuss/monitoring` and `interuss/uas_standards`.
  https://github.com/interuss/dss. Apache-2.0, pushed 2026-11-24, production grade. Discovery
  and synchronization for ASTM F3411 (Remote ID) and F3548. `uas_standards` gives typed data
  models; `monitoring` is a qualification harness. Cost: it needs CockroachDB or YugabyteDB.
- **BlueSky** (TU Delft). https://github.com/TUDelft-CNS-ATM/bluesky. MIT, pushed 2026-10-12.
  Fast-time air traffic simulator with conflict detection and resolution. Kinematic aircraft,
  not autopilots. Useful to cross-check deconfliction, or to add background traffic at counts
  SITL cannot reach.

### Dispatch, routing and hubs (not yet in the plan)

- **VROOM**. https://github.com/VROOM-Project/vroom. BSD-2, pushed 2026-11-17. Vehicle-routing
  solver with capacity, time windows and multiple depots, run as a lightweight service.
- **OR-Tools**. https://github.com/google/or-tools. Apache-2.0, pushed 2026-11-23. More
  flexible routing and scheduling.
- **Timefold Solver**. https://github.com/TimefoldAI/timefold-solver. Apache-2.0 community
  edition (enterprise features separate, unverified), pushed 2026-11-24. Java planning solver.

None is drone-aware: energy, wind and airspace costs would be ours to add to the cost matrix.
No maintained open delivery-fleet simulator was found; `sisl/MultiAgentAllocationTransit.jl`
(MIT, 2023) is research code on multi-drone delivery using transit networks.

### Attention-cost measure (spec, not yet written)

- **OpenMATB**. https://github.com/juliencegarra/OpenMATB. Licence listed as "Other"
  (CeCILL family, unverified), pushed 2026-11-11. Open reimplementation of NASA's
  Multi-Attribute Task Battery with logging: the best open instrument for workload protocols
  (secondary tasks, performance logs). Not fleet-aware. Reuse the task design and log format;
  check the licence text before copying code.
- **RESCHU** (MIT Humans and Automation Lab). No maintained repository found, only student
  copies from 2014 to 2018 (unverified). Valuable as a published experimental paradigm for
  operator-to-vehicle ratio and workload studies, to ground the measure in existing work.
- **Dronology and DroneResponse** (Notre Dame). Stale: the only hits were an old MIT-licensed
  copy (https://github.com/JaneClelandHuang/Dronology, 2017) and lab repos last pushed 2017 to
  2022 (https://github.com/SAREC-Lab). Research artefacts on human-drone interaction and safety
  cases; ideas, not code.

### Link emulator (`crates/fleetmesh-linkemu`)

Keep ours for fleet scale; the projects below are for cross-checks and small-scale realism.

- **EMANE**. https://github.com/adjacentlink/emane. BSD-style "Other" (unverified), pushed
  2026-11-20. High-fidelity radio and mobility emulation with real traffic; practical at modest
  node counts. Could validate the relay mesh model.
- **CORE**. https://github.com/coreemu/core. BSD-2, pushed 2026-06-02. Network emulator often
  paired with EMANE.
- **ns-3**. GPL-2.0, hosted on GitLab (licence variant and activity unverified). Only as a
  separate process: see licence notes.
- **Mininet-WiFi** (fork https://github.com/intrig-unicamp/mininet-wifi, 2026-10) and
  **UavNetSim** (https://github.com/Zihao-Felix-Zhou/UavNetSim, MIT, 2026-09): lighter
  references for wireless and UAV network behaviour.
- **cellular-satcom-emulator**. https://github.com/KaushikChavali/cellular-satcom-emulator.
  GPL-3.0, 2023, tiny. A reference for cellular and satellite link parameters.

### GNSS effects (`crates/fleetmesh-standins/src/gnss.rs`)

- **Damn Vulnerable Drone**. https://github.com/nicholasaleks/Damn-Vulnerable-Drone. MIT,
  pushed 2026-09-29. Worked GPS jamming and spoofing scenarios on ArduPilot SITL; a reference for
  realistic cases to test the fleet against.
- **ArduPilot SITL GPS failure and glitch parameters** (unverified at the current version).
  Could complement our receiver model for single-vehicle checks.

### Vehicles, autopilots and ground control (`crates/fleetmesh-standins`, ArduPilot SITL)

- **PX4** SITL. https://github.com/PX4/PX4-Autopilot. BSD-3, pushed 2026-11-25. A second
  autopilot stack for air and rover (boat support unverified).
- **MAVSDK**. BSD-3, pushed 2026-11-24. Clean multi-vehicle MAVLink client library, if the
  bridge ever needs one instead of its own MAVLink code.
- **Skybrush server**. https://github.com/skybrush-io/skybrush-server. GPL-3.0, pushed
  2026-11-24. Runs drone shows and swarms of hundreds to thousands, with RTK and weather
  extensions (SITL integration unverified; Skybrush Live licence unchecked). Worth studying for
  how it scales one operator over many drones.
- **QGroundControl**, **MAVProxy**, **Mission Planner**: multi-vehicle ground control for a
  handful of vehicles; useful as tools during development, not as the console.
- **MOOS-IvP**. https://github.com/moos-ivp/moos-ivp. Licence "Other" (GPL/LGPL mix,
  unverified), pushed 2026-11-21. Mature multi-vehicle marine autonomy with a strong
  human-in-the-loop experiment culture; a reference for the boat domain.
- **VRX**. https://github.com/osrf/vrx. Apache-2.0, 2026-08. Gazebo marine simulation.
- **CARLA** (MIT) and **Autoware/AWSIM**: only relevant if ground vehicles ever move off
  ArduPilot Rover to a road-driving stack.

### Agents and MCP tools (bridge MCP layer, not yet ported)

- **ROSA** (NASA JPL). https://github.com/nasa-jpl/rosa. Apache-2.0, pushed 2026-11-18.
  Natural-language agent over ROS via LangChain tools; no approval gate.
- **MAVLink and ArduPilot MCP servers**, all small single-developer projects and
  single-vehicle: https://github.com/ion-g-ion/MAVLinkMCP (MIT),
  https://github.com/deepakpopli/mavlink-mcp (MIT), https://github.com/rmeadomavic/ardupilot-mcp
  (MIT), https://github.com/Chevit/ardupilot-mcp (Apache-2.0),
  https://github.com/hfujikawa77/ardupilot-mcp-server (GPL-3.0),
  https://github.com/furkanisikay/ardupilot-mcp (AGPL-3.0). Use as references for tool shapes.
  The approval boundary must stay in the bridge.

### Fleet task layer and traffic negotiation (bridge tasks, not yet ported)

- **Open-RMF**. https://github.com/open-rmf/rmf, with `free_fleet` and `rmf_traffic`.
  Apache-2.0, pushed 2026-11-12. Multi-fleet robot traffic deconfliction, task dispatch and a
  web dashboard for indoor and site robots on ROS 2. A design reference for fleet adapters and
  traffic negotiation; the ROS 2 dependency makes it heavy to embed.
- **OpenUxAS / OpenAMASE** (US Air Force Research Laboratory). https://github.com/afrl-rq/OpenUxAS,
  https://github.com/afrl-rq/OpenAMASE. Lab open-source agreement ("Other", wording
  unverified), pushed 2026-09 and 2026-10. Multi-UAV task assignment and automated planning
  with a mission simulation environment. A reference for task allocation; no human-approval
  policy layer.

### Decision engine, audit and replay (`fleetmesh-decide`, not yet ported)

Nothing comparable found in the drone or UTM space. General tamper-evident log tools
(transparency logs, immutable ledgers) exist but were not evaluated.

## Licence notes

FleetMesh is AGPL-3.0-or-later. Not legal advice; check each licence text before depending on
code.

- Permissive licences (MIT, BSD, Apache-2.0) can be combined into AGPL-3.0 work.
- GPL-3.0 code can be combined with AGPL-3.0 code (GPLv3 section 13), so ArduPilot, MAVProxy and
  the Skybrush server are not a licence obstacle. Keeping autopilots in separate processes over
  MAVLink is still the right architecture.
- GPL-2.0-only code (possibly ns-3, unverified) cannot be combined with AGPL-3.0 code: run it as
  a separate program only.
- Licences shown as "Other" (OpenMATB, MOOS-IvP, EMANE, OpenUxAS, OpenAMASE, Mininet-WiFi,
  AWSIM) need their text read before any code is copied.
- Running Flight Blender, the DSS or VROOM as separate services keeps their licences separate
  either way.

## Not verified

RESCHU's repository and licence; the canonical Dronology and DroneResponse repositories; licence
text for OpenMATB, OpenUxAS, OpenAMASE, MOOS-IvP, EMANE and Mininet-WiFi; ns-3's licence
variant and activity; the Skybrush Live licence; MAVProxy's link-emulation modules; PX4 boat
SITL; ArduPilot GPS failure parameters at the current version; and whether any project runs
SITL at thousands of vehicles.
