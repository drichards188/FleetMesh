//! FleetMesh's company-side traffic picture (docs/design/traffic.md, sections 4 and 5).
//!
//! It turns what a private delivery company can receive (ADS-B as SBS-1 lines, AIS as NMEA
//! AIVDM, Remote ID and its own sensors' detections as JSON lines) into tracks of the traffic it
//! does not control. It uses nothing beyond those formats, so it runs unchanged on a real
//! dump1090 SBS stream or a real AIS receiver's NMEA.
//!
//! Deterministic: there is no clock inside. Every time comes from the inputs or the caller, and
//! no output depends on hash-map order, so the same calls in the same order give the same
//! picture (the bridge logs the picture and replay rebuilds it).
//!
//! ```
//! use fleetmesh_tracker::{Config, Tracker};
//! let mut tr = Tracker::new(Config::default());
//! let line = "MSG,3,1,1,A1B2C3,1,2026/06/14,17:00:00.000,2026/06/14,17:00:00.050,,3000,,,37.70000,-122.35000,,,0,0,0,0";
//! tr.ingest_sbs(line, 1_781_456_400_050);
//! tr.ingest_sbs(&line.replace("17:00:00.000", "17:00:01.000").replace("37.70000", "37.70100"), 1_781_456_401_050);
//! tr.update(1_781_456_401_100);
//! assert_eq!(tr.tracks().len(), 1);
//! assert_eq!(tr.tracks()[0].ids.icao24.as_deref(), Some("a1b2c3"));
//! ```

// The filter and fit code indexes small matrices; index loops read closer to the algebra.
#![allow(clippy::needless_range_loop)]

pub mod ais;
pub mod bearing;
pub mod config;
pub mod geo;
pub mod kf;
pub mod meas;
pub mod nmea;
pub mod sbs;
pub mod track;
pub mod tracker;
pub mod wire;

pub use config::{Config, OwnFleet, OwnVehicle, SensorSite, SiteKind, Timing};
pub use track::{
    AltRef, BearingLine, Class, Disagreement, Domain, Flag, Ids, Prediction, Source, SourceKind,
    Static, Track, TrackState,
};
pub use tracker::{Outcome, Stats, Tracker};
pub use wire::SensorKind;
