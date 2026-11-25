//! Broadcasts and their reception.
//!
//! Each object's transmitters run on their own schedules (ms of scenario time), each event at
//! a time that depends only on the object, the channel and the event's number:
//!
//! - ADS-B 1090 extended squitter (DO-260B): airborne position and velocity every 0.5 s
//!   (+-0.1 s jitter), identification every 5 s, the acquisition squitter (DF11) every second,
//!   surface position every 0.5 s moving or 5 s still; replies to the terminal radar's
//!   interrogations (DF4/5/20/21) once a 4.8 s sweep; TCAS replies (DF0/16) for airliners.
//! - UAT 978: one ADS-B message a second, the mode status (callsign) one in four.
//! - AIS Class A (ITU-R M.1371-5 table 1): every 3 min moored or at anchor below 3 kn, 10 s
//!   under 14 kn (3 1/3 s turning), 6 s to 23 kn (2 s turning), 2 s above; message 5 every 6
//!   min. Class B CS (table 2): 30 s above 2 kn, 3 min below; message 24 A and B every 6 min.
//! - Remote ID (14 CFR 89, ASTM F3411): a location message every second.
//!
//! A receiver hears a transmission when it is up, within its range (shortened for Class B's
//! 2 W and by the Remote ID transport), in radio line of sight, and the loss draw passes:
//! `base + (1 - base) (d / range)^n`, plus AIS slot collisions rising with the channel load
//! around the receiver. Every draw is keyed by object, channel, event and receiver.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use super::{Kind, Receiver, Site};
use crate::encode::ais::{self, Bits, ExtendedB, PositionA, PositionB, StaticA, StaticB, StaticVoyage};
use crate::encode::json::{self, RidMsg};
use crate::encode::sbs::{self, SbsMsg};
use crate::geo::{self, FT, KT, LatLon};
use crate::path::Kin;
use crate::rng;
use crate::terrain::{Blocked, Terrain};
use crate::world::{Adsb, Ais, Class, Mover};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ch {
    EsPos = 1,
    EsVel,
    EsIdent,
    EsDf11,
    EsSurv5,
    EsSurv6,
    EsAcas,
    Uat,
    AisPos,
    AisStatic,
    AisB24,
    AisB19,
    Rid,
}

#[derive(Clone, Debug)]
pub struct Chan {
    pub ch: Ch,
    pub next_ms: i64,
    pub k: u64,
    pub done: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TxState {
    pub chans: Vec<Chan>,
    pub init: bool,
}

/// Which TCP stream a line goes out on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Sbs,
    Ais,
    Json,
}

#[derive(Clone, Debug)]
pub enum Body {
    Sbs(SbsMsg),
    Ais { bits: Bits, channel: char, c_s: i64 },
    Json(String),
}

/// A message on its way: when it is delivered, a key that orders it, where it came from.
#[derive(Clone, Debug)]
pub struct Out {
    pub deliver_ms: i64,
    pub order: (u64, i64, u64, u16, u64, u8),
    pub src: String,
    pub kind: Kind,
    pub body: Body,
}

impl Out {
    pub fn stream(&self) -> Stream {
        match self.body {
            Body::Sbs(_) => Stream::Sbs,
            Body::Ais { .. } => Stream::Ais,
            Body::Json(_) => Stream::Json,
        }
    }
}

/// A Remote ID receiver on an own vehicle.
#[derive(Clone, Debug)]
pub struct OwnRx {
    pub id: String,
    pub p: LatLon,
    pub alt_m: f64,
    pub order: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RadioStats {
    /// transmissions by medium
    pub transmitted: BTreeMap<&'static str, u64>,
    /// receptions by `<receiver>/<kind>`
    pub received: BTreeMap<String, u64>,
    /// receptions lost, by reason
    pub lost: BTreeMap<&'static str, u64>,
    pub los_cache: usize,
}

type LosKey = (u32, u32, u8, i32, i32, i32);

pub struct Env<'a> {
    pub seed: u64,
    pub start_ms: i64,
    /// pressure altitude = altitude - this
    pub qnh_off_m: f64,
    /// geoid height above the ellipsoid (Remote ID's geodetic altitude is above the ellipsoid)
    pub geoid_n_m: f64,
    pub region: [f64; 4],
    pub rx: &'a [Receiver],
    pub own_rid: &'a [OwnRx],
    pub own_rid_range_m: f64,
    pub terrain: &'a Terrain,
    pub los: &'a mut HashMap<LosKey, bool>,
    pub ais_load: &'a HashMap<(usize, i64), f64>,
    pub stats: &'a mut RadioStats,
}

/// The AIS load bucket: ten seconds.
pub const LOAD_BUCKET_MS: i64 = 10_000;

#[derive(Clone, Copy, Debug)]
enum Lost {
    Down,
    Range,
    Horizon,
    Terrain,
    Loss,
    Slot,
}

impl Lost {
    fn as_str(self) -> &'static str {
        match self {
            Lost::Down => "receiver_down",
            Lost::Range => "range",
            Lost::Horizon => "horizon",
            Lost::Terrain => "terrain",
            Lost::Loss => "fade",
            Lost::Slot => "ais_slot",
        }
    }
}

impl Env<'_> {
    fn in_region(&self, p: LatLon) -> bool {
        let [a, b, c, d] = self.region;
        p.lat >= a && p.lat <= b && p.lon >= c && p.lon <= d
    }

    fn los(&mut self, rx: usize, si: usize, s: &Site, p: LatLon, alt: f64) -> Result<(), Lost> {
        let (dq, band, level) = if alt < 150.0 {
            (0.0005, 10.0, 0u8)
        } else if alt < 1_500.0 {
            (0.002, 50.0, 1)
        } else if alt < 6_000.0 {
            (0.01, 300.0, 2)
        } else {
            (0.05, 2_000.0, 3)
        };
        let dlon = dq / 0.79;
        let (qi, qj, qa) = ((p.lat / dq).floor() as i32, (p.lon / dlon).floor() as i32, (alt / band).floor() as i32);
        let key = (rx as u32, si as u32, level, qi, qj, qa);
        if let Some(ok) = self.los.get(&key) {
            return if *ok { Ok(()) } else { Err(Lost::Terrain) };
        }
        if self.los.len() > 2_000_000 {
            // a pure function of its key: forgetting changes nothing but the time
            self.los.clear();
        }
        let c = LatLon::new((f64::from(qi) + 0.5) * dq, (f64::from(qj) + 0.5) * dlon);
        let ca = (f64::from(qa) + 0.5) * band;
        let r = self.terrain.line_of_sight(s.p, s.alt_m, c, ca.max(alt.min(1.0)));
        let ok = r.is_ok();
        self.los.insert(key, ok);
        self.stats.los_cache = self.los.len();
        match r {
            Ok(()) => Ok(()),
            Err(Blocked::Horizon) => Err(Lost::Horizon),
            Err(Blocked::Terrain) => Err(Lost::Terrain),
        }
    }

    /// Whether receiver `ri` hears a transmission from `p` at `alt` (MSL), and how far.
    #[allow(clippy::too_many_arguments)]
    fn hear(&mut self, ri: usize, p: LatLon, alt: f64, t_s: f64, draw: u64, range_cap: f64, exp: f64, t_ms: i64) -> Result<f64, Lost> {
        let rxs = self.rx;
        let rx = &rxs[ri];
        if !rx.up(t_s) {
            return Err(Lost::Down);
        }
        let range = rx.range_m.min(range_cap);
        let mut best: Option<f64> = None;
        let mut why = Lost::Range;
        for (si, s) in rx.sites.iter().enumerate() {
            let g = geo::haversine_m(s.p, p);
            let d = (g * g + (alt - s.alt_m).powi(2)).sqrt();
            if d > range {
                continue;
            }
            match self.los(ri, si, s, p, alt) {
                Ok(()) => best = Some(best.map_or(d, |b: f64| b.min(d))),
                Err(e) => why = e,
            }
        }
        let d = best.ok_or(why)?;
        let p_loss = rx.base_loss + (1.0 - rx.base_loss) * (d / range).powf(exp);
        if rng::unit(rng::hash(draw, &[rx.index as u64, 1])) < p_loss {
            return Err(Lost::Loss);
        }
        if rx.kind == Kind::Ais && rx.slot_factor > 0.0 {
            let load = self.ais_load.get(&(ri, t_ms.div_euclid(LOAD_BUCKET_MS))).copied().unwrap_or(0.0);
            let p_slot = 1.0 - (-load * rx.slot_factor).exp();
            if rng::unit(rng::hash(draw, &[rx.index as u64, 2])) < p_slot {
                return Err(Lost::Slot);
            }
        }
        Ok(d)
    }

    fn count(&mut self, src: &str, kind: Kind) {
        *self.stats.received.entry(format!("{src}/{}", kind.as_str())).or_insert(0) += 1;
    }

    fn lose(&mut self, l: Lost) {
        *self.stats.lost.entry(l.as_str()).or_insert(0) += 1;
    }
}

fn chans_for(m: &Mover) -> Vec<Ch> {
    let mut v = Vec::new();
    match m.equip.adsb {
        Adsb::Es1090 => {
            v.extend([Ch::EsPos, Ch::EsVel, Ch::EsIdent, Ch::EsDf11, Ch::EsSurv5, Ch::EsSurv6]);
            if matches!(m.class, Class::Airliner | Class::Bizjet) {
                v.push(Ch::EsAcas);
            }
        }
        Adsb::Uat => v.push(Ch::Uat),
        Adsb::ModeS => v.extend([Ch::EsDf11, Ch::EsSurv5, Ch::EsSurv6]),
        Adsb::None => {}
    }
    if !m.equip.ais_off {
        match m.equip.ais {
            Ais::A => v.extend([Ch::AisPos, Ch::AisStatic]),
            Ais::B => {
                v.extend([Ch::AisPos, Ch::AisB24]);
                if m.equip.type19 {
                    v.push(Ch::AisB19);
                }
            }
            Ais::None => {}
        }
    }
    if m.equip.rid.is_some() {
        v.push(Ch::Rid);
    }
    v
}

fn base_period_ms(ch: Ch) -> i64 {
    match ch {
        Ch::EsPos | Ch::EsVel => 500,
        Ch::EsIdent | Ch::EsAcas => 5_000,
        Ch::EsDf11 | Ch::Uat | Ch::Rid => 1_000,
        Ch::EsSurv5 | Ch::EsSurv6 => 4_800,
        Ch::AisPos => 10_000,
        Ch::AisStatic | Ch::AisB24 | Ch::AisB19 => 360_000,
    }
}

fn jitter(key: u64, ch: Ch, k: u64, j_ms: i64) -> i64 {
    if j_ms <= 0 {
        return 0;
    }
    (rng::hash(key, &[ch as u64, k, 0x717]) % (2 * j_ms as u64 + 1)) as i64 - j_ms
}

/// Class A reporting interval (M.1371-5 table 1).
fn class_a_interval_ms(k: &Kin, turn_dps: f64) -> i64 {
    let kn = k.gs_mps / KT;
    let turning = turn_dps.abs() > 0.1;
    if matches!(k.status, 1 | 5) && kn <= 3.0 {
        180_000
    } else if kn > 23.0 {
        2_000
    } else if kn > 14.0 {
        if turning { 2_000 } else { 6_000 }
    } else if turning {
        3_333
    } else {
        10_000
    }
}

/// The broadcast position: truth, the object's navigation error, any fault.
fn broadcast(m: &Mover, k: &Kin, t_s: f64, k_ev: u64, ch: Ch) -> LatLon {
    let sig = m.equip.nav_sigma_m;
    let n = sig * rng::Smooth::new(m.key ^ 0x0A, 120.0).at(t_s) + 0.25 * sig * rng::normal(rng::hash(m.key, &[ch as u64, k_ev, 1]));
    let e = sig * rng::Smooth::new(m.key ^ 0x0B, 120.0).at(t_s) + 0.25 * sig * rng::normal(rng::hash(m.key, &[ch as u64, k_ev, 2]));
    let (fn_, fe) = m.offset(t_s).unwrap_or((0.0, 0.0));
    let p = geo::destination(k.p, 0.0, n + fn_);
    geo::destination(p, 90.0, e + fe)
}

fn utc_second(abs_ms: i64) -> u8 {
    (abs_ms.div_euclid(1000).rem_euclid(60)) as u8
}

/// Run one object's transmitters up to `until_ms` (scenario ms).
pub fn advance(m: &mut Mover, until_ms: i64, env: &mut Env, out: &mut Vec<Out>) {
    let mut tx = std::mem::take(&mut m.tx);
    if !tx.init {
        tx.init = true;
        let start = ((m.plan.t0 * 1000.0).ceil() as i64).max(0);
        tx.chans = chans_for(m)
            .into_iter()
            .map(|ch| {
                let p = base_period_ms(ch);
                let phase = (rng::hash(m.key, &[ch as u64, 0xF1]) % p as u64) as i64;
                Chan { ch, next_ms: start + phase, k: 0, done: false }
            })
            .collect();
    }
    for c in tx.chans.iter_mut() {
        let mut guard = 0;
        while !c.done && c.next_ms <= until_ms && guard < 100_000 {
            guard += 1;
            let t_ms = c.next_ms;
            match emit(m, c, t_ms, env, out) {
                Some(next) => {
                    c.k += 1;
                    c.next_ms = t_ms + next.max(1);
                }
                None => c.done = true,
            }
        }
    }
    m.tx = tx;
}

/// One event: what goes out and to whom. Returns the time to the next event, None when done.
fn emit(m: &Mover, c: &Chan, t_ms: i64, env: &mut Env, out: &mut Vec<Out>) -> Option<i64> {
    let t_s = t_ms as f64 / 1000.0;
    let p0 = base_period_ms(c.ch);
    let Some(kin) = m.state(t_s) else {
        return if t_s > m.plan.t1 { None } else { Some(p0) };
    };
    let next_default = match c.ch {
        Ch::EsPos | Ch::EsVel => p0 + jitter(m.key, c.ch, c.k, 100),
        Ch::EsIdent => p0 + jitter(m.key, c.ch, c.k, 200),
        Ch::EsDf11 => p0 + jitter(m.key, c.ch, c.k, 200),
        Ch::EsSurv5 | Ch::EsSurv6 => p0 + jitter(m.key, c.ch, c.k, 40),
        Ch::EsAcas => p0 + jitter(m.key, c.ch, c.k, 1_500),
        Ch::Uat | Ch::Rid => p0 + jitter(m.key, c.ch, c.k, 60),
        _ => p0,
    };
    let next = match c.ch {
        Ch::EsPos if kin.on_ground && kin.gs_mps < 1.0 => 5_000 + jitter(m.key, c.ch, c.k, 100),
        Ch::AisPos => {
            if m.equip.ais == Ais::B {
                if kin.gs_mps / KT > 2.0 { 30_000 } else { 180_000 }
            } else {
                class_a_interval_ms(&kin, m.plan.turn_dps(t_s))
            }
        }
        Ch::AisB24 => {
            if c.k.is_multiple_of(2) {
                1_000
            } else {
                359_000
            }
        }
        _ => next_default,
    };
    if !env.in_region(kin.p) || m.silent(t_s) {
        return Some(next);
    }
    let medium = match c.ch {
        Ch::Uat => "uat",
        Ch::Rid => "rid",
        Ch::AisPos | Ch::AisStatic | Ch::AisB24 | Ch::AisB19 => {
            if m.equip.ais == Ais::B {
                "ais_b"
            } else {
                "ais_a"
            }
        }
        _ if m.equip.adsb == Adsb::ModeS => "mode_s",
        _ => "adsb_1090",
    };
    *env.stats.transmitted.entry(medium).or_insert(0) += 1;
    let draw = rng::hash(env.seed, &[m.key, c.ch as u64, c.k]);
    match c.ch {
        Ch::AisPos | Ch::AisStatic | Ch::AisB24 | Ch::AisB19 => emit_ais(m, c, &kin, t_ms, draw, env, out),
        Ch::Rid => emit_rid(m, c, &kin, t_ms, draw, env, out),
        _ => emit_sbs(m, c, &kin, t_ms, draw, env, out),
    }
    Some(next)
}

fn order(src: u64, t_ms: i64, m: &Mover, c: &Chan, part: u8) -> (u64, i64, u64, u16, u64, u8) {
    (src, t_ms, m.key, c.ch as u16, c.k, part)
}

fn emit_sbs(m: &Mover, c: &Chan, kin: &Kin, t_ms: i64, draw: u64, env: &mut Env, out: &mut Vec<Out>) {
    let t_s = t_ms as f64 / 1000.0;
    let alt_ft = sbs::alt_25ft((kin.alt_m - env.qnh_off_m) / FT);
    let n1 = rng::normal(rng::hash(draw, &[11]));
    let n2 = rng::normal(rng::hash(draw, &[12]));
    let gs = (kin.gs_mps / KT + 0.3 * n1).max(0.0);
    let track = geo::wrap360(kin.track_deg + 0.3 * n2);
    let vr = sbs::vr_64fpm(kin.vs_mps / FT * 60.0 + 30.0 * n1);
    let ground = Some(kin.on_ground);
    let squawk = m.ident.squawk;
    let callsign = if m.equip.anonymous { None } else { m.ident.callsign.as_deref().map(sbs::pad_callsign) };
    let pos = broadcast(m, kin, t_s, c.k, c.ch);
    let base = SbsMsg {
        icao: m.ident.icao24.unwrap_or(0),
        non_icao: m.equip.anonymous,
        t_gen_ms: env.start_ms + t_ms,
        ground,
        ..Default::default()
    };
    let mut msgs: Vec<SbsMsg> = Vec::new();
    let position = |b: &SbsMsg| {
        if kin.on_ground {
            SbsMsg { msg_type: 2, gs_kt: Some(gs), track_deg: Some(track), lat: Some(pos.lat), lon: Some(pos.lon), ..b.clone() }
        } else {
            SbsMsg { msg_type: 3, alt_ft: Some(alt_ft), lat: Some(pos.lat), lon: Some(pos.lon), ..b.clone() }
        }
    };
    let velocity = |b: &SbsMsg| SbsMsg { msg_type: 4, gs_kt: Some(gs), track_deg: Some(track), vr_fpm: Some(vr), ..b.clone() };
    let ident = |b: &SbsMsg| callsign.clone().map(|cs| SbsMsg { msg_type: 1, callsign: Some(cs), ..b.clone() });
    match c.ch {
        Ch::EsPos => msgs.push(position(&base)),
        Ch::EsVel if !kin.on_ground => msgs.push(velocity(&base)),
        Ch::EsIdent => msgs.extend(ident(&base)),
        Ch::EsDf11 => msgs.push(SbsMsg { msg_type: 8, ..base.clone() }),
        Ch::EsSurv5 => msgs.push(SbsMsg {
            msg_type: 5,
            alt_ft: (!kin.on_ground).then_some(alt_ft),
            alert: Some(false),
            spi: Some(false),
            ..base.clone()
        }),
        Ch::EsSurv6 => msgs.push(SbsMsg {
            msg_type: 6,
            squawk,
            alert: Some(false),
            emergency: squawk.map(|s| matches!(s, 7500 | 7600 | 7700)),
            spi: Some(false),
            ..base.clone()
        }),
        Ch::EsAcas if !kin.on_ground && kin.alt_m > 300.0 => msgs.push(SbsMsg { msg_type: 7, alt_ft: Some(alt_ft), ..base.clone() }),
        Ch::Uat => {
            msgs.push(position(&base));
            if !kin.on_ground {
                msgs.push(velocity(&base));
            }
            if c.k.is_multiple_of(4) {
                msgs.extend(ident(&base));
            }
        }
        _ => {}
    }
    if msgs.is_empty() {
        return;
    }
    let kind = if c.ch == Ch::Uat { Kind::AdsbUat } else { Kind::Adsb1090 };
    for ri in 0..env.rx.len() {
        if env.rx[ri].kind != kind {
            continue;
        }
        match env.hear(ri, kin.p, kin.alt_m.max(2.0), t_s, draw, f64::INFINITY, 4.0, t_ms) {
            Err(l) => env.lose(l),
            Ok(_) => {
                let rx = &env.rx[ri];
                let lat = if rx.network {
                    let [a, b] = rx.delay_ms;
                    a + (rng::hash(draw, &[rx.index as u64, 3]) % ((b - a).max(0) as u64 + 1)) as i64
                } else {
                    let [a, b] = rx.latency_ms;
                    a + (rng::hash(draw, &[rx.index as u64, 3]) % ((b - a).max(0) as u64 + 1)) as i64
                };
                let deliver = env.start_ms + t_ms + lat;
                let (num, id, ladd_filter, idx) = (rx.num, rx.id.clone(), rx.ladd, rx.index);
                for (part, msg) in msgs.iter().enumerate() {
                    if ladd_filter && m.equip.ladd && msg.msg_type == 1 {
                        *env.stats.lost.entry("ladd_blocked").or_insert(0) += 1;
                        continue;
                    }
                    let mut s = msg.clone();
                    s.session = num;
                    s.t_log_ms = deliver;
                    out.push(Out {
                        deliver_ms: deliver,
                        order: order(idx as u64, t_ms, m, c, part as u8),
                        src: id.clone(),
                        kind,
                        body: Body::Sbs(s),
                    });
                }
                env.count(&id, kind);
            }
        }
    }
}

fn emit_ais(m: &Mover, c: &Chan, kin: &Kin, t_ms: i64, draw: u64, env: &mut Env, out: &mut Vec<Out>) {
    let t_s = t_ms as f64 / 1000.0;
    let abs_ms = env.start_ms + t_ms;
    let id = &m.ident;
    let mmsi = id.mmsi.unwrap_or(0);
    let pos = broadcast(m, kin, t_s, c.k, c.ch);
    let n1 = rng::normal(rng::hash(draw, &[21]));
    let sog_kn = (kin.gs_mps / KT + 0.05 * n1).max(0.0);
    let moving = sog_kn >= 0.2;
    let cog = moving.then(|| geo::wrap360(kin.track_deg + 0.5 * n1));
    let crab = (rng::unit(rng::hash(m.key, &[0xC4AB])) - 0.5) * 6.0;
    let heading = if m.equip.gyro {
        let h = if kin.held { kin.track_deg } else { kin.track_deg + crab };
        (geo::wrap360(h).round() as u16) % 360
    } else {
        511
    };
    let [bow, stern, port, stbd] = id.dims;
    let name = id.name.clone().unwrap_or_default();
    let callsign = id.ship_callsign.clone().unwrap_or_default();
    let channel = if c.k.is_multiple_of(2) { 'A' } else { 'B' };
    let mut bits = Vec::new();
    match (c.ch, m.equip.ais) {
        (Ch::AisPos, Ais::A) => {
            let ty = m.equip.pos_type;
            let rot = m.equip.rot_indicator.then(|| m.plan.turn_dps(t_s) * 60.0);
            let slot = ((abs_ms.rem_euclid(60_000)) * 2250 / 60_000) as u16;
            let interval_slots = (class_a_interval_ms(kin, m.plan.turn_dps(t_s)) * 2250 / 60_000) as u16;
            let (h, mi) = (((abs_ms / 3_600_000) % 24) as u8, ((abs_ms / 60_000) % 60) as u8);
            // stations heard, as the transponder would count them: from the channel load
            let stations = env.ais_load.get(&(0, t_ms.div_euclid(LOAD_BUCKET_MS))).map_or(20, |l| (l * 4500.0 / 6.0) as u16);
            let radio = if ty == 3 {
                ais::itdma(interval_slots.max(1), 0, false)
            } else {
                ais::sotdma(7 - (c.k % 8) as u8, slot, stations, h, mi, interval_slots)
            };
            bits.push(
                PositionA {
                    msg_type: ty,
                    repeat: 0,
                    mmsi,
                    status: kin.status,
                    rot: ais::rot_field(rot),
                    sog: ais::sog_field(Some(sog_kn)),
                    accuracy: m.equip.dgnss,
                    lon: Some(pos.lon),
                    lat: Some(pos.lat),
                    cog: ais::cog_field(cog),
                    heading,
                    second: utc_second(abs_ms),
                    maneuver: 0,
                    raim: false,
                    radio,
                }
                .bits(),
            );
        }
        (Ch::AisStatic, _) => {
            let [mo, d, h, mi] = id.eta.unwrap_or([0, 0, 24, 60]);
            bits.push(
                StaticVoyage {
                    repeat: 0,
                    mmsi,
                    ais_version: 2,
                    imo: id.imo.unwrap_or(0),
                    callsign: callsign.clone(),
                    name: name.clone(),
                    ship_type: id.ship_type,
                    to_bow: bow,
                    to_stern: stern,
                    to_port: port as u8,
                    to_starboard: stbd as u8,
                    epfd: 1,
                    eta_month: mo,
                    eta_day: d,
                    eta_hour: h,
                    eta_minute: mi,
                    draught_dm: id.draught_dm,
                    destination: id.destination.clone().unwrap_or_default(),
                    dte: false,
                }
                .bits(),
            );
        }
        (Ch::AisPos, Ais::B) => bits.push(
            PositionB {
                repeat: 0,
                mmsi,
                sog: ais::sog_field(Some(sog_kn)),
                accuracy: m.equip.dgnss,
                lon: Some(pos.lon),
                lat: Some(pos.lat),
                cog: ais::cog_field(cog),
                heading: 511,
                second: utc_second(abs_ms),
                cs: true,
                display: false,
                dsc: true,
                band: true,
                msg22: true,
                assigned: false,
                raim: false,
                radio: ais::CS_RADIO,
            }
            .bits(),
        ),
        (Ch::AisB24, _) => {
            if c.k.is_multiple_of(2) {
                bits.push(StaticA { repeat: 0, mmsi, name: name.clone() }.bits());
            } else {
                let (vendor, model, serial) = id.vendor.clone().unwrap_or_default();
                bits.push(
                    StaticB {
                        repeat: 0,
                        mmsi,
                        ship_type: id.ship_type,
                        vendor,
                        model,
                        serial,
                        callsign: callsign.clone(),
                        to_bow: bow,
                        to_stern: stern,
                        to_port: port as u8,
                        to_starboard: stbd as u8,
                        epfd: 1,
                    }
                    .bits(),
                );
            }
        }
        (Ch::AisB19, _) => bits.push(
            ExtendedB {
                repeat: 0,
                mmsi,
                regional: (0, 0),
                sog: ais::sog_field(Some(sog_kn)),
                accuracy: m.equip.dgnss,
                lon: Some(pos.lon),
                lat: Some(pos.lat),
                cog: ais::cog_field(cog),
                heading: 511,
                second: utc_second(abs_ms),
                name: name.clone(),
                ship_type: id.ship_type,
                to_bow: bow,
                to_stern: stern,
                to_port: port as u8,
                to_starboard: stbd as u8,
                epfd: 1,
                raim: false,
                dte: false,
                assigned: false,
            }
            .bits(),
        ),
        _ => {}
    }
    let Some(b) = bits.pop() else { return };
    let antenna = kin.alt_m.max(0.0) + m.class.antenna_m();
    let class_b = m.equip.ais == Ais::B;
    for ri in 0..env.rx.len() {
        if env.rx[ri].kind != Kind::Ais {
            continue;
        }
        let range_cap = if class_b { env.rx[ri].class_b_range_m } else { f64::INFINITY };
        match env.hear(ri, kin.p, antenna, t_s, draw, range_cap, 4.0, t_ms) {
            Err(l) => env.lose(l),
            Ok(_) => {
                let rx = &env.rx[ri];
                let delay = if rx.network {
                    let [a, b] = rx.delay_ms;
                    a + (rng::hash(draw, &[rx.index as u64, 3]) % ((b - a).max(0) as u64 + 1)) as i64
                } else {
                    let [a, b] = rx.latency_ms;
                    a + (rng::hash(draw, &[rx.index as u64, 3]) % ((b - a).max(0) as u64 + 1)) as i64
                };
                // the receiving station's clock stamps the tag block; a network keeps its feeder's
                let rx_ms = abs_ms + if rx.network { 30 } else { delay };
                let id_s = rx.id.clone();
                out.push(Out {
                    deliver_ms: abs_ms + delay,
                    order: order(rx.index as u64, t_ms, m, c, 0),
                    src: id_s.clone(),
                    kind: Kind::Ais,
                    body: Body::Ais { bits: b.clone(), channel, c_s: rx_ms.div_euclid(1000) },
                });
                env.count(&id_s, Kind::Ais);
            }
        }
    }
}

fn emit_rid(m: &Mover, c: &Chan, kin: &Kin, t_ms: i64, draw: u64, env: &mut Env, out: &mut Vec<Out>) {
    let Some(link) = m.equip.rid else { return };
    let t_s = t_ms as f64 / 1000.0;
    let abs_ms = env.start_ms + t_ms;
    let pos = broadcast(m, kin, t_s, c.k, c.ch);
    let id = &m.ident;
    let good = rng::unit(rng::hash(m.key, &[0xACC])) < 0.8;
    let (qnh_off_m, geoid_n_m) = (env.qnh_off_m, env.geoid_n_m);
    let msg = |rx: &str, t_rx: i64| RidMsg {
        kind: "rid",
        rx: rx.to_string(),
        t_rx,
        // the F3411 timestamp: tenths of a second
        t: abs_ms.div_euclid(100) * 100,
        uas_id: id.uas_id.clone().unwrap_or_default(),
        id_type: "serial",
        ua_type: if id.ua_type.is_empty() { "multirotor" } else { id.ua_type },
        status: if kin.on_ground { "ground" } else { "airborne" },
        lat: json::round(pos.lat, 7),
        lon: json::round(pos.lon, 7),
        // ASTM F3411 geodetic altitude: height above the WGS84 ellipsoid
        alt_geo_m: json::round(((kin.alt_m + geoid_n_m) * 2.0).round() / 2.0, 1),
        alt_baro_m: json::round(((kin.alt_m - qnh_off_m) * 2.0).round() / 2.0, 1),
        height_agl_m: json::round(((kin.alt_m - id.ground_m) * 2.0).round() / 2.0, 1),
        speed_mps: json::round((kin.gs_mps * 4.0).round() / 4.0, 2),
        track_deg: json::round(kin.track_deg.round() % 360.0, 0),
        vspeed_mps: json::round((kin.vs_mps * 2.0).round() / 2.0, 1),
        h_acc_m: if good { 3.0 } else { 10.0 },
        v_acc_m: if good { 4.0 } else { 10.0 },
        op_lat: id.op.map(|p| json::round(p.lat, 7)),
        op_lon: id.op.map(|p| json::round(p.lon, 7)),
    };
    let alt = kin.alt_m.max(0.5);
    for ri in 0..env.rx.len() {
        if env.rx[ri].kind != Kind::Rid {
            continue;
        }
        match env.hear(ri, kin.p, alt, t_s, draw, link.range_m(), 3.0, t_ms) {
            Err(l) => env.lose(l),
            Ok(_) => {
                let rx = &env.rx[ri];
                let [a, b] = rx.latency_ms;
                let lat = a + (rng::hash(draw, &[rx.index as u64, 3]) % ((b - a).max(0) as u64 + 1)) as i64;
                let (rid, idx) = (rx.id.clone(), rx.index);
                out.push(Out {
                    deliver_ms: abs_ms + lat,
                    order: order(idx as u64, t_ms, m, c, 0),
                    src: rid.clone(),
                    kind: Kind::Rid,
                    body: Body::Json(json::line(&msg(&rid, abs_ms + lat))),
                });
                env.count(&rid, Kind::Rid);
            }
        }
    }
    // own vehicles' receivers
    let range = env.own_rid_range_m.min(link.range_m());
    for o in env.own_rid {
        let g = geo::haversine_m(o.p, kin.p);
        let d = (g * g + (alt - o.alt_m).powi(2)).sqrt();
        if d > range || d < 1.0 {
            continue;
        }
        if env.terrain.line_of_sight(o.p, o.alt_m, kin.p, alt).is_err() {
            *env.stats.lost.entry("terrain").or_insert(0) += 1;
            continue;
        }
        let p_loss = 0.05 + 0.95 * (d / range).powi(3);
        if rng::unit(rng::hash(draw, &[o.order, 1])) < p_loss {
            *env.stats.lost.entry("fade").or_insert(0) += 1;
            continue;
        }
        let lat = 20 + (rng::hash(draw, &[o.order, 3]) % 60) as i64;
        out.push(Out {
            deliver_ms: abs_ms + lat,
            order: order(o.order, t_ms, m, c, 0),
            src: o.id.clone(),
            kind: Kind::Rid,
            body: Body::Json(json::line(&msg(&o.id, abs_ms + lat))),
        });
        *env.stats.received.entry(format!("own/{}", Kind::Rid.as_str())).or_insert(0) += 1;
    }
}
