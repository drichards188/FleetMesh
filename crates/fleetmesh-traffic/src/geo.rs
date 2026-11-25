//! Geometry on the WGS84 sphere approximation used everywhere in FleetMesh (haversine, initial
//! bearing, destination point) and a local east/north plane for short constructions (corner
//! fillets, offsets). Distances are metres, angles degrees, bearings true.

use serde::{Deserialize, Serialize};

pub const EARTH_R_M: f64 = 6_371_000.0;
pub const KT: f64 = 1852.0 / 3600.0;
pub const FT: f64 = 0.3048;
pub const NM: f64 = 1852.0;
pub const G: f64 = 9.80665;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub const fn new(lat: f64, lon: f64) -> LatLon {
        LatLon { lat, lon }
    }
}

/// Great-circle distance.
pub fn haversine_m(a: LatLon, b: LatLon) -> f64 {
    let (p1, p2) = (a.lat.to_radians(), b.lat.to_radians());
    let dp = p2 - p1;
    let dl = (b.lon - a.lon).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_R_M * h.sqrt().min(1.0).asin()
}

/// Initial bearing from `a` to `b`, 0 to 360.
pub fn bearing_deg(a: LatLon, b: LatLon) -> f64 {
    let (p1, p2) = (a.lat.to_radians(), b.lat.to_radians());
    let dl = (b.lon - a.lon).to_radians();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    wrap360(y.atan2(x).to_degrees())
}

/// The point `dist_m` from `a` on initial bearing `brg_deg`.
pub fn destination(a: LatLon, brg_deg: f64, dist_m: f64) -> LatLon {
    let d = dist_m / EARTH_R_M;
    let b = brg_deg.to_radians();
    let p1 = a.lat.to_radians();
    let l1 = a.lon.to_radians();
    let p2 = (p1.sin() * d.cos() + p1.cos() * d.sin() * b.cos()).asin();
    let l2 = l1 + (b.sin() * d.sin() * p1.cos()).atan2(d.cos() - p1.sin() * p2.sin());
    LatLon::new(p2.to_degrees(), wrap180(l2.to_degrees()))
}

/// East and north metres of `p` from `o` (a local plane: fine within tens of kilometres).
pub fn enu(o: LatLon, p: LatLon) -> (f64, f64) {
    let n = (p.lat - o.lat).to_radians() * EARTH_R_M;
    let e = (p.lon - o.lon).to_radians() * EARTH_R_M * o.lat.to_radians().cos();
    (e, n)
}

/// Inverse of [`enu`].
pub fn from_enu(o: LatLon, e: f64, n: f64) -> LatLon {
    LatLon::new(
        o.lat + (n / EARTH_R_M).to_degrees(),
        o.lon + (e / (EARTH_R_M * o.lat.to_radians().cos())).to_degrees(),
    )
}

pub fn wrap360(a: f64) -> f64 {
    let r = a.rem_euclid(360.0);
    if r >= 360.0 { 0.0 } else { r }
}

pub fn wrap180(a: f64) -> f64 {
    let r = wrap360(a + 180.0) - 180.0;
    if r < -180.0 { r + 360.0 } else { r }
}

/// The signed turn from bearing `a` to bearing `b`, -180 to 180.
pub fn angle_diff(a: f64, b: f64) -> f64 {
    wrap180(b - a)
}

/// Linear interpolation of two points (a short segment).
pub fn lerp(a: LatLon, b: LatLon, f: f64) -> LatLon {
    LatLon::new(a.lat + (b.lat - a.lat) * f, a.lon + (b.lon - a.lon) * f)
}

/// Even-odd point in polygon (lat/lon vertices, a small polygon).
pub fn in_polygon(p: LatLon, poly: &[LatLon]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.lat > p.lat) != (b.lat > p.lat) {
            let x = a.lon + (p.lat - a.lat) / (b.lat - a.lat) * (b.lon - a.lon);
            if p.lon < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_inverts_bearing_and_distance() {
        let a = LatLon::new(37.6135, -122.3572);
        let b = destination(a, 118.0, 22_224.0);
        assert!((haversine_m(a, b) - 22_224.0).abs() < 0.5);
        assert!((bearing_deg(a, b) - 118.0).abs() < 0.01);
        assert!((angle_diff(350.0, 10.0) - 20.0).abs() < 1e-9);
        assert!((angle_diff(10.0, 350.0) + 20.0).abs() < 1e-9);
        let (e, n) = enu(a, b);
        let c = from_enu(a, e, n);
        assert!(haversine_m(b, c) < 5.0);
    }
}
