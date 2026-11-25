//! Rules 32 ([`crate::rules::REACH_NOW_SINCE`], bd murmur-ahss): a sensor's reach under the
//! conditions where its vehicle is now.
//!
//! In a run on the physical sensing model each sensor's facts carry the catalog's
//! `physical_reach_m` ([`crate::Catalog::reaching`]): its reach on a clear day (20 km of
//! visibility, the sun 45 degrees up, sea state 2) at the platform's operating height. The
//! running model's reach can be far shorter: in fog of 50 m visibility a Puma's camera reaches a
//! boat about 140 m out, and a camera with only a visible channel reaches nothing at night. From
//! rules 32 the engine reads, for every sensor whose facts carry physical reaches, its reach now
//! instead: the vehicle fact [`crate::world::VehicleFacts::reach_now`] ([`ReachNow`]), which the
//! bridge computes outside the engine's lock from the physical model's own prediction where the
//! vehicle is, at its height, in the conditions the running model senses in there (its
//! `tracks::reach`, the computation the reach display, `GET /api/sensing/reach`, shows). The fact
//! so replay reads the reaches the engine read and needs no model.
//!
//! that is less than a clear day's planning reach ([`identify_now`], [`crate::identify::plan`]:
//! whose facts carry no physical reach (the legacy model's facts, a sensor the catalog derives
//! none for) is read as before, and so is every sensor under older rules.

use serde::{Deserialize, Serialize};

use crate::catalog::{Domain, PlatformFacts, SensorReach};
use crate::rules::RulesConfig;
use crate::world::{VehicleFacts, WorldSnapshot};

/// Below this visibility the words call it fog, m (the reach display's `fog`).
pub const FOG_M: f64 = 1_000.0;
/// Below this visibility (and from [`FOG_M`]) the words say the visibility, m.
pub const POOR_VISIBILITY_M: f64 = 10_000.0;
/// The sun this far below the horizon is night for a camera, degrees (the reach display's `light`:
/// a visible channel's light ends through civil twilight).
pub const NIGHT_DEG: f64 = -6.0;
/// The sun lower than this, and up, is the thermal crossover's dawn or dusk, degrees.
pub const LOW_SUN_DEG: f64 = 10.0;

/// Rules 32: one of ours' sensors' reach under the conditions where it is, as the physical sensing
/// model predicts it (the bridge's `tracks::reach`), and the conditions it was computed in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReachNow {
    /// The vehicle's height the reaches are for, m above mean sea level (as computed: the bridge
    /// rounds its inputs so that vehicles alike share one computation).
    pub alt_m: f64,
    /// The meteorological visibility there, m.
    pub visibility_m: f64,
    /// The sun's elevation there, degrees (negative: below the horizon).
    pub sun_elevation_deg: f64,
    /// Each of its sensors that confirms a contact's presence (radar, EO/IR, sonar), in the
    /// platform's order of its sensors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensors: Vec<SensorNow>,
}

/// Rules 32: one sensor's reach now ([`ReachNow`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorNow {
    /// Its type (`radar`, `eo_ir`, `sonar`), as the platform's sensor facts name it.
    #[serde(rename = "type")]
    pub kind: String,
    /// How far out it detects the reference contact of each domain it sees half the time all the
    /// way, m along the ground (the catalog's `physical_reach_m` is the same figure on a clear day
    /// at the operating height): 0 where it detects one nowhere now; a domain it does not see is
    /// absent.
    pub detect_m: SensorReach,
    /// An EO/IR's identification reach now, m of slant range from the vehicle: where its chance to
    /// identify the reference contact of each domain (and, in `size_class`, each size class's
    /// smallest member) falls to one half; 0 where it identifies one nowhere now. Absent for any
    /// other sensor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identify_m: Option<SensorReach>,
    /// An EO/IR with a thermal channel (MWIR or LWIR), which needs no light.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub thermal: bool,
}

impl ReachNow {
    /// The entry for the platform's sensor `i` (the `k`-th of its type: the `k`-th entry of that
    /// type), if the fact has one.
    pub fn sensor(&self, p: &PlatformFacts, i: usize) -> Option<&SensorNow> {
        let kind = &p.sensors.get(i)?.kind;
        let k = p.sensors[..i].iter().filter(|s| s.kind == *kind).count();
        self.sensors.iter().filter(|s| s.kind == *kind).nth(k)
    }

    /// The conditions it was computed in, in words, where they are not a clear day's: "at night",
    /// "in twilight", "with the sun 4 degrees up" (the thermal crossover), "in fog of 50 m
    /// visibility", "in 3.2 km visibility", joined; none on a clear day.
    pub fn conditions(&self) -> Option<String> {
        let sun = self.sun_elevation_deg;
        let mut parts = Vec::new();
        if sun < NIGHT_DEG {
            parts.push("at night".to_string());
        } else if sun < 0.0 {
            parts.push("in twilight".to_string());
        } else if sun < LOW_SUN_DEG {
            parts.push(format!("with the sun {sun:.0} degrees up"));
        }
        let v = self.visibility_m;
        if v < FOG_M {
            parts.push(format!("in fog of {v:.0} m visibility"));
        } else if v < POOR_VISIBILITY_M {
            parts.push(format!("in {:.1} km visibility", v / 1000.0));
        }
        (!parts.is_empty()).then(|| parts.join(" and "))
    }
}

/// Whether `p`'s sensor facts carry physical reaches (a run on the physical sensing model): the
/// sensors rules 32 read at their reach now.
pub fn physical(p: &PlatformFacts) -> bool {
    p.sensors.iter().any(|s| s.reach_m.is_some())
}

/// Rules 32: the reach-now fact a decision on `world` reads for `v`, fresh as any fact (against
/// the vehicle's freshness limit), or why not.
pub fn fresh<'v>(v: &'v VehicleFacts, world: &WorldSnapshot, cfg: &RulesConfig) -> Result<&'v ReachNow, String> {
    crate::rules::fresh(&v.reach_now, v, "sensor reach under the conditions now", world, cfg)
}

/// Rules 32: how far `v`'s sensor `i` detects a contact in `domain`, m along the ground, when the
/// engine reads its reach now (rules 32, the sensor's facts carrying a physical reach): `Some`
/// with its reach now (`None` inside: it reaches none there now, or the reach now is not known);
/// `None` when it reads the sensor's facts as before ([`crate::SensorFact::reach_for`]).
pub fn detect_now(v: &VehicleFacts, i: usize, domain: Domain, world: &WorldSnapshot, cfg: &RulesConfig) -> Option<Option<f64>> {
    let p = v.platform.as_ref()?;
    let s = p.sensors.get(i)?;
    s.reach_m.as_ref()?;
    Some(fresh(v, world, cfg).ok().and_then(|rn| rn.sensor(p, i)).and_then(|n| n.detect_m.get(domain)))
}


#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    

}
