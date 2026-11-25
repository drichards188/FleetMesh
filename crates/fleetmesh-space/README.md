# fleetmesh-space

Satellite geometry for FleetMesh, as a library.

| Module | What it provides |
|---|---|
| `orbit` | SGP4 propagation of public element sets (CelesTrak OMM JSON or TLE lines) |
| `access` | Access windows: when a satellite is above a ground terminal's elevation mask |
| `geo` | Frames and geometry: TEME to Earth-fixed, WGS84 geodetic, look angles, sun position |
| `gnss` | Dilution of precision from satellite geometry (no signal or clock model here) |
| `area` | A polygon, or a point with a radius |
| `timemap` | Scenario time versus propagation time, so dated scenarios use current element sets |
| `scenario_clock` | `FLEETMESH_AT` (RFC 3339) plus elapsed time times `FLEETMESH_RATE`, read the same way by every process |

Users: `crates/fleetmesh-linkemu` (SATCOM availability, `TimeMap`) and `crates/fleetmesh-standins`
(GNSS geometry, the scenario clock).

Constellation data is not shipped yet. Until it is, SATCOM links fall back to static
parameters and GNSS receivers see a fixed sky; both places are marked `FLEETMESH-REVIEW`. GNSS
jamming, spoofing and degradation are effects on the fleet's receivers, modelled in the
stand-ins (`crates/fleetmesh-standins/src/gnss.rs`), not here. Design notes:
`docs/design/link-and-gnss.md`.

Tests: `cargo test -p fleetmesh-space` (SGP4 verification vectors in `tests/data/`).
