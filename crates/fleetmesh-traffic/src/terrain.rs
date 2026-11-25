//! Radio line of sight between a receiver and a transmitter.
//!
//! Three parts, in order of cost:
//! 1. The radio horizon for an effective Earth of `k` radii (4/3 for a standard atmosphere):
//!    `sqrt(2 k R h1) + sqrt(2 k R h2)`.
//! 2. The baked surface grid the headless link emulator already reads
//!    (`sim/terrain/golden-gate.grid.gz`, eval/headless/bake_world.py: tidewater's
//!    `max(elevation, water level)` on a 20 m grid around the Golden Gate). It covers the Marin
//!    Headlands, the Presidio, Angel Island and Alcatraz; where it has no data the surface is
//!    taken as sea level.
//! 3. A mask of named hills beyond the grid (Twin Peaks, San Bruno Mountain, Mount Tamalpais,
//!    Yerba Buena Island, the East Bay hills), each a smooth dome, configured in sim/traffic.toml.
//!
//! The profile is judged by the link emulator's own [`path_obstruction_m`] (Earth bulge, no
//! Fresnel clearance): positive means the terrain stands above the ray by that much.

use std::io::Read;
use std::path::Path;

use fleetmesh_linkemu::geo::LosParams;
use fleetmesh_linkemu::terrain::path_obstruction_m;
use serde::Deserialize;

use crate::geo::{self, EARTH_R_M, LatLon};

#[derive(Debug, Clone, Deserialize)]
struct GridHeader {
    lat0: f64,
    lon0: f64,
    dlat: f64,
    dlon: f64,
    nlat: usize,
    nlon: usize,
    scale: f64,
    nodata: i16,
}

/// A baked grid: gzip of one JSON header line, then `nlat x nlon` little-endian i16, row by row
/// from the south-west (the format crates/fleetmesh-bridge/src/decision/terrain.rs reads).
pub struct Grid {
    h: GridHeader,
    data: Vec<i16>,
    pub source: String,
}

impl Grid {
    pub fn load(path: &Path) -> Result<Grid, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(file)
            .read_to_end(&mut raw)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let nl = raw
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| format!("{}: no header line", path.display()))?;
        let h: GridHeader =
            serde_json::from_slice(&raw[..nl]).map_err(|e| format!("{}: header: {e}", path.display()))?;
        let body = &raw[nl + 1..];
        if body.len() != h.nlat * h.nlon * 2 {
            return Err(format!("{}: {} bytes for a {}x{} grid", path.display(), body.len(), h.nlat, h.nlon));
        }
        let data = body.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect();
        Ok(Grid { h, data, source: path.display().to_string() })
    }

    fn at(&self, i: usize, j: usize) -> Option<f64> {
        let v = *self.data.get(i * self.h.nlon + j)?;
        (v != self.h.nodata).then(|| f64::from(v) * self.h.scale)
    }

    /// Bilinear height at a point; None outside the grid or next to a sample without data.
    pub fn height(&self, lat: f64, lon: f64) -> Option<f64> {
        let y = (lat - self.h.lat0) / self.h.dlat;
        let x = (lon - self.h.lon0) / self.h.dlon;
        if !(y >= 0.0 && x >= 0.0) {
            return None;
        }
        let (i, j) = (y.floor() as usize, x.floor() as usize);
        if i + 1 >= self.h.nlat || j + 1 >= self.h.nlon {
            return None;
        }
        let (fy, fx) = (y - i as f64, x - j as f64);
        let a = self.at(i, j)?;
        let b = self.at(i, j + 1)?;
        let c = self.at(i + 1, j)?;
        let d = self.at(i + 1, j + 1)?;
        Some((a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy)
    }

    fn bbox(&self) -> (f64, f64, f64, f64) {
        (
            self.h.lat0,
            self.h.lat0 + self.h.dlat * self.h.nlat as f64,
            self.h.lon0,
            self.h.lon0 + self.h.dlon * self.h.nlon as f64,
        )
    }
}

/// A smooth dome: `top_m * cos^2(pi/2 * r / radius_m)` inside its radius.
#[derive(Debug, Clone, Deserialize)]
pub struct Hill {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub top_m: f64,
    pub radius_m: f64,
}

impl Hill {
    fn height(&self, p: LatLon) -> f64 {
        let (e, n) = geo::enu(LatLon::new(self.lat, self.lon), p);
        let r = (e * e + n * n).sqrt();
        if r >= self.radius_m {
            0.0
        } else {
            let c = (std::f64::consts::FRAC_PI_2 * r / self.radius_m).cos();
            self.top_m * c * c
        }
    }
}

/// Why a path is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    Horizon,
    Terrain,
}

pub struct Terrain {
    grid: Option<Grid>,
    grid_box: (f64, f64, f64, f64),
    hills: Vec<Hill>,
    k: f64,
    sample_m: f64,
    max_top_m: f64,
}

impl Terrain {
    pub fn new(grid: Option<Grid>, hills: Vec<Hill>, k: f64, sample_m: f64) -> Terrain {
        let grid_box = grid.as_ref().map_or((0.0, 0.0, 0.0, 0.0), Grid::bbox);
        let max_top_m = hills.iter().map(|h| h.top_m).fold(400.0, f64::max);
        Terrain { grid, grid_box, hills, k, sample_m: sample_m.max(10.0), max_top_m }
    }

    /// No terrain: the radio horizon only.
    pub fn flat() -> Terrain {
        Terrain::new(None, Vec::new(), 4.0 / 3.0, 100.0)
    }

    pub fn grid_source(&self) -> Option<&str> {
        self.grid.as_ref().map(|g| g.source.as_str())
    }

    pub fn hills(&self) -> usize {
        self.hills.len()
    }

    /// The surface a path must clear at a point, metres above sea level.
    pub fn surface_m(&self, p: LatLon) -> f64 {
        let mut h: f64 = 0.0;
        let (la0, la1, lo0, lo1) = self.grid_box;
        if p.lat >= la0 && p.lat <= la1 && p.lon >= lo0 && p.lon <= lo1
            && let Some(g) = self.grid.as_ref().and_then(|g| g.height(p.lat, p.lon))
        {
            h = h.max(g);
        }
        for hill in &self.hills {
            if (p.lat - hill.lat).abs() * 111_320.0 < hill.radius_m {
                h = h.max(hill.height(p));
            }
        }
        h
    }

    /// The radio horizon distance for two antennas at these heights above sea level.
    pub fn horizon_m(&self, h1_m: f64, h2_m: f64) -> f64 {
        let kr = 2.0 * self.k * EARTH_R_M;
        (kr * h1_m.max(0.5)).sqrt() + (kr * h2_m.max(0.5)).sqrt()
    }

    /// Whether `a` (at `ha_m` MSL) and `b` (at `hb_m` MSL) see each other.
    pub fn line_of_sight(&self, a: LatLon, ha_m: f64, b: LatLon, hb_m: f64) -> Result<(), Blocked> {
        let d = geo::haversine_m(a, b);
        if d > self.horizon_m(ha_m, hb_m) {
            return Err(Blocked::Horizon);
        }
        if d < 2.0 * self.sample_m {
            return Ok(());
        }
        // The ray's lowest point minus the bulge clears every hill: nothing to sample.
        let bulge = d * d / (8.0 * self.k * EARTH_R_M);
        if ha_m.min(hb_m) - bulge > self.max_top_m + 10.0 {
            return Ok(());
        }
        let n = ((d / self.sample_m).ceil() as usize).clamp(2, 4_000);
        let surface: Vec<f64> = (1..n).map(|i| self.surface_m(geo::lerp(a, b, i as f64 / n as f64))).collect();
        let p = LosParams { gs_antenna_m: 0.0, antenna_m: 0.0, k: self.k, fresnel_mhz: 0.0, fresnel_clear: 0.0 };
        if path_obstruction_m(&surface, d, ha_m, hb_m, &p) > 0.0 { Err(Blocked::Terrain) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hill_between_blocks_a_low_path_and_not_a_high_one() {
        let hill = Hill { name: "dome".into(), lat: 37.80, lon: -122.45, top_m: 250.0, radius_m: 1_000.0 };
        let t = Terrain::new(None, vec![hill], 4.0 / 3.0, 50.0);
        let a = LatLon::new(37.80, -122.50);
        let b = LatLon::new(37.80, -122.40);
        assert_eq!(t.line_of_sight(a, 10.0, b, 10.0), Err(Blocked::Terrain));
        assert_eq!(t.line_of_sight(a, 10.0, b, 600.0), Ok(()));
        // 15 m and 3 m antennas: about 22.9 km (4.12 (sqrt 15 + sqrt 3) km)
        let h = t.horizon_m(15.0, 3.0);
        assert!((h - 23_100.0).abs() < 300.0, "{h}");
        let far = geo::destination(a, 270.0, 30_000.0);
        assert_eq!(t.line_of_sight(a, 15.0, far, 3.0), Err(Blocked::Horizon));
    }

    #[test]
    fn the_marin_headlands_hide_the_ocean_north_of_point_bonita_from_crissy_field() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sim/terrain/golden-gate.grid.gz");
        let Ok(grid) = Grid::load(&path) else { return };
        let t = Terrain::new(Some(grid), Vec::new(), 4.0 / 3.0, 40.0);
        let crissy = LatLon::new(37.8040, -122.4650);
        // a fishing boat off Rodeo Beach, behind the headlands
        let rodeo = LatLon::new(37.8300, -122.5450);
        assert_eq!(t.line_of_sight(crissy, 14.0, rodeo, 3.0), Err(Blocked::Terrain));
        // a ship under the middle of the Golden Gate Bridge, in plain view from the Pier hub
        let pier = LatLon::new(37.8170, -122.4150);
        let span = LatLon::new(37.8180, -122.4790);
        assert_eq!(t.line_of_sight(pier, 13.0, span, 30.0), Ok(()));
        // but not a boat in the eastbound lane beyond Fort Point from Crissy Field: the
        // Presidio bluffs stand in the way
        let lane = LatLon::new(37.8050, -122.5000);
        assert_eq!(t.line_of_sight(crissy, 14.0, lane, 3.0), Err(Blocked::Terrain));
    }
}
