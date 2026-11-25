//! Local metres: a tangent-plane projection of the WGS84 ellipsoid about a reference point.
//!
//! A point (latitude, longitude) is placed on the ellipsoid surface (height 0), turned to
//! Earth-centred coordinates, and projected onto the east-north plane at the reference point
//! (an orthographic projection). Altitude is carried separately, never as the plane's "up", so a
//! far aircraft does not sink with the Earth's curvature. Over the FleetMesh region (about 70 by
//! 70 km) the scale error is below 3e-5 (about 1 m at 40 km), far under any sensor's accuracy.
//!
//! Grid north and true north differ by a few tenths of a degree at the region's edge; every
//! bearing and velocity crosses the boundary through [`Local::convergence`].

use std::f64::consts::PI;

pub const WGS84_A: f64 = 6_378_137.0;
pub const WGS84_F: f64 = 1.0 / 298.257_223_563;
const E2: f64 = WGS84_F * (2.0 - WGS84_F);
/// Mean Earth radius, for the few places that need only a rough figure.
pub const EARTH_R: f64 = 6_371_000.0;

pub const KT: f64 = 1852.0 / 3600.0;
pub const FT: f64 = 0.3048;

#[derive(Clone, Copy, Debug)]
pub struct Local {
    lat0: f64,
    lon0: f64,
    sin_lat0: f64,
    cos_lat0: f64,
    sin_lon0: f64,
    cos_lon0: f64,
    ecef0: [f64; 3],
}

fn ecef_surface(lat: f64, lon: f64) -> [f64; 3] {
    let (sl, cl) = lat.sin_cos();
    let (so, co) = lon.sin_cos();
    let n = WGS84_A / (1.0 - E2 * sl * sl).sqrt();
    [n * cl * co, n * cl * so, n * (1.0 - E2) * sl]
}

/// Meridional radius of curvature.
fn radius_m(lat: f64) -> f64 {
    let s = lat.sin();
    WGS84_A * (1.0 - E2) / (1.0 - E2 * s * s).powf(1.5)
}

/// Prime-vertical radius of curvature.
fn radius_n(lat: f64) -> f64 {
    let s = lat.sin();
    WGS84_A / (1.0 - E2 * s * s).sqrt()
}

impl Local {
    pub fn new(lat_deg: f64, lon_deg: f64) -> Self {
        let lat0 = lat_deg.to_radians();
        let lon0 = lon_deg.to_radians();
        let (sin_lat0, cos_lat0) = lat0.sin_cos();
        let (sin_lon0, cos_lon0) = lon0.sin_cos();
        Local {
            lat0,
            lon0,
            sin_lat0,
            cos_lat0,
            sin_lon0,
            cos_lon0,
            ecef0: ecef_surface(lat0, lon0),
        }
    }

    pub fn origin_deg(&self) -> (f64, f64) {
        (self.lat0.to_degrees(), self.lon0.to_degrees())
    }

    fn fwd_rad(&self, lat: f64, lon: f64) -> (f64, f64) {
        let p = ecef_surface(lat, lon);
        let d = [
            p[0] - self.ecef0[0],
            p[1] - self.ecef0[1],
            p[2] - self.ecef0[2],
        ];
        let e = -self.sin_lon0 * d[0] + self.cos_lon0 * d[1];
        let n = -self.sin_lat0 * self.cos_lon0 * d[0] - self.sin_lat0 * self.sin_lon0 * d[1]
            + self.cos_lat0 * d[2];
        (e, n)
    }

    /// Degrees to local metres (x east, y north on the grid).
    pub fn to_xy(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        self.fwd_rad(lat_deg.to_radians(), lon_deg.to_radians())
    }

    /// Local metres back to degrees. A fixed-point iteration: the projection is within 1% of a
    /// plain scale near the reference, so four rounds reach well below a millimetre.
    pub fn to_latlon(&self, x: f64, y: f64) -> (f64, f64) {
        let mut lat = self.lat0 + y / radius_m(self.lat0);
        let mut lon = self.lon0 + x / (radius_n(self.lat0) * self.cos_lat0);
        for _ in 0..5 {
            let (e, n) = self.fwd_rad(lat, lon);
            lat += (y - n) / radius_m(lat);
            lon += (x - e) / (radius_n(lat) * lat.cos());
        }
        (lat.to_degrees(), wrap_lon_deg(lon.to_degrees()))
    }

    /// Grid bearing of true north at a point, radians. A true bearing is the grid bearing
    /// minus this.
    pub fn convergence(&self, lat_deg: f64, lon_deg: f64) -> f64 {
        let lat = lat_deg.to_radians();
        let lon = lon_deg.to_radians();
        let d = 1e-6;
        let (e1, n1) = self.fwd_rad(lat - d, lon);
        let (e2, n2) = self.fwd_rad(lat + d, lon);
        (e2 - e1).atan2(n2 - n1)
    }

    /// Convergence at a point given in local metres.
    pub fn convergence_xy(&self, x: f64, y: f64) -> f64 {
        let (lat, lon) = self.to_latlon(x, y);
        self.convergence(lat, lon)
    }
}

/// A true (east, north) vector to grid (x, y), given the convergence at that place.
pub fn true_to_grid(gamma: f64, e: f64, n: f64) -> (f64, f64) {
    let (s, c) = gamma.sin_cos();
    (e * c + n * s, -e * s + n * c)
}

/// A grid (x, y) vector to true (east, north).
pub fn grid_to_true(gamma: f64, x: f64, y: f64) -> (f64, f64) {
    let (s, c) = gamma.sin_cos();
    (x * c - y * s, x * s + y * c)
}

/// Wrap an angle to (-pi, pi].
pub fn wrap_pi(a: f64) -> f64 {
    let mut a = a % (2.0 * PI);
    if a > PI {
        a -= 2.0 * PI;
    } else if a <= -PI {
        a += 2.0 * PI;
    }
    a
}

/// Wrap degrees to [0, 360).
pub fn wrap_360(a: f64) -> f64 {
    let a = a % 360.0;
    if a < 0.0 { a + 360.0 } else { a }
}

fn wrap_lon_deg(a: f64) -> f64 {
    let mut a = a;
    while a > 180.0 {
        a -= 360.0;
    }
    while a <= -180.0 {
        a += 360.0;
    }
    a
}

/// Great-circle distance in metres (mean radius), for tests and rough checks.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_R * a.sqrt().asin()
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_scale() {
        let l = Local::new(37.75, -122.5);
        for &(lat, lon) in &[
            (37.45, -122.9),
            (38.05, -122.1),
            (37.75, -122.5),
            (37.8, -122.42),
        ] {
            let (x, y) = l.to_xy(lat, lon);
            let (la, lo) = l.to_latlon(x, y);
            assert!(
                (la - lat).abs() < 1e-9 && (lo - lon).abs() < 1e-9,
                "{lat} {lon} -> {la} {lo}"
            );
        }
        // Distance in the plane against the great circle, 40 km out.
        let (x1, y1) = l.to_xy(37.95, -122.2);
        let (x2, y2) = l.to_xy(37.96, -122.21);
        let d_plane = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        let d_true = haversine_m(37.95, -122.2, 37.96, -122.21);
        assert!(
            (d_plane - d_true).abs() / d_true < 3e-3,
            "{d_plane} {d_true}"
        );
    }

    #[test]
    fn convergence_is_small_and_signed() {
        let l = Local::new(37.75, -122.5);
        assert!(l.convergence(37.75, -122.5).abs() < 1e-9);
        let g = l.convergence(37.75, -122.1).to_degrees();
        // About delta-longitude times sin(latitude): 0.4 * 0.61 = 0.25 degrees.
        assert!(g.abs() > 0.2 && g.abs() < 0.3, "{g}");
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 6, 14), 20_618);
    }
}
