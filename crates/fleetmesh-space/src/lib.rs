//! Satellite geometry for FleetMesh: SGP4 propagation of public element sets, access windows
//! for a ground terminal's elevation mask, GNSS dilution of precision, and the scenario clock
//! and epoch mapping every process shares.
//!
//! The link emulator uses it for SATCOM availability and the stand-ins for GNSS geometry and
//! the scenario clock. Constellation data is not shipped yet: SATCOM falls back to static link
//! parameters and GNSS receivers see a fixed sky (both marked FLEETMESH-REVIEW). See
//! docs/design/link-and-gnss.md.

pub mod access;
pub mod area;
pub mod geo;
pub mod gnss;
pub mod orbit;
pub mod scenario_clock;
pub mod timemap;

pub use area::{Area, NamedArea};
pub use geo::Geodetic;
pub use orbit::{Satellite, Sgp4Mode};
pub use timemap::{EpochMode, TimeMap};
