//! The tracker: identities, filters, association, lifecycle, flags and the output picture.

use crate::ais::{self, AisMessage};
use crate::bearing::{Lob, LobObs, R_MIN};
use crate::config::{
    Config, OwnFleet, OwnVehicle, SensorSite, SiteKind, ais_a_interval_ms, ais_b_interval_ms,
};
use crate::geo::{FT, KT, Local, grid_to_true, wrap_360};
use crate::kf::{Innov, Kf, M6, MAXM, ellipse, lin_components, propagate};
use crate::meas::{Polar, noise1, noise2, speed_course_cov};
use crate::nmea::{self, Assembler, NmeaError};
use crate::sbs::{self, SbsKind};
use crate::track::{
    AltRef, BearingLine, Class, Disagreement, Domain, Flag, Ids, Kinematics, Prediction, Source,
    SourceKind, Static, Track, TrackState,
};
use crate::wire::{self, Det, JsonLine, Rid, SensorKind};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

/// What one ingest call did with its line.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Used: a track was created or updated.
    Fused,
    /// The same transmission was already heard by another receiver; only the receiver was noted.
    Duplicate,
    /// Held: the first fragments of a multi-sentence message, or a detection waiting for its scan.
    Pending,
    /// Valid but not used (own fleet, own vessel, a message type without traffic content).
    Ignored(&'static str),
    /// Not decodable.
    Error(String),
}

/// Counters since the tracker was created, and the picture's make-up at the last `update`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    pub sbs_lines: u64,
    pub sbs_errors: u64,
    pub sbs_ignored: u64,
    pub nmea_lines: u64,
    pub nmea_errors: u64,
    pub nmea_checksum_errors: u64,
    pub nmea_fragments_dropped: u64,
    pub ais_messages: u64,
    pub ais_errors: u64,
    pub ais_other_types: u64,
    pub ais_own_vessel: u64,
    pub json_lines: u64,
    pub json_errors: u64,
    pub rid_reports: u64,
    pub det_reports: u64,
    pub det_errors: u64,
    pub duplicates: u64,
    pub late_reports: u64,
    pub own_ignored: u64,
    pub own_detections: u64,
    pub no_position_reports: u64,
    pub jumps_rejected: u64,
    pub det_corroborated: u64,
    pub det_updated: u64,
    pub det_new_tracks: u64,
    pub det_bearing_only_new: u64,
    pub triangulated: u64,
    pub tracks_created: u64,
    pub tracks_dropped: u64,
    pub tracks_merged: u64,
    pub handovers: u64,
    pub tracks_live: u64,
    pub tracks_confirmed: u64,
    pub tracks_coasting: u64,
    pub tracks_tentative: u64,
    pub tracks_cooperative: u64,
    pub tracks_bearing_only: u64,
    pub disagreements: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Identity {
    Icao { addr: u32, non_icao: bool },
    Mmsi(u32),
    Uas(String),
}

type SensorKey = (SensorKind, String);

/// A cooperative track's twin: (sensed track, sensor, distance).
type Pairing = Option<(u64, String, f64)>;

#[derive(Debug, Clone)]
struct SrcRec {
    kind: SourceKind,
    rx: String,
    last_t: i64,
    /// Last time this source moved the track's state.
    last_kin_t: Option<i64>,
    interval_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct DisagreeState {
    candidate_since: Option<i64>,
    until: i64,
    pair: Option<(u64, String, f64, i64)>,
}

#[derive(Debug, Clone)]
struct Rec {
    no: u64,
    identity: Option<Identity>,
    domain: Domain,
    class: Class,
    class_from_source: bool,
    cooperative: bool,
    kf: Option<Kf>,
    lob: Option<Lob>,
    alt_ref: AltRef,
    alt_known: bool,
    ids: Ids,
    statics: Static,
    sources: Vec<SrcRec>,
    confirmed: bool,
    pos_reports: u32,
    first_t: i64,
    /// First report of the current identity.
    first_id_t: i64,
    /// Last report of the identity source (any content).
    last_report_t: i64,
    primary: SourceKind,
    interval_ms: i64,
    anonymous: bool,
    ladd: bool,
    on_ground: bool,
    nav_status: Option<u8>,
    invalid_mmsi: bool,
    emergency: bool,
    jump_until: i64,
    id_changed_until: i64,
    triangulated: bool,
    heading: Option<(f64, i64)>,
    pending_vel: Option<(i64, f64, f64, f64, f64)>,
    pending_alt: Option<(i64, f64, AltRef, f64)>,
    rejects: Vec<(i64, f64, f64)>,
    corroborated: Vec<(SensorKey, i64)>,
    hit_times: Vec<i64>,
    init_sensor: Option<SensorKey>,
    scan_ms: i64,
    disagree: DisagreeState,
}

impl Rec {
    fn new(no: u64, domain: Domain, cooperative: bool, primary: SourceKind, t: i64) -> Rec {
        Rec {
            no,
            identity: None,
            domain,
            class: Class::unknown(domain),
            class_from_source: false,
            cooperative,
            kf: None,
            lob: None,
            alt_ref: AltRef::None,
            alt_known: false,
            ids: Ids::default(),
            statics: Static::default(),
            sources: Vec::new(),
            confirmed: false,
            pos_reports: 0,
            first_t: t,
            first_id_t: t,
            last_report_t: t,
            primary,
            interval_ms: 1_000,
            anonymous: false,
            ladd: false,
            on_ground: false,
            nav_status: None,
            invalid_mmsi: false,
            emergency: false,
            jump_until: i64::MIN,
            id_changed_until: i64::MIN,
            triangulated: false,
            heading: None,
            pending_vel: None,
            pending_alt: None,
            rejects: Vec::new(),
            corroborated: Vec::new(),
            hit_times: Vec::new(),
            init_sensor: None,
            scan_ms: 1_000,
            disagree: DisagreeState::default(),
        }
    }

    fn upsert_source(&mut self, kind: SourceKind, rx: &str, t: i64, kin: bool, interval_ms: i64) {
        if let Some(s) = self
            .sources
            .iter_mut()
            .find(|s| s.kind == kind && s.rx == rx)
        {
            s.last_t = s.last_t.max(t);
            if kin {
                s.last_kin_t = Some(s.last_kin_t.map_or(t, |k| k.max(t)));
                s.interval_ms = interval_ms;
            }
        } else {
            self.sources.push(SrcRec {
                kind,
                rx: rx.to_string(),
                last_t: t,
                last_kin_t: if kin { Some(t) } else { None },
                interval_ms,
            });
        }
    }

    fn corroborate(&mut self, key: &SensorKey, t: i64) {
        if let Some(c) = self.corroborated.iter_mut().find(|c| &c.0 == key) {
            c.1 = c.1.max(t);
        } else {
            self.corroborated.push((key.clone(), t));
        }
    }

    fn add_hit(&mut self, t: i64) {
        let half = self.scan_ms / 2;
        if self.hit_times.last().is_none_or(|&l| t - l >= half) {
            self.hit_times.push(t);
            if self.hit_times.len() > 16 {
                self.hit_times.remove(0);
            }
        }
    }

    /// When this track starts coasting and when it is dropped, from its sources' silences.
    fn deadlines(&self, cfg: &Config) -> (i64, i64) {
        if self.kf.is_none()
            && let Some(lob) = &self.lob
        {
            let t = lob.latest().map_or(self.first_t, |o| o.t);
            return (t + cfg.bearing_coast_ms, t + cfg.bearing_drop_ms);
        }
        let mut coast = i64::MIN;
        let mut drop = i64::MIN;
        for s in &self.sources {
            if let Some(k) = s.last_kin_t {
                let tm = cfg.timing(s.kind);
                coast = coast.max(k + tm.coast_ms(s.interval_ms));
                drop = drop.max(k + tm.drop_ms(s.interval_ms));
            }
        }
        if drop == i64::MIN {
            // A track that never had kinematics (identity only) is kept a short while.
            let t = self.last_report_t + 60_000;
            return (t, t);
        }
        (coast, drop)
    }

    /// Time of the last kinematic report from the track's own kind of source.
    fn last_primary_kin(&self) -> Option<i64> {
        let coop = self.cooperative;
        self.sources
            .iter()
            .filter(|s| s.kind.cooperative() == coop)
            .filter_map(|s| s.last_kin_t)
            .max()
    }

    fn coop_fresh(&self, cfg: &Config, t: i64) -> bool {
        let tm = cfg.timing(self.primary);
        self.sources
            .iter()
            .filter(|s| s.kind.cooperative())
            .filter_map(|s| s.last_kin_t)
            .any(|k| t - k <= tm.coast_ms(self.interval_ms))
    }
}

/// A cooperative report, decoded from any of the formats.
#[derive(Debug, Clone)]
struct Report {
    id: Identity,
    kind: SourceKind,
    rx: String,
    t: i64,
    domain: Domain,
    pos: Option<(f64, f64, f64)>,
    alt: Option<(f64, AltRef, f64)>,
    /// Speed m/s, course degrees true, sigma speed, sigma course (radians).
    vel: Option<(f64, f64, f64, f64)>,
    vz: Option<(f64, f64)>,
    interval_ms: i64,
    on_ground: Option<bool>,
    callsign: Option<String>,
    anonymous: bool,
    emergency: Option<bool>,
    nav_status: Option<u8>,
    heading: Option<f64>,
    class: Option<Class>,
    statics: Static,
    invalid_mmsi: bool,
}

impl Report {
    fn new(
        id: Identity,
        kind: SourceKind,
        rx: String,
        t: i64,
        domain: Domain,
        interval_ms: i64,
    ) -> Report {
        Report {
            id,
            kind,
            rx,
            t,
            domain,
            pos: None,
            alt: None,
            vel: None,
            vz: None,
            interval_ms,
            on_ground: None,
            callsign: None,
            anonymous: false,
            emergency: None,
            nav_status: None,
            heading: None,
            class: None,
            statics: Static::default(),
            invalid_mmsi: false,
        }
    }

    fn kinematic(&self) -> bool {
        self.pos.is_some() || self.vel.is_some() || self.alt.is_some()
    }
}

#[derive(Debug, Clone)]
struct DetIn {
    kind: SensorKind,
    platform: String,
    t: i64,
    polar: Polar,
    r_max: f64,
    domain: Domain,
    scan_ms: i64,
    small: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CandKind {
    Track,
    LobSame,
    LobCross,
    LobFix,
}

#[derive(Debug, Clone)]
struct Cand {
    cost: f64,
    det: usize,
    rec: u64,
    kind: CandKind,
    innov: Option<Innov>,
}

#[derive(Debug, Clone)]
struct Platform {
    t: i64,
    s: [f64; 3],
    vel: Option<[f64; 3]>,
    last_det_t: i64,
    range_m: f64,
    scan_ms: i64,
}

#[derive(Debug, Clone)]
struct OwnLocal {
    uid: String,
    t: i64,
    p: [f64; 3],
}

/// The company-side traffic picture.
pub struct Tracker {
    cfg: Config,
    local: Local,
    recs: BTreeMap<u64, Rec>,
    by_id: HashMap<Identity, u64>,
    next_no: u64,
    assembler: Assembler,
    dedup: HashMap<u64, (i64, u64)>,
    pending: BTreeMap<SensorKey, Vec<DetIn>>,
    platforms: HashMap<SensorKey, Platform>,
    sites: BTreeMap<String, SensorSite>,
    sbs_sessions: HashMap<String, String>,
    own_icao: HashSet<u32>,
    own_mmsi: HashSet<u32>,
    own_uas: HashSet<String>,
    own_vehicles: Vec<OwnLocal>,
    anon_ranges: Vec<(u32, u32)>,
    ladd: HashSet<u32>,
    classes: HashMap<u32, Class>,
    out: Vec<Track>,
    disagreements: Vec<Disagreement>,
    stats: Stats,
    now: i64,
}

fn hash64<T: Hash>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Upper chi-square quantile (Wilson-Hilferty with a rational normal quantile).
pub fn chi2_quantile(p: f64, k: usize) -> f64 {
    let q = (1.0 - p).clamp(1e-12, 0.5);
    let t = (-2.0 * q.ln()).sqrt();
    let z = t
        - (2.515_517 + 0.802_853 * t + 0.010_328 * t * t)
            / (1.0 + 1.432_788 * t + 0.189_269 * t * t + 0.001_308 * t * t * t);
    let k = k as f64;
    let a = 2.0 / (9.0 * k);
    (k * (1.0 - a + z * a.sqrt()).powi(3)).max(0.0)
}

/// A filter started from a bearing-line fit.
fn kf_from_fit(f: &crate::bearing::Fit, sensor_alt: f64, cfg: &Config) -> Kf {
    let mut p: M6 = [[0.0; 6]; 6];
    let map = [0usize, 1, 3, 4];
    for (a, &i) in map.iter().enumerate() {
        for (b, &j) in map.iter().enumerate() {
            p[i][j] = f.c[a][b];
        }
    }
    let (z, zs) = f.z.unwrap_or((sensor_alt + 150.0, 400.0));
    p[2][2] = zs * zs;
    p[5][5] = 4.0;
    let x = [f.x[0], f.x[1], z, f.x[2], f.x[3], 0.0];
    Kf::new(f.t, x, p, cfg.q_noncoop_air, cfg.q_air_vertical, false)
}

fn round_to(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

fn source_of(kind: SensorKind) -> SourceKind {
    match kind {
        SensorKind::Daa => SourceKind::Daa,
        SensorKind::MarineRadar => SourceKind::MarineRadar,
        SensorKind::GroundRadar => SourceKind::GroundRadar,
    }
}

/// Three letters and a flight number, the shape of an airline's ICAO callsign.
fn airline_callsign(cs: &str) -> bool {
    let b = cs.as_bytes();
    b.len() >= 4
        && b.len() <= 7
        && b[..3].iter().all(|c| c.is_ascii_uppercase())
        && b[3].is_ascii_digit()
        && b[3..].iter().all(|c| c.is_ascii_alphanumeric())
}

fn eta_text(eta: (u8, u8, u8, u8)) -> Option<String> {
    let (mo, d, h, mi) = eta;
    if (1..=12).contains(&mo) && (1..=31).contains(&d) && h < 24 && mi < 60 {
        Some(format!("{mo:02}-{d:02}T{h:02}:{mi:02}Z"))
    } else {
        None
    }
}

fn merge_static(into: &mut Static, from: &Static) {
    macro_rules! take {
        ($($f:ident),*) => { $( if from.$f.is_some() { into.$f = from.$f.clone(); } )* };
    }
    take!(
        name,
        callsign,
        imo,
        ship_type,
        ship_type_text,
        length_m,
        beam_m,
        draught_m,
        destination,
        eta,
        nav_status,
        nav_status_text,
        mothership_mmsi,
        squawk,
        id_type,
        ua_type,
        op_lat,
        op_lon
    );
}

fn dims(bow: u16, stern: u16, port: u8, stbd: u8, s: &mut Static) {
    let l = bow as f64 + stern as f64;
    let b = port as f64 + stbd as f64;
    if l > 0.0 {
        s.length_m = Some(l);
    }
    if b > 0.0 {
        s.beam_m = Some(b);
    }
}

impl Tracker {
    pub fn new(config: Config) -> Tracker {
        let local = Local::new(config.origin_lat, config.origin_lon);
        let mut t = Tracker {
            local,
            recs: BTreeMap::new(),
            by_id: HashMap::new(),
            next_no: 1,
            assembler: Assembler::new(config.nmea_fragment_ms),
            dedup: HashMap::new(),
            pending: BTreeMap::new(),
            platforms: HashMap::new(),
            sites: BTreeMap::new(),
            sbs_sessions: HashMap::new(),
            own_icao: HashSet::new(),
            own_mmsi: HashSet::new(),
            own_uas: HashSet::new(),
            own_vehicles: Vec::new(),
            anon_ranges: Vec::new(),
            ladd: HashSet::new(),
            classes: HashMap::new(),
            out: Vec::new(),
            disagreements: Vec::new(),
            stats: Stats::default(),
            now: i64::MIN,
            cfg: config,
        };
        t.apply_config_lists();
        t
    }

    fn apply_config_lists(&mut self) {
        let hex = |s: &str| u32::from_str_radix(s.trim().trim_start_matches('~'), 16).ok();
        self.anon_ranges = self
            .cfg
            .anonymous_icao_ranges
            .iter()
            .filter_map(|(a, b)| Some((hex(a)?, hex(b)?)))
            .collect();
        self.ladd = self.cfg.ladd_icao.iter().filter_map(|s| hex(s)).collect();
        self.classes = self
            .cfg
            .aircraft_classes
            .iter()
            .filter_map(|(k, v)| Some((hex(k)?, *v)))
            .collect();
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The local-metres projection the tracker uses.
    pub fn local(&self) -> &Local {
        &self.local
    }

    /// Receivers and sensor sites (the simulator's `/sensors`). Replaces the previous list.
    pub fn set_sensors(&mut self, sites: Vec<SensorSite>) {
        self.sites.clear();
        self.sbs_sessions.clear();
        for s in sites {
            if let Some(n) = &s.sbs_session {
                self.sbs_sessions.insert(n.clone(), s.id.clone());
            }
            let kind = match s.kind {
                SiteKind::GroundRadar => Some(SensorKind::GroundRadar),
                SiteKind::MarineRadar => Some(SensorKind::MarineRadar),
                SiteKind::Daa => Some(SensorKind::Daa),
                _ => None,
            };
            if let (Some(k), Some(lat), Some(lon)) = (kind, s.lat, s.lon) {
                let (x, y) = self.local.to_xy(lat, lon);
                let key = (k, s.id.clone());
                let range_m = s.range_m.unwrap_or_else(|| self.default_range(k));
                let scan_ms = s.scan_ms.unwrap_or_else(|| self.default_scan(k));
                let p = self.platforms.entry(key).or_insert(Platform {
                    t: i64::MIN,
                    s: [x, y, s.alt_m.unwrap_or(0.0)],
                    vel: None,
                    last_det_t: i64::MIN,
                    range_m,
                    scan_ms,
                });
                p.s = [x, y, s.alt_m.unwrap_or(0.0)];
                p.range_m = range_m;
                p.scan_ms = scan_ms;
            }
            self.sites.insert(s.id.clone(), s);
        }
    }

    /// The company's own vehicles' identities: their broadcasts are not traffic.
    pub fn set_own_fleet(&mut self, own: &OwnFleet) {
        let hex = |s: &str| u32::from_str_radix(s.trim(), 16).ok();
        self.own_icao = own.icao.iter().filter_map(|s| hex(s)).collect();
        self.own_mmsi = own.mmsi.iter().copied().collect();
        self.own_uas = own.uas_ids.iter().cloned().collect();
    }

    /// Own vehicles' current positions, so sensors' detections of them are dropped.
    pub fn set_own_vehicles(&mut self, v: &[OwnVehicle]) {
        self.own_vehicles = v
            .iter()
            .map(|o| {
                let (x, y) = self.local.to_xy(o.lat, o.lon);
                OwnLocal {
                    uid: o.uid.clone(),
                    t: o.t,
                    p: [x, y, o.alt_m],
                }
            })
            .collect();
    }

    /// QNH in hPa (from weather), for comparing pressure and geometric altitudes.
    pub fn set_qnh(&mut self, qnh_hpa: Option<f64>) {
        self.cfg.qnh_hpa = qnh_hpa;
    }

    pub fn tracks(&self) -> &[Track] {
        &self.out
    }

    /// The time of the picture: the last `update`, if any.
    pub fn picture_time(&self) -> Option<i64> {
        (self.now != i64::MIN).then_some(self.now)
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Cooperative tracks whose broadcasts disagree with the company's own sensors, as of the
    /// last `update`.
    pub fn disagreements(&self) -> &[Disagreement] {
        &self.disagreements
    }

    fn default_range(&self, k: SensorKind) -> f64 {
        self.cfg.sensor_range_m.get(&k).copied().unwrap_or(5_000.0)
    }

    fn default_scan(&self, k: SensorKind) -> i64 {
        self.cfg.scan_ms.get(&k).copied().unwrap_or(1_000)
    }

    fn gate(&self, m: usize) -> f64 {
        chi2_quantile(self.cfg.gate_probability, m)
    }

    /// The message's own time when it is plausible against its receipt time, else the receipt.
    fn msg_time(&self, msg_t: Option<i64>, receipt: i64) -> i64 {
        match msg_t {
            Some(m)
                if m <= receipt + self.cfg.max_clock_skew_ms
                    && m >= receipt - self.cfg.max_feed_delay_ms =>
            {
                m
            }
            _ => receipt,
        }
    }

    /// The same content heard by another receiver within the window is a duplicate. The same
    /// receiver never hears one transmission twice, so identical content from it is a new report
    /// (a steady velocity message, say).
    fn is_dup(&mut self, key: u64, t: i64, rx: &str) -> bool {
        let w = self.cfg.dedup_window_ms;
        let rxh = hash64(&rx);
        match self.dedup.get(&key) {
            Some(&(t0, r0)) if r0 != rxh && (t - t0).abs() <= w => true,
            _ => {
                self.dedup.insert(key, (t, rxh));
                false
            }
        }
    }

    /// Offset to add to an altitude in `from` to express it in `to`, and that offset's variance.
    fn alt_offset(&self, from: AltRef, to: AltRef) -> (f64, f64) {
        if from == to || from == AltRef::None || to == AltRef::None {
            return (0.0, 0.0);
        }
        let (m, s) = match self.cfg.qnh_hpa {
            Some(q) => (8.32 * (q - 1013.25), self.cfg.baro_geo_sigma_known_m),
            None => (0.0, self.cfg.baro_geo_sigma_unknown_m),
        };
        match (from, to) {
            (AltRef::Baro, AltRef::Geo) => (m, s * s),
            _ => (-m, s * s),
        }
    }

    fn is_anonymous_icao(&self, addr: u32, non_icao: bool) -> bool {
        non_icao
            || self
                .anon_ranges
                .iter()
                .any(|&(a, b)| addr >= a && addr <= b)
    }

    // ---------------------------------------------------------------- SBS

    /// One SBS-1 line. The receiver is the session-id field (mapped through `set_sensors`).
    pub fn ingest_sbs(&mut self, line: &str, receipt_ms: i64) -> Outcome {
        self.sbs_line(line, receipt_ms, None)
    }

    /// One SBS-1 line from a known receiver (a dump1090 writes a constant session id).
    pub fn ingest_sbs_from(&mut self, line: &str, receipt_ms: i64, rx: &str) -> Outcome {
        self.sbs_line(line, receipt_ms, Some(rx))
    }

    fn sbs_line(&mut self, line: &str, receipt: i64, rx_override: Option<&str>) -> Outcome {
        self.stats.sbs_lines += 1;
        let m = match sbs::parse(line, self.cfg.sbs_utc_offset_min) {
            Ok(m) => m,
            Err(e) => {
                self.stats.sbs_errors += 1;
                return Outcome::Error(e.to_string());
            }
        };
        let Some(addr) = m.icao else {
            self.stats.sbs_ignored += 1;
            return Outcome::Ignored("no address");
        };
        let id = Identity::Icao {
            addr,
            non_icao: m.non_icao,
        };
        if self.own_icao.contains(&addr) && !m.non_icao {
            self.stats.own_ignored += 1;
            return Outcome::Ignored("own fleet");
        }
        if m.kind == SbsKind::Id {
            if let (Some(cs), Some(&no)) = (m.callsign.clone(), self.by_id.get(&id))
                && let Some(r) = self.recs.get_mut(&no)
            {
                r.ids.callsign = Some(cs);
                return Outcome::Fused;
            }
            self.stats.sbs_ignored += 1;
            return Outcome::Ignored("ID line for an unknown aircraft");
        }
        if m.kind != SbsKind::Msg {
            self.stats.sbs_ignored += 1;
            return Outcome::Ignored("not a transmission");
        }
        let rx = match rx_override {
            Some(r) => r.to_string(),
            None => match &m.session_id {
                Some(s) => self
                    .sbs_sessions
                    .get(s)
                    .cloned()
                    .unwrap_or_else(|| s.clone()),
                None => "sbs".to_string(),
            },
        };
        let site_kind = self.sites.get(&rx).map(|s| s.kind);
        let kind = if site_kind == Some(SiteKind::AdsbUat) {
            SourceKind::AdsbUat
        } else {
            SourceKind::Adsb1090
        };
        let t = self.msg_time(m.generated_ms, receipt);
        let content = line.trim().splitn(11, ',').nth(10).unwrap_or("");
        let key = hash64(&("sbs", addr, m.non_icao, m.transmission, content));
        let tm = self.cfg.timing(kind);
        let mut rep = Report::new(id.clone(), kind, rx, t, Domain::Air, tm.nominal_ms);
        rep.anonymous = self.is_anonymous_icao(addr, m.non_icao);
        let tx = m.transmission.unwrap_or(0);
        let alt = m
            .altitude_ft
            .map(|a| (a * FT, AltRef::Baro, self.cfg.adsb_alt_sigma_m));
        let pos = match (m.lat, m.lon) {
            (Some(a), Some(o)) => Some((a, o, self.cfg.adsb_pos_sigma_m)),
            _ => None,
        };
        let vel = match (m.ground_speed_kt, m.track_deg) {
            (Some(gs), Some(tr)) => Some((
                gs * KT,
                tr,
                self.cfg.adsb_speed_sigma_mps,
                self.cfg.adsb_track_sigma_deg.to_radians(),
            )),
            _ => None,
        };
        match tx {
            1 => rep.callsign = m.callsign.clone(),
            2 => {
                rep.pos = pos;
                rep.vel = vel;
                rep.on_ground = Some(true);
            }
            3 => {
                rep.pos = pos;
                rep.alt = alt;
                rep.on_ground = m.on_ground;
            }
            4 => {
                rep.vel = vel;
                rep.vz = m
                    .vertical_rate_fpm
                    .map(|v| (v * FT / 60.0, self.cfg.adsb_vrate_sigma_mps));
            }
            5 | 7 => {
                rep.alt = alt;
                rep.on_ground = m.on_ground;
            }
            6 => {
                rep.alt = alt;
                rep.on_ground = m.on_ground;
                rep.statics.squawk = m.squawk.clone();
            }
            _ => {}
        }
        let squawk_emergency = matches!(
            m.squawk.as_deref(),
            Some("7500") | Some("7600") | Some("7700")
        );
        if m.emergency.is_some() || m.squawk.is_some() {
            rep.emergency = Some(m.emergency.unwrap_or(false) || squawk_emergency);
        }
        if let Some(c) = self.classes.get(&addr) {
            rep.class = Some(*c);
        }
        if self.is_dup(key, t, &rep.rx) {
            self.stats.duplicates += 1;
            self.note_duplicate(&rep);
            return Outcome::Duplicate;
        }
        self.apply(rep)
    }

    fn note_duplicate(&mut self, rep: &Report) {
        if let Some(&no) = self.by_id.get(&rep.id)
            && let Some(r) = self.recs.get_mut(&no)
        {
            let kin = rep.kinematic() && r.kf.is_some();
            r.upsert_source(rep.kind, &rep.rx, rep.t, kin, rep.interval_ms);
        }
    }

    // ---------------------------------------------------------------- AIS

    /// One NMEA line (with or without a tag block). The receiver is the tag block's `s:`, else
    /// "ais".
    pub fn ingest_nmea(&mut self, line: &str, receipt_ms: i64) -> Outcome {
        self.nmea_line(line, receipt_ms, "ais")
    }

    /// One NMEA line from a known receiver, for streams without tag blocks.
    pub fn ingest_nmea_from(&mut self, line: &str, receipt_ms: i64, rx: &str) -> Outcome {
        self.nmea_line(line, receipt_ms, rx)
    }

    fn nmea_line(&mut self, line: &str, receipt: i64, default_rx: &str) -> Outcome {
        self.stats.nmea_lines += 1;
        let (tag, sentence) = match nmea::split_line(line) {
            Ok(v) => v,
            Err(e) => return self.nmea_error(e),
        };
        let s = match nmea::parse_vdm(sentence, self.cfg.nmea_require_checksum) {
            Ok(s) => s,
            Err(e) => return self.nmea_error(e),
        };
        let asm = match self.assembler.push(tag.as_ref(), s, default_rx, receipt) {
            Ok(Some(a)) => a,
            Ok(None) => {
                self.stats.nmea_fragments_dropped = self.assembler.dropped;
                return Outcome::Pending;
            }
            Err(e) => {
                self.stats.nmea_fragments_dropped = self.assembler.dropped;
                return self.nmea_error(e);
            }
        };
        if asm.own {
            self.stats.ais_own_vessel += 1;
            return Outcome::Ignored("own vessel (VDO)");
        }
        let msg = match ais::decode(&asm.payload, asm.fill) {
            Ok(m) => m,
            Err(e) => {
                self.stats.ais_errors += 1;
                return Outcome::Error(e.to_string());
            }
        };
        self.stats.ais_messages += 1;
        let base_t = self.msg_time(asm.time_ms, receipt);
        let key = hash64(&("ais", &asm.payload, asm.fill));
        let Some(mmsi) = msg.mmsi() else {
            self.stats.ais_other_types += 1;
            return Outcome::Ignored("AIS message without an MMSI");
        };
        if self.own_mmsi.contains(&mmsi) {
            self.stats.own_ignored += 1;
            return Outcome::Ignored("own fleet");
        }
        let reps = self.ais_reports(&msg, asm.rx.clone(), base_t);
        if reps.is_empty() {
            self.stats.ais_other_types += 1;
            return Outcome::Ignored("AIS message type without traffic content");
        }
        if self.is_dup(key, base_t, &asm.rx) {
            self.stats.duplicates += 1;
            for r in &reps {
                self.note_duplicate(r);
            }
            return Outcome::Duplicate;
        }
        let mut out = Outcome::Ignored("nothing to fuse");
        for r in reps {
            out = self.apply(r);
        }
        out
    }

    fn nmea_error(&mut self, e: NmeaError) -> Outcome {
        match e {
            NmeaError::BadChecksum | NmeaError::BadTagChecksum | NmeaError::MissingChecksum => {
                self.stats.nmea_checksum_errors += 1
            }
            NmeaError::Fragment => {}
            _ => self.stats.nmea_errors += 1,
        }
        self.stats.nmea_fragments_dropped = self.assembler.dropped;
        Outcome::Error(e.to_string())
    }

    /// The report time: the tag block's (or receipt) time, moved back to the report's own UTC
    /// second when that is clearly earlier (a delayed feed).
    fn ais_time(base: i64, second: u8) -> i64 {
        if second >= 60 {
            return base;
        }
        let base_s = base.div_euclid(1000);
        let cur = base_s.rem_euclid(60);
        let delta = (cur - second as i64).rem_euclid(60);
        if (2..=30).contains(&delta) {
            base - delta * 1000
        } else {
            base
        }
    }

    fn ais_reports(&self, msg: &AisMessage, rx: String, base_t: i64) -> Vec<Report> {
        let mut out = Vec::new();
        let pos_report = |p: &ais::Position, rx: String| -> Report {
            let class_a = p.msg_type <= 3;
            let kind = if class_a {
                SourceKind::AisA
            } else {
                SourceKind::AisB
            };
            let turning = p.rot_raw.is_some_and(|r| r.unsigned_abs() >= 10);
            let interval = if class_a {
                ais_a_interval_ms(p.sog_kn, p.nav_status, turning)
            } else {
                ais_b_interval_ms(p.sog_kn, p.class_b_cs.unwrap_or(true))
            };
            let t = Self::ais_time(base_t, p.second);
            let mut r = Report::new(
                Identity::Mmsi(p.mmsi),
                kind,
                rx,
                t,
                Domain::Surface,
                interval,
            );
            r.invalid_mmsi = !ais::mmsi_valid(p.mmsi);
            if let (Some(lat), Some(lon)) = (p.lat, p.lon) {
                let s = if p.accuracy {
                    self.cfg.ais_pos_sigma_m[0]
                } else {
                    self.cfg.ais_pos_sigma_m[1]
                };
                r.pos = Some((lat, lon, s));
            }
            if let Some(sog) = p.sog_kn.filter(|s| *s < 102.2) {
                let speed = sog * KT;
                let (course, sc) = match p.cog_deg {
                    Some(c) => (c, self.cfg.ais_course_sigma_deg.to_radians()),
                    // Without a course only a near-zero speed is usable.
                    None if sog < 0.5 => (0.0, std::f64::consts::PI),
                    None => (0.0, f64::NAN),
                };
                if sc.is_finite() {
                    r.vel = Some((
                        speed,
                        course,
                        self.cfg.ais_speed_sigma_mps + 0.02 * speed,
                        sc,
                    ));
                }
            }
            r.nav_status = p.nav_status;
            if let Some(n) = p.nav_status {
                r.statics.nav_status = Some(n);
                r.statics.nav_status_text = Some(ais::nav_status_text(n).to_string());
            }
            r.heading = p.heading_deg.map(|h| h as f64);
            r
        };
        match msg {
            AisMessage::Position(p) => out.push(pos_report(p, rx)),
            AisMessage::ClassBExtended(e) => {
                let mut r = pos_report(&e.position, rx);
                if !e.name.is_empty() {
                    r.statics.name = Some(e.name.clone());
                }
                if e.ship_type != 0 {
                    r.statics.ship_type = Some(e.ship_type);
                    r.statics.ship_type_text = Some(ais::ship_type_text(e.ship_type).to_string());
                    r.class = Some(Class::from_ship_type(e.ship_type));
                }
                dims(
                    e.to_bow,
                    e.to_stern,
                    e.to_port,
                    e.to_starboard,
                    &mut r.statics,
                );
                out.push(r);
            }
            AisMessage::StaticVoyage(s) => {
                let mut r = Report::new(
                    Identity::Mmsi(s.mmsi),
                    SourceKind::AisA,
                    rx,
                    base_t,
                    Domain::Surface,
                    360_000,
                );
                r.invalid_mmsi = !ais::mmsi_valid(s.mmsi);
                let st = &mut r.statics;
                if !s.name.is_empty() {
                    st.name = Some(s.name.clone());
                }
                if !s.callsign.is_empty() {
                    st.callsign = Some(s.callsign.clone());
                }
                if s.imo != 0 {
                    st.imo = Some(s.imo);
                }
                if s.ship_type != 0 {
                    st.ship_type = Some(s.ship_type);
                    st.ship_type_text = Some(ais::ship_type_text(s.ship_type).to_string());
                    r.class = Some(Class::from_ship_type(s.ship_type));
                }
                dims(s.to_bow, s.to_stern, s.to_port, s.to_starboard, st);
                if s.draught_m > 0.0 {
                    st.draught_m = Some(s.draught_m);
                }
                if !s.destination.is_empty() {
                    st.destination = Some(s.destination.clone());
                }
                st.eta = eta_text(s.eta);
                out.push(r);
            }
            AisMessage::StaticA { mmsi, name, .. } => {
                let mut r = Report::new(
                    Identity::Mmsi(*mmsi),
                    SourceKind::AisB,
                    rx,
                    base_t,
                    Domain::Surface,
                    360_000,
                );
                r.invalid_mmsi = !ais::mmsi_valid(*mmsi);
                if !name.is_empty() {
                    r.statics.name = Some(name.clone());
                }
                out.push(r);
            }
            AisMessage::StaticB(b) => {
                let mut r = Report::new(
                    Identity::Mmsi(b.mmsi),
                    SourceKind::AisB,
                    rx,
                    base_t,
                    Domain::Surface,
                    360_000,
                );
                r.invalid_mmsi = !ais::mmsi_valid(b.mmsi);
                if b.ship_type != 0 {
                    r.statics.ship_type = Some(b.ship_type);
                    r.statics.ship_type_text = Some(ais::ship_type_text(b.ship_type).to_string());
                    r.class = Some(Class::from_ship_type(b.ship_type));
                }
                if !b.callsign.is_empty() {
                    r.statics.callsign = Some(b.callsign.clone());
                }
                dims(
                    b.to_bow,
                    b.to_stern,
                    b.to_port,
                    b.to_starboard,
                    &mut r.statics,
                );
                r.statics.mothership_mmsi = b.mothership_mmsi;
                out.push(r);
            }
            AisMessage::Other { .. } => {}
        }
        out
    }

    // ---------------------------------------------------------------- JSON

    /// One JSON line of section 5 (`rid` or `det`). Times come from the line.
    pub fn ingest_json(&mut self, line: &str) -> Outcome {
        self.stats.json_lines += 1;
        match wire::parse(line) {
            Ok(JsonLine::Rid(r)) => self.ingest_rid(&r),
            Ok(JsonLine::Det(d)) => self.ingest_det(&d),
            Err(e) => {
                self.stats.json_errors += 1;
                Outcome::Error(e.to_string())
            }
        }
    }

    pub fn ingest_rid(&mut self, r: &Rid) -> Outcome {
        self.stats.rid_reports += 1;
        if self.own_uas.contains(&r.uas_id) {
            self.stats.own_ignored += 1;
            return Outcome::Ignored("own fleet");
        }
        let t = self.msg_time(r.t, r.t_rx);
        let id = Identity::Uas(r.uas_id.clone());
        let tm = self.cfg.timing(SourceKind::RemoteId);
        let mut rep = Report::new(
            id,
            SourceKind::RemoteId,
            r.rx.clone(),
            t,
            Domain::Air,
            tm.nominal_ms,
        );
        rep.anonymous = r.id_type.as_deref() == Some("session");
        rep.class = Some(Class::Drone);
        if let (Some(lat), Some(lon)) = (r.lat, r.lon) {
            let s = r
                .h_acc_m
                .map_or(self.cfg.rid_pos_sigma_m, |a| (a / 2.0).max(1.0));
            rep.pos = Some((lat, lon, s));
        }
        let sv = r
            .v_acc_m
            .map_or(self.cfg.rid_alt_sigma_m, |a| (a / 2.0).max(1.0));
        rep.alt = match (r.alt_geo_m, r.alt_baro_m) {
            (Some(g), _) => Some((g - self.cfg.geoid_n_m, AltRef::Geo, sv)),
            (None, Some(b)) => Some((b, AltRef::Baro, sv)),
            _ => None,
        };
        if let (Some(sp), Some(tr)) = (r.speed_mps, r.track_deg) {
            rep.vel = Some((sp, tr, self.cfg.rid_speed_sigma_mps, 3f64.to_radians()));
        }
        rep.vz = r.vspeed_mps.map(|v| (v, 0.5));
        rep.on_ground = r.status.as_deref().map(|s| s == "ground");
        rep.emergency = r.status.as_deref().map(|s| s == "emergency");
        rep.statics.id_type = r.id_type.clone();
        rep.statics.ua_type = r.ua_type.clone();
        rep.statics.op_lat = r.op_lat;
        rep.statics.op_lon = r.op_lon;
        let key = hash64(&(
            "rid",
            &r.uas_id,
            r.t,
            r.lat.map(f64::to_bits),
            r.lon.map(f64::to_bits),
        ));
        if self.is_dup(key, t, &r.rx) {
            self.stats.duplicates += 1;
            self.note_duplicate(&rep);
            return Outcome::Duplicate;
        }
        self.apply(rep)
    }

    // ---------------------------------------------------------------- cooperative fusion

    fn apply(&mut self, rep: Report) -> Outcome {
        let no = match self.by_id.get(&rep.id) {
            Some(&no) => no,
            None => {
                if !rep.kinematic() && rep.callsign.is_none() && rep.statics == Static::default() {
                    self.stats.no_position_reports += 1;
                    return Outcome::Ignored("no content for an unknown identity");
                }
                let no = self.next_no;
                self.next_no += 1;
                let mut r = Rec::new(no, rep.domain, true, rep.kind, rep.t);
                r.identity = Some(rep.id.clone());
                match &rep.id {
                    Identity::Icao { addr, .. } => {
                        r.ids.icao24 = Some(sbs::icao_hex(*addr));
                        r.ladd = self.ladd.contains(addr);
                    }
                    Identity::Mmsi(m) => r.ids.mmsi = Some(*m),
                    Identity::Uas(u) => r.ids.uas_id = Some(u.clone()),
                }
                self.recs.insert(no, r);
                self.by_id.insert(rep.id.clone(), no);
                self.stats.tracks_created += 1;
                no
            }
        };
        let cfg = &self.cfg;
        let local = &self.local;
        let alt_off = |from: AltRef, to: AltRef| -> (f64, f64) {
            if from == to || from == AltRef::None || to == AltRef::None {
                return (0.0, 0.0);
            }
            let (m, s) = match cfg.qnh_hpa {
                Some(q) => (8.32 * (q - 1013.25), cfg.baro_geo_sigma_known_m),
                None => (0.0, cfg.baro_geo_sigma_unknown_m),
            };
            if from == AltRef::Baro {
                (m, s * s)
            } else {
                (-m, s * s)
            }
        };
        let Some(r) = self.recs.get_mut(&no) else {
            return Outcome::Ignored("lost");
        };
        let stats = &mut self.stats;
        let t = rep.t;

        // Identity and static content.
        r.last_report_t = r.last_report_t.max(t);
        if rep.anonymous {
            r.anonymous = true;
        }
        if rep.invalid_mmsi {
            r.invalid_mmsi = true;
        }
        if let Some(cs) = &rep.callsign {
            r.ids.callsign = Some(cs.clone());
            if !r.class_from_source && cfg.classify_airline_callsigns {
                r.class = if airline_callsign(cs) {
                    Class::Airliner
                } else {
                    Class::UnknownAir
                };
            }
        }
        if let Some(c) = rep.class
            && !c.is_unknown()
        {
            r.class = c;
            r.class_from_source = true;
        }
        merge_static(&mut r.statics, &rep.statics);
        if let Some(n) = rep.nav_status {
            r.nav_status = Some(n);
        }
        if let Some(g) = rep.on_ground {
            r.on_ground = g;
        }
        if let Some(e) = rep.emergency {
            r.emergency = e;
        }
        if let Some(h) = rep.heading {
            r.heading = Some((h, t));
        }
        let kin = rep.kinematic();
        if rep.pos.is_some() {
            r.primary = rep.kind;
            r.interval_ms = rep.interval_ms;
        }
        r.upsert_source(
            rep.kind,
            &rep.rx,
            t,
            kin && (r.kf.is_some() || rep.pos.is_some()),
            rep.interval_ms,
        );
        if !kin {
            return Outcome::Fused;
        }

        let surface = rep.domain == Domain::Surface;
        let (qh, qv) = if surface {
            (cfg.q_surface, 0.0)
        } else {
            (cfg.q_air, cfg.q_air_vertical)
        };

        // Velocity as a grid vector and its covariance.
        let vel_grid = |lat: f64, lon: f64, v: (f64, f64, f64, f64)| {
            let gamma = local.convergence(lat, lon);
            let cg = v.1.to_radians() + gamma;
            let (vx, vy) = (v.0 * cg.sin(), v.0 * cg.cos());
            (vx, vy, speed_course_cov(v.0, cg, v.2, v.3))
        };

        if r.kf.is_none() {
            let Some((lat, lon, ps)) = rep.pos else {
                // Kinematics before any position: keep for the first fix.
                if let Some(v) = rep.vel {
                    r.pending_vel = Some((t, v.0, v.1, v.2, v.3));
                }
                if let Some((a, ar, s)) = rep.alt {
                    r.pending_alt = Some((t, a, ar, s));
                }
                return Outcome::Fused;
            };
            let (x, y) = local.to_xy(lat, lon);
            let mut xv = [x, y, 0.0, 0.0, 0.0, 0.0];
            let mut p: M6 = [[0.0; 6]; 6];
            p[0][0] = ps * ps;
            p[1][1] = ps * ps;
            let vsig = if surface { 10.0 } else { 120.0 };
            p[3][3] = vsig * vsig;
            p[4][4] = vsig * vsig;
            let vel = rep.vel.or(r
                .pending_vel
                .filter(|pv| (t - pv.0).abs() <= 10_000)
                .map(|pv| (pv.1, pv.2, pv.3, pv.4)));
            if let Some(v) = vel {
                let (vx, vy, c) = vel_grid(lat, lon, v);
                xv[3] = vx;
                xv[4] = vy;
                p[3][3] = c[0][0];
                p[3][4] = c[0][1];
                p[4][3] = c[1][0];
                p[4][4] = c[1][1];
            }
            if surface {
                p[2][2] = 0.01;
                p[5][5] = 1e-4;
                r.alt_ref = AltRef::None;
                r.alt_known = true;
            } else {
                let alt = rep.alt.or(r
                    .pending_alt
                    .filter(|pa| (t - pa.0).abs() <= 10_000)
                    .map(|pa| (pa.1, pa.2, pa.3)));
                match alt {
                    Some((a, ar, s)) => {
                        xv[2] = a;
                        p[2][2] = s * s;
                        r.alt_ref = ar;
                        r.alt_known = true;
                    }
                    None => {
                        p[2][2] = 1000.0 * 1000.0;
                        r.alt_ref = match rep.kind {
                            SourceKind::Adsb1090 | SourceKind::AdsbUat => AltRef::Baro,
                            _ => AltRef::Geo,
                        };
                    }
                }
                p[5][5] = 25.0;
                if let Some((vz, s)) = rep.vz {
                    xv[5] = vz;
                    p[5][5] = s * s;
                }
            }
            r.kf = Some(Kf::new(t, xv, p, qh, qv, surface));
            r.pos_reports = 1;
            r.first_id_t = r.first_id_t.min(t);
            r.upsert_source(rep.kind, &rep.rx, t, true, rep.interval_ms);
            let need = match rep.kind {
                SourceKind::AisA | SourceKind::AisB => cfg.coop_confirm_ais,
                SourceKind::RemoteId => cfg.coop_confirm_rid,
                _ => cfg.coop_confirm_adsb,
            };
            if r.pos_reports >= need {
                r.confirmed = true;
            }
            return Outcome::Fused;
        }

        let Some(kf) = r.kf.as_mut() else {
            return Outcome::Fused;
        };
        if t < kf.t_ms - cfg.oosm_max_ms {
            stats.late_reports += 1;
            return Outcome::Fused;
        }
        let mut fused = false;
        if let Some((lat, lon, ps)) = rep.pos {
            let (x, y) = local.to_xy(lat, lon);
            let rr = noise2([[ps * ps, 0.0], [0.0, ps * ps]]);
            let z = [x, y];
            if let Some(inn) = kf.innov(t, |xs| lin_components(&[0, 1], &z, &rr, 0.0, xs)) {
                let (xp, _) = kf.at(t);
                let dist = ((xp[0] - x).powi(2) + (xp[1] - y).powi(2)).sqrt();
                if inn.d2 > cfg.jump_chi2 && dist > cfg.jump_min_m {
                    stats.jumps_rejected += 1;
                    r.jump_until = t + cfg.jump_flag_ms;
                    r.rejects.push((t, x, y));
                    if r.rejects.len() > 3 {
                        r.rejects.remove(0);
                    }
                    let n = r.rejects.len();
                    if n >= 2 {
                        let (t1, x1, y1) = r.rejects[n - 2];
                        let (t2, x2, y2) = r.rejects[n - 1];
                        let dt = (t2 - t1) as f64 / 1000.0;
                        let vmax = if surface { 40.0 } else { 350.0 };
                        let d = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
                        if dt > 0.0 && dt <= 60.0 && d <= vmax * dt + 6.0 * ps {
                            // Consistent reports at the new place: the track moves there.
                            let (vx, vy) = ((x2 - x1) / dt, (y2 - y1) / dt);
                            kf.x[0] = x2;
                            kf.x[1] = y2;
                            kf.x[3] = vx;
                            kf.x[4] = vy;
                            kf.t_ms = t2;
                            let vv = (2.0 * ps / dt).powi(2) + 4.0;
                            for i in 0..6 {
                                for j in 0..6 {
                                    if i != j
                                        && (i < 2 || j < 2 || i == 3 || i == 4 || j == 3 || j == 4)
                                    {
                                        kf.p[i][j] = 0.0;
                                    }
                                }
                            }
                            kf.p[0][0] = ps * ps;
                            kf.p[1][1] = ps * ps;
                            kf.p[3][3] = vv;
                            kf.p[4][4] = vv;
                            r.rejects.clear();
                            fused = true;
                        }
                    }
                } else {
                    r.rejects.clear();
                    kf.apply(&inn);
                    fused = true;
                    r.pos_reports += 1;
                }
            }
        }
        if let Some((a, ar, s)) = rep.alt
            && !surface
        {
            if !r.alt_known {
                kf.predict_to(t);
                kf.x[2] = a;
                for i in 0..6 {
                    kf.p[2][i] = 0.0;
                    kf.p[i][2] = 0.0;
                }
                kf.p[2][2] = s * s;
                r.alt_ref = ar;
                r.alt_known = true;
                fused = true;
            } else {
                // The measurement predicted from the track: track altitude plus (ref -> meas).
                let (off, var) = alt_off(r.alt_ref, ar);
                let rr = noise1(s * s + var);
                let z = [a];
                if let Some(inn) = kf.innov(t, |xs| lin_components(&[2], &z, &rr, off, xs))
                    && inn.d2 < 100.0
                {
                    kf.apply(&inn);
                    fused = true;
                }
            }
        }
        if let Some(v) = rep.vel {
            let (xp, _) = kf.at(t);
            let (lat, lon) = local.to_latlon(xp[0], xp[1]);
            let (vx, vy, c) = vel_grid(lat, lon, v);
            let rr = noise2(c);
            let z = [vx, vy];
            if let Some(inn) = kf.innov(t, |xs| lin_components(&[3, 4], &z, &rr, 0.0, xs))
                && inn.d2 < 200.0
            {
                kf.apply(&inn);
                fused = true;
            }
        }
        if let Some((vz, s)) = rep.vz
            && !surface
        {
            let rr = noise1(s * s);
            let z = [vz];
            if let Some(inn) = kf.innov(t, |xs| lin_components(&[5], &z, &rr, 0.0, xs))
                && inn.d2 < 200.0
            {
                kf.apply(&inn);
                fused = true;
            }
        }
        let need = match rep.kind {
            SourceKind::AisA | SourceKind::AisB => cfg.coop_confirm_ais,
            SourceKind::RemoteId => cfg.coop_confirm_rid,
            _ => cfg.coop_confirm_adsb,
        };
        if r.pos_reports >= need {
            r.confirmed = true;
        }
        if fused {
            r.upsert_source(rep.kind, &rep.rx, t, true, rep.interval_ms);
        }
        Outcome::Fused
    }

    // ---------------------------------------------------------------- detections

    pub fn ingest_det(&mut self, d: &Det) -> Outcome {
        self.stats.det_reports += 1;
        let key: SensorKey = (d.sensor, d.platform.clone());
        let site = self.sites.get(&d.platform);
        let (lat, lon, alt) = match (d.sensor_lat, d.sensor_lon) {
            (Some(a), Some(o)) => (
                a,
                o,
                d.sensor_alt_m.or(site.and_then(|s| s.alt_m)).unwrap_or(0.0),
            ),
            _ => match site.and_then(|s| Some((s.lat?, s.lon?, s.alt_m.unwrap_or(0.0)))) {
                Some(v) => v,
                None => {
                    self.stats.det_errors += 1;
                    return Outcome::Error("detection without a sensor position".into());
                }
            },
        };
        let Some(bearing) = d.bearing_deg else {
            self.stats.det_errors += 1;
            return Outcome::Error("detection without a bearing".into());
        };
        let (x, y) = self.local.to_xy(lat, lon);
        let gamma = self.local.convergence(lat, lon);
        let s = [x, y, alt];
        let size_idx = match d.size.as_deref() {
            Some("small") => 0,
            Some("medium") => 1,
            Some("large") => 2,
            _ => 3,
        };
        let range_m = site
            .and_then(|s| s.range_m)
            .unwrap_or_else(|| self.default_range(d.sensor));
        let r_max = match d.sensor {
            SensorKind::Daa => self.cfg.daa_range_by_size_m[size_idx],
            _ => range_m,
        };
        let scan_ms = site
            .and_then(|s| s.scan_ms)
            .unwrap_or_else(|| self.default_scan(d.sensor));
        let domain = match d.sensor {
            SensorKind::Daa => Domain::Air,
            SensorKind::MarineRadar => Domain::Surface,
            SensorKind::GroundRadar => site.and_then(|s| s.domain).unwrap_or(Domain::Air),
        };
        // The platform's own motion (for range rate), from its successive positions.
        let pl = self.platforms.entry(key.clone()).or_insert(Platform {
            t: d.t,
            s,
            vel: None,
            last_det_t: d.t,
            range_m,
            scan_ms,
        });
        let since = d.t.saturating_sub(pl.t);
        if (200..=10_000).contains(&since) {
            let dt = since as f64 / 1000.0;
            pl.vel = Some([
                (s[0] - pl.s[0]) / dt,
                (s[1] - pl.s[1]) / dt,
                (s[2] - pl.s[2]) / dt,
            ]);
        }
        if d.t != pl.t {
            pl.t = d.t;
            pl.s = s;
        }
        pl.last_det_t = pl.last_det_t.max(d.t);
        let sv = pl.vel;
        let sb = d
            .sigma_bearing_deg
            .unwrap_or_else(|| {
                self.cfg
                    .bearing_sigma_deg
                    .get(&d.sensor)
                    .copied()
                    .unwrap_or(2.0)
            })
            .max(0.05)
            .to_radians();
        let polar = Polar {
            s,
            sv,
            gamma,
            bearing: (bearing.to_radians(), sb),
            elev: d.elev_deg.map(|e| {
                (
                    e.to_radians(),
                    d.sigma_elev_deg
                        .unwrap_or(self.cfg.elev_sigma_deg)
                        .max(0.05)
                        .to_radians(),
                )
            }),
            range: d.range_m.map(|r| {
                (
                    r,
                    d.sigma_range_m
                        .unwrap_or_else(|| {
                            self.cfg
                                .range_sigma_m
                                .get(&d.sensor)
                                .copied()
                                .unwrap_or(25.0)
                        })
                        .max(0.5),
                )
            }),
            rspeed: d
                .radial_speed_mps
                .filter(|_| sv.is_some() || d.sensor == SensorKind::GroundRadar)
                .map(|v| {
                    (
                        v,
                        d.sigma_radial_speed_mps
                            .unwrap_or(self.cfg.radial_speed_sigma_mps)
                            .max(0.05),
                    )
                }),
        };
        let det = DetIn {
            kind: d.sensor,
            platform: d.platform.clone(),
            t: d.t,
            polar,
            r_max,
            domain,
            scan_ms,
            small: size_idx == 0,
        };
        // A scan is the detections of one sensor at (nearly) one time.
        let flush = self
            .pending
            .get(&key)
            .and_then(|g| g.first())
            .is_some_and(|f| (d.t - f.t).abs() > 200);
        if flush && let Some(g) = self.pending.remove(&key) {
            self.process_scan(g);
        }
        self.pending.entry(key).or_default().push(det);
        Outcome::Pending
    }

    fn flush_dets(&mut self) {
        let groups = std::mem::take(&mut self.pending);
        for (_, g) in groups {
            self.process_scan(g);
        }
    }

    fn own_vehicle_hit(&self, d: &DetIn) -> bool {
        let g = self.gate(d.polar.dim().min(3));
        self.own_vehicles.iter().any(|o| {
            if o.uid == d.platform || (d.t - o.t).abs() > self.cfg.own_vehicle_max_age_ms {
                return false;
            }
            let dx = o.p[0] - d.polar.s[0];
            let dy = o.p[1] - d.polar.s[1];
            let dz = o.p[2] - d.polar.s[2];
            let rho = (dx * dx + dy * dy).sqrt().max(1.0);
            let r = (rho * rho + dz * dz).sqrt();
            let so = self.cfg.own_vehicle_sigma_m;
            let mut d2 = 0.0;
            let b = crate::geo::wrap_pi(d.polar.bearing.0 - (dx.atan2(dy) - d.polar.gamma));
            d2 += b * b / (d.polar.bearing.1.powi(2) + (so / rho).powi(2));
            if let Some((e, se)) = d.polar.elev {
                let ee = e - dz.atan2(rho);
                d2 += ee * ee / (se * se + (so / r).powi(2));
            }
            if let Some((rg, sr)) = d.polar.range {
                d2 += (rg - r).powi(2) / (sr * sr + so * so);
            }
            d2 <= g
        })
    }

    fn lob_obs(d: &DetIn) -> LobObs {
        LobObs {
            t: d.t,
            s: d.polar.s,
            platform: d.platform.clone(),
            kind: d.kind,
            beta: d.polar.bearing.0 + d.polar.gamma,
            beta_true: d.polar.bearing.0,
            sb: d.polar.bearing.1,
            elev: d.polar.elev,
            r_max: d.r_max,
        }
    }

    fn candidates(&self, d: &DetIn, di: usize, out: &mut Vec<Cand>) {
        for (&no, r) in &self.recs {
            if r.domain != d.domain {
                continue;
            }
            if let Some(kf) = &r.kf {
                let dtp = (d.t - kf.t_ms) as f64 / 1000.0;
                let px = kf.x[0] + kf.x[3] * dtp - d.polar.s[0];
                let py = kf.x[1] + kf.x[4] * dtp - d.polar.s[1];
                let pz = if kf.surface {
                    0.0
                } else {
                    kf.x[2] - d.polar.s[2]
                };
                let dist = (px * px + py * py + pz * pz).sqrt();
                let sig = (kf.p[0][0] + kf.p[1][1] + kf.p[2][2]).sqrt()
                    + dtp.abs() * (kf.p[3][3] + kf.p[4][4]).sqrt()
                    + self.cfg.baro_geo_sigma_unknown_m;
                // A cheap reach test before the full innovation: slant distance against the
                // detection's range, or against the far end of a bearing-only line.
                let far = match d.polar.range {
                    Some((rg, sr)) => (dist - rg).abs() > 5.0 * sig + 6.0 * sr + 300.0 + rg * 0.05,
                    None => dist > d.r_max * 1.5 + 5.0 * sig + 300.0,
                };
                if far {
                    continue;
                }
                let (z_off, z_var) = if r.alt_known {
                    self.alt_offset(r.alt_ref, AltRef::Geo)
                } else {
                    (0.0, 0.0)
                };
                let surface = kf.surface;
                let Some(inn) = kf.innov(d.t, |x| d.polar.lin(x, z_off, z_var, surface)) else {
                    continue;
                };
                if inn.d2 <= self.gate(inn.m) {
                    // A confirmed track keeps priority over a tentative one that also gates.
                    let cost = inn.cost()
                        + if r.confirmed {
                            0.0
                        } else {
                            self.cfg.tentative_penalty
                        };
                    out.push(Cand {
                        cost,
                        det: di,
                        rec: no,
                        kind: CandKind::Track,
                        innov: Some(inn),
                    });
                }
            } else if let Some(lob) = &r.lob {
                let Some(last) = lob.latest() else { continue };
                if d.t - last.t > self.cfg.bearing_drop_ms {
                    continue;
                }
                if d.polar.range.is_some() {
                    // A detection with a range on a bearing-only track's line fixes it.
                    let z_guess = last.elev.map_or(d.polar.s[2] + 150.0, |(e, _)| {
                        last.s[2] + 0.5 * last.r_max * e.tan()
                    });
                    let Some((p, _)) = d.polar.fix(z_guess) else {
                        continue;
                    };
                    let (dx, dy) = (p[0] - last.s[0], p[1] - last.s[1]);
                    let rho = (dx * dx + dy * dy).sqrt();
                    if rho > 1.3 * last.r_max || rho < R_MIN {
                        continue;
                    }
                    let (b, bv) = lob
                        .predict_same(&last.platform, last.kind, d.t)
                        .map(|(b, _)| b)
                        .unwrap_or((last.beta, last.sb * last.sb));
                    let nu = crate::geo::wrap_pi(dx.atan2(dy) - b);
                    let var = bv
                        + (d.polar.bearing.1 * d.polar.range.map_or(0.0, |r| r.0) / rho.max(1.0))
                            .powi(2);
                    let d2 = nu * nu / var;
                    if d2 <= self.gate(1) {
                        out.push(Cand {
                            cost: 1e6 + d2,
                            det: di,
                            rec: no,
                            kind: CandKind::LobFix,
                            innov: None,
                        });
                    }
                } else if let Some(((b, bv), e)) = lob.predict_same(&d.platform, d.kind, d.t) {
                    let bg = d.polar.bearing.0 + d.polar.gamma;
                    let nu = crate::geo::wrap_pi(bg - b);
                    let var = bv + d.polar.bearing.1.powi(2);
                    let mut d2 = nu * nu / var;
                    let mut ln = var.ln();
                    let mut m = 1;
                    if let (Some((ep, ev)), Some((ed, sd))) = (e, d.polar.elev) {
                        let v2 = ev + sd * sd;
                        d2 += (ed - ep).powi(2) / v2;
                        ln += v2.ln();
                        m = 2;
                    }
                    if d2 <= self.gate(m) {
                        let cost = d2
                            + ln
                            + if r.confirmed {
                                0.0
                            } else {
                                self.cfg.tentative_penalty
                            };
                        out.push(Cand {
                            cost,
                            det: di,
                            rec: no,
                            kind: CandKind::LobSame,
                            innov: None,
                        });
                    }
                } else if let Some(c) = lob.cross_cost(&Self::lob_obs(d), &self.cfg) {
                    out.push(Cand {
                        cost: c,
                        det: di,
                        rec: no,
                        kind: CandKind::LobCross,
                        innov: None,
                    });
                }
            }
        }
    }

    fn process_scan(&mut self, dets: Vec<DetIn>) {
        let mut cands = Vec::new();
        let mut keep = vec![true; dets.len()];
        for (i, d) in dets.iter().enumerate() {
            if self.own_vehicle_hit(d) {
                self.stats.own_detections += 1;
                keep[i] = false;
                continue;
            }
            self.candidates(d, i, &mut cands);
        }
        cands.sort_by(|a, b| {
            a.cost
                .total_cmp(&b.cost)
                .then(a.det.cmp(&b.det))
                .then(a.rec.cmp(&b.rec))
        });
        let mut det_used = vec![false; dets.len()];
        let mut rec_used: HashSet<u64> = HashSet::new();
        let mut chosen = Vec::new();
        for c in cands {
            if det_used[c.det] || rec_used.contains(&c.rec) {
                continue;
            }
            det_used[c.det] = true;
            rec_used.insert(c.rec);
            chosen.push(c);
        }
        for c in chosen {
            self.apply_det(&dets[c.det], c);
        }
        for (i, d) in dets.iter().enumerate() {
            if keep[i] && !det_used[i] {
                self.start_from_det(d);
            }
        }
    }

    fn kf_from_fix(&self, d: &DetIn) -> Option<Kf> {
        let surface = d.domain == Domain::Surface;
        let z_guess = if surface { 0.0 } else { d.polar.s[2] + 150.0 };
        let (p, c) = d.polar.fix(z_guess)?;
        let mut pm: M6 = [[0.0; 6]; 6];
        pm[0][0] = c[0][0];
        pm[0][1] = c[0][1];
        pm[1][0] = c[1][0];
        pm[1][1] = c[1][1];
        let vs = if surface { 10.0 } else { 40.0 };
        pm[3][3] = vs * vs;
        pm[4][4] = vs * vs;
        let mut x = [p[0], p[1], p[2], 0.0, 0.0, 0.0];
        if surface {
            x[2] = 0.0;
            pm[2][2] = 0.01;
            pm[5][5] = 1e-4;
        } else {
            pm[2][2] = match (d.polar.elev, d.polar.range) {
                (Some((e, se)), Some((r, sr))) => (r * se).powi(2) + (sr * e.sin()).powi(2) + 4.0,
                _ => 400.0 * 400.0,
            };
            pm[5][5] = 25.0;
        }
        let (qh, qv) = if surface {
            (self.cfg.q_noncoop_surface, 0.0)
        } else {
            (self.cfg.q_noncoop_air, self.cfg.q_air_vertical)
        };
        Some(Kf::new(d.t, x, pm, qh, qv, surface))
    }

    fn start_from_det(&mut self, d: &DetIn) {
        let no = self.next_no;
        let src = source_of(d.kind);
        let mut r = Rec::new(no, d.domain, false, src, d.t);
        r.scan_ms = d.scan_ms;
        r.interval_ms = d.scan_ms;
        r.init_sensor = Some((d.kind, d.platform.clone()));
        r.alt_ref = if d.domain == Domain::Surface {
            AltRef::None
        } else {
            AltRef::Geo
        };
        if d.domain == Domain::Surface && d.small {
            r.class = Class::SmallCraft;
        }
        if d.polar.range.is_some() {
            let Some(kf) = self.kf_from_fix(d) else {
                return;
            };
            r.alt_known = d.domain == Domain::Surface || d.polar.elev.is_some();
            r.kf = Some(kf);
            self.stats.det_new_tracks += 1;
        } else {
            let mut lob = Lob::default();
            lob.push(Self::lob_obs(d), self.cfg.tri_window_ms);
            r.lob = Some(lob);
            self.stats.det_bearing_only_new += 1;
        }
        r.upsert_source(src, &d.platform, d.t, true, d.scan_ms);
        r.add_hit(d.t);
        self.next_no += 1;
        self.stats.tracks_created += 1;
        self.recs.insert(no, r);
    }

    fn apply_det(&mut self, d: &DetIn, c: Cand) {
        let key: SensorKey = (d.kind, d.platform.clone());
        let src = source_of(d.kind);
        let fix_kf = if c.kind == CandKind::LobFix {
            self.kf_from_fix(d)
        } else {
            None
        };
        let cfg = &self.cfg;
        let Some(r) = self.recs.get_mut(&c.rec) else {
            return;
        };
        match c.kind {
            CandKind::Track => {
                let Some(inn) = c.innov else { return };
                if r.cooperative && r.coop_fresh(cfg, d.t) {
                    // A fresh broadcast is not moved by the company's own sensors (they check it).
                    r.corroborate(&key, d.t);
                    r.upsert_source(src, &d.platform, d.t, false, d.scan_ms);
                    self.stats.det_corroborated += 1;
                    return;
                }
                // A triangulated track keeps its lines: a new bearing-only line refits them all
                // (a batch fit holds up far better than a filter fed bearings alone).
                let refit = match (r.lob.as_mut(), d.polar.range) {
                    (Some(lob), None) => {
                        lob.push(Self::lob_obs(d), cfg.tri_window_ms);
                        lob.fit(cfg)
                    }
                    _ => None,
                };
                match refit {
                    Some(f) => {
                        r.kf = Some(kf_from_fit(&f, d.polar.s[2], cfg));
                        r.alt_known = f.z.is_some();
                    }
                    None => {
                        if let Some(kf) = r.kf.as_mut() {
                            kf.apply(&inn);
                        }
                    }
                }
                if d.polar.range.is_some() {
                    r.lob = None;
                    r.triangulated = false;
                    if d.polar.elev.is_some() {
                        r.alt_known = true;
                    }
                }
                if r.cooperative {
                    r.corroborate(&key, d.t);
                } else {
                    r.add_hit(d.t);
                }
                r.upsert_source(src, &d.platform, d.t, true, d.scan_ms);
                self.stats.det_updated += 1;
            }
            CandKind::LobSame | CandKind::LobCross => {
                let fit = {
                    let Some(lob) = r.lob.as_mut() else { return };
                    lob.push(Self::lob_obs(d), cfg.tri_window_ms);
                    lob.fit(cfg)
                };
                r.add_hit(d.t);
                r.upsert_source(src, &d.platform, d.t, true, d.scan_ms);
                if let Some(f) = fit {
                    r.kf = Some(kf_from_fit(&f, d.polar.s[2], cfg));
                    r.alt_known = f.z.is_some();
                    r.triangulated = true;
                    self.stats.triangulated += 1;
                }
                self.stats.det_updated += 1;
            }
            CandKind::LobFix => {
                let Some(kf) = fix_kf else { return };
                r.kf = Some(kf);
                r.lob = None;
                r.alt_known = d.polar.elev.is_some();
                r.add_hit(d.t);
                r.upsert_source(src, &d.platform, d.t, true, d.scan_ms);
                self.stats.det_updated += 1;
            }
        }
    }

    // ---------------------------------------------------------------- update

    /// Bring the picture to `t_ms`: process held detections, age and drop tracks, check
    /// integrity, and rebuild [`Tracker::tracks`] at that time.
    pub fn update(&mut self, t_ms: i64) {
        self.now = t_ms;
        self.flush_dets();
        let w = self.cfg.dedup_window_ms * 2;
        if self.dedup.len() > 4096 {
            self.dedup.retain(|_, v| t_ms - v.0 <= w);
        }
        self.lifecycle(t_ms);
        self.merge_into_cooperative(t_ms);
        self.handover(t_ms);
        self.check_disagreement(t_ms);
        self.build_output(t_ms);
    }

    fn lifecycle(&mut self, t: i64) {
        let (m, n) = (self.cfg.confirm_m, self.cfg.confirm_n);
        let mut drop = Vec::new();
        for (&no, r) in self.recs.iter_mut() {
            if !r.cooperative && !r.confirmed {
                let hits = r.hit_times.len() as u32;
                if hits >= m {
                    r.confirmed = true;
                } else {
                    let active_until = r
                        .init_sensor
                        .as_ref()
                        .and_then(|k| self.platforms.get(k))
                        .map_or(t, |p| p.last_det_t + r.scan_ms / 2);
                    let span = active_until.min(t) - r.first_t;
                    let looks = (span.max(0) / r.scan_ms.max(1)) as u32 + 1;
                    if looks.saturating_sub(hits) > n.saturating_sub(m) {
                        drop.push(no);
                        continue;
                    }
                }
            }
            let (_, drop_at) = r.deadlines(&self.cfg);
            if t > drop_at {
                drop.push(no);
            }
        }
        for no in drop {
            if let Some(r) = self.recs.remove(&no) {
                if let Some(id) = &r.identity
                    && self.by_id.get(id) == Some(&no)
                {
                    self.by_id.remove(id);
                }
                self.stats.tracks_dropped += 1;
            }
        }
    }

    /// A non-cooperative track that is the same object as a cooperative one (inside a tight
    /// gate on position and velocity) is folded into it.
    fn merge_into_cooperative(&mut self, t: i64) {
        let g4 = chi2_quantile(0.99, 4);
        let mut merges: Vec<(u64, u64)> = Vec::new();
        let coop: Vec<(u64, Domain, [f64; 6], M6)> = self
            .recs
            .values()
            .filter(|r| r.cooperative && r.confirmed)
            .filter_map(|r| r.kf.as_ref().map(|k| (r.no, r.domain, k.at(t))))
            .map(|(no, d, (x, p))| (no, d, x, p))
            .collect();
        for r in self.recs.values() {
            if r.cooperative {
                continue;
            }
            let Some(kf) = &r.kf else { continue };
            let (x, p) = kf.at(t);
            let mut best: Option<(f64, u64)> = None;
            for (cno, dom, cx, cp) in &coop {
                if *dom != r.domain {
                    continue;
                }
                let idx = [0usize, 1, 3, 4];
                let mut s = [[0.0; MAXM]; MAXM];
                let mut nu = [0.0; MAXM];
                for (a, &i) in idx.iter().enumerate() {
                    nu[a] = x[i] - cx[i];
                    for (b, &j) in idx.iter().enumerate() {
                        s[a][b] = p[i][j] + cp[i][j];
                    }
                }
                let Some((si, _)) = crate::kf::invert(&s, 4) else {
                    continue;
                };
                let d2: f64 = (0..4)
                    .map(|a| (0..4).map(|b| nu[a] * si[a][b] * nu[b]).sum::<f64>())
                    .sum();
                if d2 <= g4 && best.is_none_or(|b| d2 < b.0) {
                    best = Some((d2, *cno));
                }
            }
            if let Some((_, cno)) = best {
                merges.push((r.no, cno));
            }
        }
        for (nno, cno) in merges {
            let Some(n) = self.recs.remove(&nno) else {
                continue;
            };
            if let Some(c) = self.recs.get_mut(&cno) {
                for s in &n.sources {
                    if !s.kind.cooperative() {
                        let key = match s.kind {
                            SourceKind::Daa => SensorKind::Daa,
                            SourceKind::MarineRadar => SensorKind::MarineRadar,
                            _ => SensorKind::GroundRadar,
                        };
                        c.corroborate(&(key, s.rx.clone()), s.last_t);
                        c.upsert_source(s.kind, &s.rx, s.last_t, false, s.interval_ms);
                    }
                }
            }
            self.stats.tracks_merged += 1;
        }
    }

    /// A new anonymous identity that starts where an old anonymous one went quiet continues the
    /// old track (a rotating Privacy ICAO Address or Remote ID session id).
    fn handover(&mut self, t: i64) {
        let hw = self.cfg.handover_ms;
        let young: Vec<u64> = self
            .recs
            .values()
            .filter(|r| r.cooperative && r.anonymous && r.kf.is_some() && t - r.first_id_t <= hw)
            .map(|r| r.no)
            .collect();
        let mut taken: HashSet<u64> = HashSet::new();
        for yno in young {
            let Some(y) = self.recs.get(&yno) else {
                continue;
            };
            let Some(ykf) = y.kf.clone() else { continue };
            let (yfirst, ydom) = (y.first_id_t, y.domain);
            let mut best: Option<(f64, u64)> = None;
            for o in self.recs.values() {
                if o.no == yno
                    || !o.cooperative
                    || !o.anonymous
                    || o.domain != ydom
                    || taken.contains(&o.no)
                {
                    continue;
                }
                if o.first_id_t >= yfirst
                    || o.last_report_t > yfirst
                    || yfirst - o.last_report_t > hw
                {
                    continue;
                }
                let Some(okf) = &o.kf else { continue };
                let (ox, op) = okf.at(ykf.t_ms);
                let s = [
                    [op[0][0] + ykf.p[0][0], op[0][1] + ykf.p[0][1], 0.0, 0.0],
                    [op[1][0] + ykf.p[1][0], op[1][1] + ykf.p[1][1], 0.0, 0.0],
                    [0.0; 4],
                    [0.0; 4],
                ];
                let Some((si, _)) = crate::kf::invert(&s, 2) else {
                    continue;
                };
                let nu = [ykf.x[0] - ox[0], ykf.x[1] - ox[1]];
                let d2 = nu[0] * (si[0][0] * nu[0] + si[0][1] * nu[1])
                    + nu[1] * (si[1][0] * nu[0] + si[1][1] * nu[1]);
                if d2 > self.cfg.handover_chi2 {
                    continue;
                }
                // Velocities must agree too when the new track knows its own.
                if ykf.p[3][3] < 100.0 {
                    let dv = ((ykf.x[3] - ox[3]).powi(2) + (ykf.x[4] - ox[4]).powi(2)).sqrt();
                    let sv = (ykf.p[3][3] + ykf.p[4][4] + op[3][3] + op[4][4]).sqrt() + 3.0;
                    if dv > 3.0 * sv {
                        continue;
                    }
                }
                if best.is_none_or(|b| d2 < b.0) {
                    best = Some((d2, o.no));
                }
            }
            let Some((_, ono)) = best else { continue };
            taken.insert(ono);
            let Some(y) = self.recs.remove(&yno) else {
                continue;
            };
            let Some(o) = self.recs.get_mut(&ono) else {
                continue;
            };
            if let Some(oid) = o.identity.take() {
                self.by_id.remove(&oid);
            }
            if let Some(yid) = &y.identity {
                self.by_id.insert(yid.clone(), ono);
            }
            o.identity = y.identity.clone();
            o.ids = y.ids.clone();
            o.kf = y.kf.clone();
            o.alt_ref = y.alt_ref;
            o.alt_known = y.alt_known;
            o.first_id_t = y.first_id_t;
            o.last_report_t = y.last_report_t;
            o.pos_reports += y.pos_reports;
            o.confirmed = true;
            o.id_changed_until = t + 60_000;
            merge_static(&mut o.statics, &y.statics);
            for s in &y.sources {
                if let Some(k) = s.last_kin_t {
                    o.upsert_source(s.kind, &s.rx, k, true, s.interval_ms);
                }
                o.upsert_source(s.kind, &s.rx, s.last_t, false, s.interval_ms);
            }
            self.stats.handovers += 1;
        }
    }

    fn check_disagreement(&mut self, t: i64) {
        let cfg = &self.cfg;
        let g_pos = chi2_quantile(0.999, 2);
        let g_vel = chi2_quantile(0.99, 2);
        struct Snap {
            no: u64,
            domain: Domain,
            x: [f64; 6],
            p: M6,
            alt: Option<(f64, f64)>,
            sensors: Vec<(SensorKey, i64)>,
        }
        let snap = |r: &Rec, kf: &Kf| -> Snap {
            let (x, p) = kf.at(t);
            let alt = if r.alt_known && r.domain == Domain::Air && p[2][2].sqrt() < 300.0 {
                let (off, _) = self.alt_offset(r.alt_ref, AltRef::Geo);
                Some((x[2] + off, p[2][2].sqrt()))
            } else {
                None
            };
            let sensors = r
                .sources
                .iter()
                .filter(|s| !s.kind.cooperative())
                .filter_map(|s| {
                    let k = match s.kind {
                        SourceKind::Daa => SensorKind::Daa,
                        SourceKind::MarineRadar => SensorKind::MarineRadar,
                        _ => SensorKind::GroundRadar,
                    };
                    s.last_kin_t.map(|lt| ((k, s.rx.clone()), lt))
                })
                .collect();
            Snap {
                no: r.no,
                domain: r.domain,
                x,
                p,
                alt,
                sensors,
            }
        };
        let sensed: Vec<Snap> = self
            .recs
            .values()
            .filter(|r| !r.cooperative && r.confirmed && r.lob.is_none())
            .filter_map(|r| r.kf.as_ref().map(|k| snap(r, k)))
            .collect();
        let mut results: Vec<(u64, Pairing)> = Vec::new();
        for r in self.recs.values() {
            if !r.cooperative
                || !r.confirmed
                || r.on_ground
                || matches!(r.nav_status, Some(1) | Some(5))
            {
                if r.cooperative {
                    results.push((r.no, None));
                }
                continue;
            }
            let Some(kf) = &r.kf else { continue };
            let c = snap(r, kf);
            let cspeed = (c.x[3].powi(2) + c.x[4].powi(2)).sqrt();
            let mut best: Option<(f64, u64, String)> = None;
            if cspeed >= cfg.disagree_min_speed_mps {
                for n in &sensed {
                    if n.domain != c.domain {
                        continue;
                    }
                    let dx = n.x[0] - c.x[0];
                    let dy = n.x[1] - c.x[1];
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist > cfg.disagree_radius_m {
                        continue;
                    }
                    // Not the same object by position ...
                    let s = [
                        [n.p[0][0] + c.p[0][0], n.p[0][1] + c.p[0][1], 0.0, 0.0],
                        [n.p[1][0] + c.p[1][0], n.p[1][1] + c.p[1][1], 0.0, 0.0],
                        [0.0; 4],
                        [0.0; 4],
                    ];
                    let Some((si, _)) = crate::kf::invert(&s, 2) else {
                        continue;
                    };
                    let d2p =
                        dx * (si[0][0] * dx + si[0][1] * dy) + dy * (si[1][0] * dx + si[1][1] * dy);
                    if d2p <= g_pos {
                        continue;
                    }
                    // ... but moving with it: a well-measured velocity that matches closely (a loose
                    // velocity "matches" anything).
                    let nspeed = (n.x[3].powi(2) + n.x[4].powi(2)).sqrt();
                    if nspeed < cfg.disagree_min_speed_mps {
                        continue;
                    }
                    if ellipse(n.p[3][3], n.p[3][4], n.p[4][4]).0 > cfg.disagree_max_vel_sigma_mps {
                        continue;
                    }
                    let (dvx, dvy) = (n.x[3] - c.x[3], n.x[4] - c.x[4]);
                    if (dvx * dvx + dvy * dvy).sqrt() > (0.1 * cspeed).max(3.0) {
                        continue;
                    }
                    let sv = [
                        [n.p[3][3] + c.p[3][3] + 4.0, n.p[3][4] + c.p[3][4], 0.0, 0.0],
                        [n.p[4][3] + c.p[4][3], n.p[4][4] + c.p[4][4] + 4.0, 0.0, 0.0],
                        [0.0; 4],
                        [0.0; 4],
                    ];
                    let Some((svi, _)) = crate::kf::invert(&sv, 2) else {
                        continue;
                    };
                    let d2v = dvx * (svi[0][0] * dvx + svi[0][1] * dvy)
                        + dvy * (svi[1][0] * dvx + svi[1][1] * dvy);
                    if d2v > g_vel {
                        continue;
                    }
                    if let (Some((za, sa)), Some((zb, sb))) = (c.alt, n.alt)
                        && (za - zb).abs() > 3.0 * (sa * sa + sb * sb).sqrt() + 150.0
                    {
                        continue;
                    }
                    // A sensor that sees the other object, has the broadcast position in its
                    // coverage, and has not seen anything there.
                    for (skey, slast) in &n.sensors {
                        let Some(pl) = self.platforms.get(skey) else {
                            continue;
                        };
                        if t - slast > 3 * pl.scan_ms.max(1_000) {
                            continue;
                        }
                        let (ex, ey) = (c.x[0] - pl.s[0], c.x[1] - pl.s[1]);
                        if (ex * ex + ey * ey).sqrt() > pl.range_m {
                            continue;
                        }
                        let seen = r
                            .corroborated
                            .iter()
                            .find(|(k, _)| k == skey)
                            .map(|(_, ct)| *ct);
                        if seen.is_some_and(|ct| *slast - ct <= 2 * pl.scan_ms.max(1_000)) {
                            continue;
                        }
                        if best.as_ref().is_none_or(|b| dist < b.0) {
                            best = Some((dist, n.no, format!("{}:{}", skey.0.as_str(), skey.1)));
                        }
                    }
                }
            }
            results.push((r.no, best.map(|(d, n, s)| (n, s, d))));
        }
        let mut sensed_flag: Vec<(u64, i64)> = Vec::new();
        for (cno, pair) in results {
            let Some(r) = self.recs.get_mut(&cno) else {
                continue;
            };
            match pair {
                Some((nno, sensor, dist)) => {
                    let since = *r.disagree.candidate_since.get_or_insert(t);
                    if t - since >= cfg.disagree_persist_ms {
                        r.disagree.until = t + cfg.disagree_hold_ms;
                        let first = r.disagree.pair.as_ref().map_or(since, |p| p.3);
                        r.disagree.pair = Some((nno, sensor, dist, first));
                        sensed_flag.push((nno, r.disagree.until));
                    }
                }
                None => {
                    r.disagree.candidate_since = None;
                    if t > r.disagree.until {
                        r.disagree.pair = None;
                    }
                }
            }
        }
        for (nno, until) in sensed_flag {
            if let Some(n) = self.recs.get_mut(&nno) {
                n.disagree.until = n.disagree.until.max(until);
            }
        }
        self.disagreements = self
            .recs
            .values()
            .filter(|r| r.cooperative && r.disagree.until >= t)
            .filter_map(|r| {
                let (nno, sensor, dist, since) = r.disagree.pair.clone()?;
                Some(Disagreement {
                    track: format!("trk-{:06}", r.no),
                    sensed: format!("trk-{nno:06}"),
                    offset_m: round_to(dist, 1),
                    sensor,
                    since_t: since,
                })
            })
            .collect();
    }

    fn build_output(&mut self, t: i64) {
        let cfg = &self.cfg;
        let mut out = Vec::with_capacity(self.recs.len());
        let mut st = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        for r in self.recs.values() {
            let (coast_at, _) = r.deadlines(cfg);
            let state = if !r.confirmed {
                TrackState::Tentative
            } else if t > coast_at {
                TrackState::Coasting
            } else {
                TrackState::Confirmed
            };
            let mut flags = Vec::new();
            let mut kin = Kinematics::default();
            let (lat, lon, alt_m, alt_ref, h_sigma, v_sigma, vel, vs, mut heading, bearing_only);
            if let Some(kf) = &r.kf {
                let (x, p) = kf.at(t.max(kf.t_ms));
                let (la, lo) = self.local.to_latlon(x[0], x[1]);
                let gamma = self.local.convergence(la, lo);
                let (ve, vn) = grid_to_true(gamma, x[3], x[4]);
                let (major, _, _) = ellipse(p[0][0], p[0][1], p[1][1]);
                lat = la;
                lon = lo;
                h_sigma = major;
                let zs = p[2][2].sqrt();
                if r.domain == Domain::Air && r.alt_known && zs <= cfg.max_alt_sigma_out_m {
                    alt_m = Some(round_to(x[2], 1));
                    alt_ref = r.alt_ref;
                    v_sigma = Some(round_to(zs, 1));
                    vs = Some(round_to(x[5], 2));
                } else {
                    alt_m = None;
                    alt_ref = AltRef::None;
                    v_sigma = None;
                    vs = None;
                }
                vel = Some([round_to(ve, 2), round_to(vn, 2)]);
                let speed = (ve * ve + vn * vn).sqrt();
                heading = if speed >= 0.5 {
                    Some(round_to(wrap_360(ve.atan2(vn).to_degrees()), 1))
                } else {
                    None
                };
                bearing_only = None;
                kin = Kinematics {
                    x,
                    p,
                    qh: kf.qh,
                    qv: kf.qv,
                    surface: kf.surface,
                    valid: true,
                };
            } else if let Some(o) = r.lob.as_ref().and_then(|l| l.latest()) {
                let r_lo = R_MIN;
                let r_hi = o.r_max;
                let mid = 0.5 * (r_lo + r_hi);
                let (sx, sy) = (o.s[0] + mid * o.beta.sin(), o.s[1] + mid * o.beta.cos());
                let (la, lo) = self.local.to_latlon(sx, sy);
                let (sla, slo) = self.local.to_latlon(o.s[0], o.s[1]);
                lat = la;
                lon = lo;
                h_sigma = (0.5 * (r_hi - r_lo)).max(mid * o.sb);
                match o.elev {
                    Some((e, se)) => {
                        let vsg = 0.5 * (r_hi - r_lo) * e.tan().abs() + mid * se;
                        if vsg <= cfg.max_alt_sigma_out_m {
                            alt_m = Some(round_to(o.s[2] + mid * e.tan(), 1));
                            alt_ref = AltRef::Geo;
                            v_sigma = Some(round_to(vsg, 1));
                        } else {
                            alt_m = None;
                            alt_ref = AltRef::None;
                            v_sigma = None;
                        }
                    }
                    None => {
                        alt_m = None;
                        alt_ref = AltRef::None;
                        v_sigma = None;
                    }
                }
                vel = None;
                vs = None;
                heading = None;
                bearing_only = Some(BearingLine {
                    sensor_lat: round_to(sla, 7),
                    sensor_lon: round_to(slo, 7),
                    sensor_alt_m: round_to(o.s[2], 1),
                    bearing_deg: round_to(wrap_360(o.beta_true.to_degrees()), 2),
                    sigma_bearing_deg: round_to(o.sb.to_degrees(), 2),
                    elev_deg: o.elev.map(|(e, _)| round_to(e.to_degrees(), 2)),
                    range_m: [r_lo, r_hi],
                    t: o.t,
                });
                flags.push(Flag::BearingOnly);
            } else {
                continue;
            }
            if let Some((h, ht)) = r.heading
                && t - ht <= cfg.timing(r.primary).coast_ms(r.interval_ms)
            {
                heading = Some(h);
            }
            // Flags.
            if r.cooperative {
                let tm = cfg.timing(r.primary);
                if let Some(k) = r.last_primary_kin()
                    && t - k > tm.stale_ms(r.interval_ms)
                {
                    flags.push(Flag::Stale);
                }
            } else if let Some(k) =
                r.last_primary_kin()
                    .or(r.lob.as_ref().and_then(|l| l.latest()).map(|o| o.t))
                && t - k > (2 * r.scan_ms).max(2_000) + r.scan_ms / 2
            {
                flags.push(Flag::Stale);
            }
            if r.anonymous {
                flags.push(Flag::Anonymous);
            }
            if r.ladd {
                flags.push(Flag::Ladd);
            }
            if t <= r.id_changed_until {
                flags.push(Flag::IdChanged);
            }
            if r.disagree.until >= t {
                flags.push(Flag::SensorDisagree);
            }
            if t <= r.jump_until {
                flags.push(Flag::PositionJump);
            }
            if r.invalid_mmsi {
                flags.push(Flag::InvalidMmsi);
            }
            if r.on_ground && r.domain == Domain::Air {
                flags.push(Flag::OnGround);
            }
            match r.nav_status {
                Some(5) => flags.push(Flag::Moored),
                Some(1) => flags.push(Flag::AtAnchor),
                _ => {}
            }
            if r.emergency {
                flags.push(Flag::Emergency);
            }
            if r.triangulated && r.kf.is_some() {
                flags.push(Flag::Triangulated);
            }
            flags.sort();
            flags.dedup();
            let mut sources: Vec<Source> = r
                .sources
                .iter()
                .filter(|s| t - s.last_t <= cfg.timing(s.kind).drop_ms(s.interval_ms))
                .map(|s| Source {
                    kind: s.kind,
                    rx: s.rx.clone(),
                    last_t: s.last_t,
                })
                .collect();
            sources.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.rx.cmp(&b.rx)));
            let mut ids = r.ids.clone();
            ids.anonymous = r.anonymous;
            if r.ladd {
                ids.callsign = None;
            }
            match state {
                TrackState::Tentative => st.0 += 1,
                TrackState::Confirmed => st.1 += 1,
                TrackState::Coasting => st.2 += 1,
            }
            if r.cooperative {
                st.3 += 1;
            }
            if bearing_only.is_some() {
                st.4 += 1;
            }
            if r.disagree.until >= t && r.cooperative {
                st.5 += 1;
            }
            out.push(Track {
                id: format!("trk-{:06}", r.no),
                t,
                domain: r.domain,
                class: r.class,
                cooperative: r.cooperative,
                ids,
                lat: round_to(lat, 7),
                lon: round_to(lon, 7),
                alt_m,
                alt_ref,
                h_sigma_m: round_to(h_sigma, 1),
                v_sigma_m: v_sigma,
                vel_mps: vel,
                vs_mps: vs,
                heading_deg: heading,
                state,
                sources,
                flags,
                static_: r.statics.clone(),
                bearing_only,
                kin,
            });
        }
        self.stats.tracks_live = out.len() as u64;
        self.stats.tracks_tentative = st.0;
        self.stats.tracks_confirmed = st.1;
        self.stats.tracks_coasting = st.2;
        self.stats.tracks_cooperative = st.3;
        self.stats.tracks_bearing_only = st.4;
        self.stats.disagreements = st.5;
        self.out = out;
    }

    // ---------------------------------------------------------------- prediction

    /// Where a track will be `dt_s` seconds after its time, with the filter's growing
    /// uncertainty. A bearing-only track stays on its line with its sigma growing at the
    /// domain's top speed.
    pub fn predict(&self, track: &Track, dt_s: f64) -> Prediction {
        let t = track.t + (dt_s * 1000.0).round() as i64;
        if !track.kin.valid {
            let vmax = if track.domain == Domain::Air {
                80.0
            } else {
                15.0
            };
            let h = track.h_sigma_m + vmax * dt_s.abs();
            return Prediction {
                t,
                lat: track.lat,
                lon: track.lon,
                alt_m: track.alt_m,
                h_sigma_m: h,
                h_sigma_minor_m: h,
                h_sigma_dir_deg: 0.0,
                v_sigma_m: track.v_sigma_m,
            };
        }
        let k = &track.kin;
        let (x, p) = propagate(&k.x, &k.p, dt_s, k.qh, k.qv, k.surface);
        let (lat, lon) = self.local.to_latlon(x[0], x[1]);
        let (major, minor, dir) = ellipse(p[0][0], p[0][1], p[1][1]);
        let gamma = self.local.convergence(lat, lon);
        let (alt_m, v_sigma) = if track.alt_m.is_some() {
            (Some(round_to(x[2], 1)), Some(round_to(p[2][2].sqrt(), 1)))
        } else {
            (None, None)
        };
        Prediction {
            t,
            lat: round_to(lat, 7),
            lon: round_to(lon, 7),
            alt_m,
            h_sigma_m: round_to(major, 1),
            h_sigma_minor_m: round_to(minor, 1),
            h_sigma_dir_deg: round_to(wrap_360((dir - gamma).to_degrees()), 1),
            v_sigma_m: v_sigma,
        }
    }

    /// Predictions every `step_s` seconds out to `horizon_s`.
    pub fn predict_path(&self, track: &Track, horizon_s: f64, step_s: f64) -> Vec<Prediction> {
        let step = step_s.max(0.1);
        let n = (horizon_s / step).floor() as usize;
        (1..=n)
            .map(|i| self.predict(track, i as f64 * step))
            .collect()
    }
}
