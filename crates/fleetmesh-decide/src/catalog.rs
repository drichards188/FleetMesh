//! Platform facts from the catalog (`sim/platforms.toml`) and the capability table.
//!
//! The bridge copies a vehicle's [`PlatformFacts`] into each [`crate::WorldSnapshot`], so the
//! facts a rule read (armed, turn radius, endurance, a sensor's reach) are logged with the
//! decision and replay needs no catalog file. A sensor's reach is the one of the sensing model
//! the run is on ([`Reaches`], [`Catalog::reaching`]).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::intent::TaskKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Air,
    Ground,
    Surface,
}

impl Domain {
    pub fn as_str(self) -> &'static str {
        match self {
            Domain::Air => "air",
            Domain::Ground => "ground",
            Domain::Surface => "surface",
        }
    }
}

/// Which autopilot a vehicle runs (or imitates), which sets what it can be tasked with.
/// Mirrors `KINDS` in the Python bridge's `tasks.py`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutopilotKind {
    Plane,
    Vtol,
    Copter,
    Blimp,
    Boat,
    Ugv,
}

impl AutopilotKind {
    pub fn tasks(self) -> &'static [TaskKind] {
        use TaskKind::*;
        match self {
            AutopilotKind::Plane
            | AutopilotKind::Vtol
            | AutopilotKind::Copter
            | AutopilotKind::Blimp
            | AutopilotKind::Boat
            | AutopilotKind::Ugv => &[
                Orbit, Patrol, Transit, Hold, Rtl, Pickup, Deliver, Reroute, ReturnToHub, LandNow, Abort, SwapBattery,
                ConfirmDropZone,
            ],
        }
    }

    pub fn supports(self, kind: TaskKind) -> bool {
        self.tasks().contains(&kind)
    }

    /// Fixed-wing kinds fly orbits as a loiter circle with a physical minimum radius.
    pub fn is_fixed_wing(self) -> bool {
        matches!(self, AutopilotKind::Plane | AutopilotKind::Vtol)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AutopilotKind::Plane => "plane",
            AutopilotKind::Vtol => "vtol",
            AutopilotKind::Copter => "copter",
            AutopilotKind::Blimp => "blimp",
            AutopilotKind::Boat => "boat",
            AutopilotKind::Ugv => "ugv",
        }
    }

    /// From a catalog entry: SITL `vehicle` or stand-in `behavior` (as `SITL_KIND` /
    /// `STANDIN_KIND` in the Python bridge).
    pub fn from_catalog(sim: &str, vehicle_or_behavior: &str) -> Option<Self> {
        Some(match (sim, vehicle_or_behavior) {
            (_, "plane") => AutopilotKind::Plane,
            ("sitl", "vtol") => AutopilotKind::Vtol,
            ("sitl", "copter") => AutopilotKind::Copter,
            ("sitl", "blimp") => AutopilotKind::Blimp,
            (_, "boat") | ("sitl", "sailboat") => AutopilotKind::Boat,
            ("sitl", "ugv") => AutopilotKind::Ugv,
            _ => return None,
        })
    }
}

/// What a platform's `land_now` does (see [`PlatformFacts::land_now`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandNowBehavior {
    Land,
    LoiterLand,
    Stop,
    Moor,
}

impl LandNowBehavior {
    /// The behaviour the autopilot kind implies when the catalog does not say.
    pub fn default_for(kind: AutopilotKind) -> LandNowBehavior {
        match kind {
            AutopilotKind::Copter | AutopilotKind::Blimp => LandNowBehavior::Land,
            AutopilotKind::Plane | AutopilotKind::Vtol => LandNowBehavior::LoiterLand,
            AutopilotKind::Ugv => LandNowBehavior::Stop,
            AutopilotKind::Boat => LandNowBehavior::Moor,
        }
    }
}

/// What the decision layer needs to know about a platform type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlatformFacts {
    pub id: String,
    pub domain: Domain,
    pub cruise_mps: Option<f64>,
    /// cruise speed / turn rate, rounded to metres (as the Python bridge computes it).
    pub min_turn_radius_m: Option<f64>,
    pub endurance_h: Option<f64>,
    /// The autopilot the catalog says this platform runs. The live kind comes from the
    /// vehicle's heartbeat ([`crate::VehicleFacts::autopilot`]).
    pub autopilot: Option<AutopilotKind>,
    /// The platform's sensors (`sensors = [...]` in the catalog): which of our vehicles can
    /// existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensors: Vec<SensorFact>,
    /// The catalog's climb rate, m/s (`climb_mps`): rules 6 count one of our vehicles as able
    /// to change altitude at least this fast (or `vertical_rate_mps`, the larger) when they
    /// read a keep-out in 3D. Absent in logs written before it existed and for platforms
    /// without the figure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub climb_mps: Option<f64>,
    /// The catalog's top speed, m/s (`max_mps`): rules 6 count a fixed wing of ours as able
    /// to dive at it down `FIXED_WING_DIVE_DEG`. Absent where the catalog has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mps: Option<f64>,
    /// The catalog's payload limit, kg (`payload_kg`): the payload rule's maximum for this
    /// platform. Absent where the catalog has none (the rule then holds `PAYLOAD_UNKNOWN`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_kg: Option<f64>,
    /// The catalog's cruise altitude, m (`typical_alt_m`): the geofence rule's altitude limit is
    /// this plus its slack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typical_alt_m: Option<f64>,
    /// A boat's draft, m (`draft_m`): the tide rule's keel clearance reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_m: Option<f64>,
    /// What the platform's own `land_now` is (delivery-orders.md section 3): `land` (a copter),
    /// `loiter_land` (a plane: loiter, then land at the nearest approved landing site), `stop` (a
    /// car), `moor` (a boat: moor or hold station at the nearest dock). Absent: not stated, so
    /// the platform's autopilot kind decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land_now: Option<LandNowBehavior>,
    /// Rules 34 ([`crate::rules::GROUP_ORDER_SINCE`]): the type's separation and band for a group
    /// order's layout ([`crate::group::Formation`]: the catalog's `sep_m`, `alt_band_m` and
    /// `depth_band_m` over the default by autopilot kind, every figure an estimate). Put on the
    /// facts only for a run deciding under rules 34 ([`Catalog::formed`]); the engine drops it
    /// before logging under older rules. Absent elsewhere and in older logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation: Option<crate::group::Formation>,
}

/// Contact kinds the catalog's `effects` do not name, and the kind whose figures stand for each
/// sailboat, a cargo ship, a car, a truck, a small drone, a light aircraft, a UUV and a person):
/// each an estimate [E] by size and speed. The truth side keeps the same table
/// (crates/murmur-contacts `damage::KIND_ALIASES`; crates/fleetmesh-headless tests that they agree).
pub const KIND_ALIASES: &[(&str, &str)] = &[
    // a tanker is a cargo ship's size and speed
    ("tanker", "cargo_ship"),
    ("ferry", "cargo_ship"),
    // small, fast and armed
    ("patrol_boat", "small_fast_boat"),
    ("helicopter", "light_aircraft"),
    ("unknown_uas", "small_drone"),
    ("submarine", "uuv"),
    ("periscope", "uuv"),
    ("diver", "person"),
];

/// The kind whose figures stand for a contact of `kind` ([`KIND_ALIASES`]; itself otherwise).
pub fn target_kind(kind: &str) -> &str {
    KIND_ALIASES.iter().find(|(k, _)| *k == kind).map_or(kind, |(_, to)| to)
}

/// One sensor from the catalog: its type (`radar`, `eo_ir`, `ais`, `sonar`, `esm`) and its
/// nominal range (where a nominal contact is detected half the time).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorFact {
    #[serde(rename = "type")]
    pub kind: String,
    /// The legacy sensing model's reach (and the engine's in a run on that model).
    pub range_m: f64,
    /// In a run on the physical sensing model, the engine's reach by the domain of what it
    /// looks for: the catalog's `physical_reach_m`, derived from the physical model
    /// (docs/SENSING.md, "The decision engine's reaches"; [`Catalog::reaching`]). Absent in a
    /// run on the legacy model, for a sensor the catalog derives none for, and in every log
    /// before it: the engine reads `range_m` then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach_m: Option<SensorReach>,
}

impl SensorFact {
    /// How far this sensor reaches a contact in `domain`, m: its physical reach for the domain
    /// when it carries them (none: it does not reach one there at all), else `range_m`.
    pub fn reach_for(&self, domain: Domain) -> Option<f64> {
        match &self.reach_m {
            Some(r) => r.get(domain),
            None => Some(self.range_m),
        }
    }

}

/// A sensor's reach by the domain of the contact, m (none: it does not reach one there).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorReach {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsurface: Option<f64>,
}

impl SensorReach {
    pub fn get(&self, domain: Domain) -> Option<f64> {
        match domain {
            Domain::Air => self.air,
            Domain::Ground => self.ground,
            Domain::Surface => self.surface,
        }
    }
}

#[derive(Deserialize)]
struct RawPlatform {
    domain: Domain,
    sim: String,
    vehicle: Option<String>,
    behavior: Option<String>,
    cruise_mps: Option<f64>,
    turn_dps: Option<f64>,
    endurance_h: Option<f64>,
    climb_mps: Option<f64>,
    max_mps: Option<f64>,
    payload_kg: Option<f64>,
    draft_m: Option<f64>,
    land_now: Option<LandNowBehavior>,
    #[serde(default)]
    sensors: Vec<RawSensor>,
    /// Each sensor's reach under the physical model, by its type and the contact's domain
    /// (generated: docs/SENSING.md, "The decision engine's reaches").
    #[serde(default)]
    physical_reach_m: BTreeMap<String, SensorReach>,
    /// Rules 34: a group order's figures for the type (docs/SWARMS.md section 3.3).
    sep_m: Option<crate::group::RawSep>,
    alt_band_m: Option<crate::group::RawAltBand>,
    depth_band_m: Option<[f64; 2]>,
    loiter_radius_m: Option<f64>,
    typical_alt_m: Option<f64>,
}

#[derive(Deserialize)]
struct RawSensor {
    #[serde(rename = "type")]
    kind: String,
    range_m: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("platform catalog: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("platform catalog {0}: {1}")]
    Io(String, std::io::Error),
    #[error("platform catalog: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    /// Each platform's facts with `range_m` alone ([`Reaches::Legacy`]).
    pub platforms: BTreeMap<String, PlatformFacts>,
    /// Each platform's `physical_reach_m`, by sensor type: what [`Catalog::reaching`] puts
    /// on its sensors for a run on the physical model.
    physical_reach: BTreeMap<String, BTreeMap<String, SensorReach>>,
    /// Rules 34: each platform's group-order figures, the entry's own over the default by
    /// autopilot kind: what [`Catalog::formed`] puts on its facts for a run under rules 34.
    formation: BTreeMap<String, crate::group::Formation>,
}

impl Catalog {
    pub fn from_toml(text: &str) -> Result<Self, CatalogError> {
        let mut raw: BTreeMap<String, RawPlatform> = toml::from_str(text)?;
        let mut physical_reach = BTreeMap::new();
        let mut raws: BTreeMap<String, crate::group::RawFormation> = BTreeMap::new();
        for (id, r) in &mut raw {
            for (kind, reach) in &r.physical_reach_m {
                if !r.sensors.iter().any(|s| s.kind == *kind) {
                    return Err(CatalogError::Invalid(format!(
                        "[{id}] physical_reach_m.{kind}: the platform carries no {kind}"
                    )));
                }
                for d in [Domain::Air, Domain::Ground, Domain::Surface] {
                    if let Some(m) = reach.get(d)
                        && !(m.is_finite() && m > 0.0)
                    {
                        return Err(CatalogError::Invalid(format!(
                            "[{id}] physical_reach_m.{kind}.{}: {m} m (a reach is above 0; leave out a domain it does not reach)",
                            format!("{d:?}").to_lowercase()
                        )));
                    }
                }
            }
            // rules 34: a group order's figures, each one the entry gives within its bounds
            let raw = crate::group::RawFormation {
                sep_m: r.sep_m.clone(),
                alt_band_m: r.alt_band_m.clone(),
                depth_band_m: r.depth_band_m,
                loiter_radius_m: r.loiter_radius_m,
                cruise_alt_m: r.typical_alt_m,
            };
            if let Some(why) = raw.invalid(r.domain) {
                return Err(CatalogError::Invalid(format!("[{id}] {why}")));
            }
            raws.insert(id.clone(), raw);
            let reach = std::mem::take(&mut r.physical_reach_m);
            if !reach.is_empty() {
                physical_reach.insert(id.clone(), reach);
            }
        }
        let platforms = raw
            .into_iter()
            .map(|(id, r)| {
                let min_turn_radius_m = match (r.cruise_mps, r.turn_dps) {
                    (Some(v), Some(dps)) if v > 0.0 && dps > 0.0 => {
                        Some((v / dps.to_radians()).round())
                    }
                    _ => None,
                };
                let which = r.vehicle.as_deref().or(r.behavior.as_deref()).unwrap_or("");
                let facts = PlatformFacts {
                    id: id.clone(),
                    domain: r.domain,
                    cruise_mps: r.cruise_mps,
                    min_turn_radius_m,
                    endurance_h: r.endurance_h,
                    autopilot: AutopilotKind::from_catalog(&r.sim, which),
                    sensors: r
                        .sensors
                        .into_iter()
                        .map(|s| SensorFact {
                            kind: s.kind,
                            range_m: s.range_m,
                            reach_m: None,
                        })
                        .collect(),
                    climb_mps: r.climb_mps.filter(|c| c.is_finite() && *c > 0.0),
                    max_mps: r.max_mps.filter(|c| c.is_finite() && *c > 0.0),
                    payload_kg: r.payload_kg.filter(|c| c.is_finite() && *c > 0.0),
                    typical_alt_m: r.typical_alt_m.filter(|c| c.is_finite() && *c > 0.0),
                    draft_m: r.draft_m.filter(|c| c.is_finite() && *c >= 0.0),
                    land_now: r.land_now,
                    formation: None,
                };
                (id, facts)
            })
            .collect::<BTreeMap<String, PlatformFacts>>();
        let formation = platforms
            .iter()
            .map(|(id, p)| (id.clone(), raws.get(id).cloned().unwrap_or_default().resolve(p)))
            .collect();
        Ok(Catalog {
            platforms,
            physical_reach,
            formation,
        })
    }

    pub fn load(path: &Path) -> Result<Self, CatalogError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CatalogError::Io(path.display().to_string(), e))?;
        Self::from_toml(&text)
    }

    pub fn get(&self, id: &str) -> Option<&PlatformFacts> {
        self.platforms.get(id)
    }

    /// Rules 34 ([`crate::rules::GROUP_ORDER_SINCE`]): this catalog with each platform's
    /// group-order figures on its facts ([`PlatformFacts::formation`]), for a run deciding under
    /// rules 34; a run under older rules uses the catalog as loaded, so its facts, and every
    /// decision logged with them, are as before.
    pub fn formed(&self) -> Catalog {
        let mut c = self.clone();
        for (id, p) in &mut c.platforms {
            p.formation = self.formation.get(id).copied();
        }
        c
    }

    /// A platform's group-order figures, as [`Catalog::formed`] puts them on its facts.
    pub fn formation(&self, id: &str) -> Option<crate::group::Formation> {
        self.formation.get(id).copied()
    }

}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// The shipped catalog (`sim/platforms.toml`) is the four delivery platforms, each with the
    /// figures the checks read.
    #[test]
    fn the_shipped_catalog_is_the_four_delivery_platforms() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let c = Catalog::load(&root.join("sim/platforms.toml")).unwrap();
        for id in ["delivery_drone", "delivery_car", "delivery_boat", "delivery_plane"] {
            let p = c.get(id).unwrap_or_else(|| panic!("{id} in the catalog"));
            assert!(p.cruise_mps.is_some_and(|v| v > 0.0), "{id}: a cruise speed");
            assert!(p.endurance_h.is_some_and(|v| v > 0.0), "{id}: an endurance");
        }
        assert_eq!(c.get("delivery_drone").unwrap().domain, Domain::Air);
        assert_eq!(c.get("delivery_car").unwrap().domain, Domain::Ground);
        assert_eq!(c.get("delivery_boat").unwrap().domain, Domain::Surface);
        assert!(c.get("puma").is_none(), "no platform of the earlier catalog");
    }
}
