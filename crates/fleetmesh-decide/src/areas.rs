//! Operator areas: the approved operating area(s) and no-go zones, from GeoJSON
//! (`sim/areas/*.geojson`).
//!
//! Each feature needs `properties.id` and `properties.kind` (`"approved"`, `"no_go"`,
//! `Polygon` or `MultiPolygon`. Anything else is a load error: a silently skipped no-go zone
//! would be a fail-open.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::geo::{LonLat, Polygon, TaskPath};
use crate::hash::sha256_hex;
use crate::time::Time;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AreaKind {
    /// Where the operator has approved operations. Leaving it needs a human (policy).
    Approved,
    /// No task path may enter it (geofence rule).
    NoGo,
    /// Entry needs a person: an airspace class, a school at pickup time, a ship channel
    /// (delivery-rules.md section 4).
    Restricted,
    /// People live or gather here. No refusal, but routes over it are marked and several rules
    /// react (a plane's hold, abort routing).
    Populated,
    /// An approved landing area outside a hub.
    Landing,
}

impl AreaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AreaKind::Approved => "approved",
            AreaKind::NoGo => "no_go",
            AreaKind::Restricted => "restricted",
            AreaKind::Populated => "populated",
            AreaKind::Landing => "landing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub id: String,
    pub name: String,
    pub kind: AreaKind,
    pub polygons: Vec<Polygon>,
    /// A ceiling, m above the ground: the most a route's altitude may be inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_m: Option<f64>,
    /// A temporary area is in force from `valid_from` (absent: always) until `valid_until`
    /// (absent: no end); `valid_from` or `valid_until` makes it temporary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<Time>,
    /// Where a temporary restriction comes from (a notice reference, an integration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
}

impl Area {
    /// Whether the area has a time window.
    pub fn is_temporary(&self) -> bool {
        self.valid_from.is_some() || self.valid_until.is_some()
    }

    /// Whether the area is in force at `t`.
    pub fn active_at(&self, t: Time) -> bool {
        self.valid_from.is_none_or(|f| t >= f) && self.valid_until.is_none_or(|u| t < u)
    }

    pub fn contains(&self, p: LonLat) -> bool {
        self.polygons.iter().any(|poly| poly.contains(p))
    }

    pub fn segment_touches_boundary(&self, a: LonLat, b: LonLat) -> bool {
        self.polygons
            .iter()
            .any(|poly| poly.segment_touches_boundary(a, b))
    }

    /// Why `path` enters this area, or None if it stays clear.
    pub fn intrusion(&self, path: &TaskPath) -> Option<String> {
        for (i, p) in path.points.iter().enumerate() {
            if self.contains(*p) {
                return Some(format!("waypoint {} is inside {}", i + 1, self.id));
            }
        }
        for (i, a, b) in path.legs() {
            if self.segment_touches_boundary(a, b) {
                let to = if i + 1 == path.points.len() { 1 } else { i + 2 };
                return Some(format!("leg {}-{} crosses {}", i + 1, to, self.id));
            }
        }
        None
    }
}

/// The areas in force, with where and when they were loaded (both are logged as inputs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AreaSet {
    pub source: String,
    pub loaded_t: Time,
    pub areas: Vec<Area>,
}

#[derive(Debug, thiserror::Error)]
pub enum AreaError {
    #[error("{source_name}: {msg}")]
    Invalid { source_name: String, msg: String },
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
}

impl AreaSet {
    pub fn empty(source: impl Into<String>, loaded_t: Time) -> Self {
        AreaSet {
            source: source.into(),
            loaded_t,
            areas: Vec::new(),
        }
    }

    /// SHA-256 (hex) of the canonical serialization: compact `serde_json` of this struct
    /// (fields in declaration order, areas and points in file order, no maps, floats in
    /// shortest round-trip form). The audit log stores the set once under this hash and
    /// decision events reference it; replay re-serializes the logged set and checks it.
    pub fn content_hash(&self) -> Result<String, serde_json::Error> {
        Ok(sha256_hex(serde_json::to_string(self)?.as_bytes()))
    }

    pub fn approved(&self) -> impl Iterator<Item = &Area> {
        self.areas.iter().filter(|a| a.kind == AreaKind::Approved)
    }

    pub fn no_go(&self) -> impl Iterator<Item = &Area> {
        self.areas.iter().filter(|a| a.kind == AreaKind::NoGo)
    }

    /// Parse one GeoJSON FeatureCollection.
    pub fn from_geojson(source: &str, text: &str, loaded_t: Time) -> Result<Self, AreaError> {
        let bad = |msg: String| AreaError::Invalid {
            source_name: source.to_string(),
            msg,
        };
        let doc: Value = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        if doc.get("type").and_then(Value::as_str) != Some("FeatureCollection") {
            return Err(bad("expected a GeoJSON FeatureCollection".into()));
        }
        let features = doc
            .get("features")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("missing features".into()))?;
        let mut areas = Vec::new();
        for (i, f) in features.iter().enumerate() {
            areas.push(parse_feature(f).map_err(|m| bad(format!("feature {i}: {m}")))?);
        }
        Ok(AreaSet {
            source: source.to_string(),
            loaded_t,
            areas,
        })
    }

    /// Load every `*.geojson` in a directory, in file-name order, as one set.
    pub fn load_dir(dir: &Path, loaded_t: Time) -> Result<Self, AreaError> {
        let io = |e| AreaError::Io(dir.display().to_string(), e);
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map_err(io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "geojson"))
            .collect();
        files.sort();
        let mut set = AreaSet::empty(dir.display().to_string(), loaded_t);
        let mut names = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f)
                .map_err(|e| AreaError::Io(f.display().to_string(), e))?;
            let one = AreaSet::from_geojson(&f.display().to_string(), &text, loaded_t)?;
            set.areas.extend(one.areas);
            names.push(
                f.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        set.source = format!("{} [{}]", dir.display(), names.join(", "));
        Ok(set)
    }
}

fn parse_feature(f: &Value) -> Result<Area, String> {
    let props = f.get("properties").ok_or("missing properties")?;
    let id = props
        .get("id")
        .and_then(Value::as_str)
        .ok_or("properties.id (string) is required")?
        .to_string();
    let name = props
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let kind = match props.get("kind").and_then(Value::as_str) {
        Some("approved") => AreaKind::Approved,
        Some("no_go") => AreaKind::NoGo,
        Some("restricted") => AreaKind::Restricted,
        Some("populated") => AreaKind::Populated,
        Some("landing") => AreaKind::Landing,
        other => {
            return Err(format!(
                "{id}: properties.kind must be \"approved\", \"no_go\", \"restricted\", \"populated\" or \"landing\", got {other:?}"
            ));
        }
    };
    let ceiling_m = match props.get("ceiling_m") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|c| c.is_finite() && *c > 0.0)
                .ok_or_else(|| format!("{id}: properties.ceiling_m must be a positive number of metres"))?,
        ),
    };
    let time = |key: &str| -> Result<Option<Time>, String> {
        match props.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => {
                let text = v.as_str().ok_or_else(|| format!("{id}: properties.{key} must be an RFC 3339 time"))?;
                let t = chrono::DateTime::parse_from_rfc3339(text)
                    .map_err(|e| format!("{id}: properties.{key}: {e}"))?;
                Ok(Some(Time(t.timestamp_millis())))
            }
        }
    };
    let (valid_from, valid_until) = (time("valid_from")?, time("valid_until")?);
    if let (Some(f), Some(u)) = (valid_from, valid_until)
        && u <= f
    {
        return Err(format!("{id}: valid_until must be after valid_from"));
    }
    let source_ref = props
        .get("notam_ref")
        .or_else(|| props.get("source"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let geom = f.get("geometry").ok_or("missing geometry")?;
    let coords = geom.get("coordinates").ok_or("missing coordinates")?;
    let polygons = match geom.get("type").and_then(Value::as_str) {
        Some("Polygon") => vec![parse_polygon(coords)?],
        Some("MultiPolygon") => coords
            .as_array()
            .ok_or("MultiPolygon coordinates must be an array")?
            .iter()
            .map(parse_polygon)
            .collect::<Result<_, _>>()?,
        other => {
            return Err(format!(
                "{id}: geometry must be Polygon or MultiPolygon, got {other:?}"
            ));
        }
    };
    Ok(Area {
        id,
        name,
        kind,
        polygons,
        ceiling_m,
        valid_from,
        valid_until,
        source_ref,
    })
}

fn parse_polygon(v: &Value) -> Result<Polygon, String> {
    let rings = v.as_array().ok_or("polygon must be an array of rings")?;
    let mut parsed = rings.iter().map(parse_ring);
    let exterior = parsed.next().ok_or("polygon has no rings")??;
    let holes = parsed.collect::<Result<Vec<_>, _>>()?;
    Ok(Polygon { exterior, holes })
}

fn parse_ring(v: &Value) -> Result<Vec<LonLat>, String> {
    let pts = v.as_array().ok_or("ring must be an array of positions")?;
    let ring: Vec<LonLat> = pts
        .iter()
        .map(|p| {
            let a = p.as_array().ok_or("position must be an array")?;
            let lon = a.first().and_then(Value::as_f64).ok_or("bad longitude")?;
            let lat = a.get(1).and_then(Value::as_f64).ok_or("bad latitude")?;
            let ll = LonLat::new(lon, lat);
            if ll.is_valid() {
                Ok(ll)
            } else {
                Err("position out of range")
            }
        })
        .collect::<Result<_, _>>()?;
    if ring.len() < 3 {
        return Err("ring needs at least 3 positions".into());
    }
    Ok(ring)
}
