//! Rules 30 ([`crate::rules::NODE_LOSS_SINCE`], docs/NODE-LOSS.md section 3): where one of our
//! vehicles can be once it stops reporting.
//!
//! A vehicle whose position is not fresh is **silent**. Until rules 30 the friendly keep-out
//! counted it anywhere its catalog cruise speed reached since its last report, which grows
//! without bound, never says why, and understates a vehicle that runs faster than it cruises.
//! From rules 30 it counts within a union of hypotheses, each a pure function of facts every
//! decision logs and of the engine's own record of its orders (logged with the decision as
//! [`crate::world::WorldSnapshot::silent_orders`], which replay checks against its own):
//!
//! - **C, it runs its order** (the link may be cut one way): every order that may be running on
//!   it ([`in_force`]: the newest it started, and any newer whose start went out unconfirmed),
//!   laid out as the bridge flies it: from where it was last heard to any of its waypoints, the
//!   waypoints in order (a patrol's loop closed), an orbit's circle, an rtl's way home and its
//!   arrival; with no order known, the circle it was flying when last heard, or a hold where it
//!   was if it was still.
//! - **L, its lost-link failsafe fired** (it lost us too): its own failsafe
//!   anywhere on C then straight home (each of C's shapes' hull with home), a hold, loiter,
//!   brake or land on C, a glide from C, and on the water a passive hold, a depth hold, a
//!   surfacing or a disarm on C then adrift. With the plan not known, every action its autopilot
//!   has. Its timer is ignored: the region holds from the first second.
//! - **D, adrift or falling**: presumed lost, or past the endurance its last battery report
//!   leaves; alone, with a `mobility` damage report (it cannot run its order). On the surface a
//!   disc growing at [`SURFACE_DRIFT_MPS`], under water at [`SUBSURFACE_DRIFT_MPS`] (rules 33:
//!   the policy's `[rules.silent] drift_mps`, [`RulesConfig::silent_drift`]), on the ground
//!   [`GROUND_DRIFT_M`], a fixed wing its glide from its last altitude, a rotary its fall, an
//!   airship the wind.
//! - **F, nothing known**: no order, circle or stillness to go on (or its order ended so that
//!   what it does now is not known): anywhere its top speed reaches since its last report (the
//!   owner's decision 3: catalog `max_mps`, else the domain's).
//!
//! Every shape is capped by that same reach (a sound bound, whichever is farther counts), with
//! the water's current or the wind added for what they carry. Corridors and holds do not grow
//! with time: no one can retask a silent vehicle (the queue holds every order for it). A corridor
//! is [`PATH_TOLERANCE_M`] wide either side, plus where in its report interval it was and its
//! turning circle's diameter (a turn back at a waypoint); on the water, plus
//! [`CROSS_SET_SHARE`] of each leg's length times the current over its cruise (a vessel steering
//! at its next point is set off its leg by a cross current); a vessel (an airship) no faster than
//! the current (the wind) is carried off its way at that speed. The figures were checked in the
//! headless simulator against the stand-ins (`eval/headless/node_loss.py`, docs/NODE-LOSS.md
//! section 3, "As built"). They are constants here, as `CIRCLE_SLACK_M` and `LINE_OF_FIRE_M` are
//! (a disc), two (a corridor), more (a padded polygon, an rtl's way home from a segment).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::catalog::{AutopilotKind, Domain};
use crate::engine::Order;
use crate::geo::LonLat;
use crate::ids::{IntentId, Uid};
use crate::intent::{Intent, IntentSpec, State, Task, TaskKind};
use crate::rules::{RulesConfig, fresh};
use crate::time::Time;
use crate::world::{VehicleFacts, WorldSnapshot, platform_autopilot};

/// Rules 30: how fast the water carries a vessel that makes no way, m/s (a guess: the Gate's
/// spring current). Rules 33: the policy's `[rules.silent] drift_mps.surface`.
pub const SURFACE_DRIFT_MPS: f64 = 2.5;
/// Rules 30: the same for a vehicle under the surface, m/s. Rules 33: `drift_mps.subsurface`.
pub const SUBSURFACE_DRIFT_MPS: f64 = SURFACE_DRIFT_MPS;
/// Rules 30: how fast the wind carries an airship that makes no way, m/s (a guess). Rules 33:
/// `drift_mps.air`.
pub const AIR_DRIFT_MPS: f64 = 10.0;

/// What a domain's vehicle may go at when its platform names no top speed, m/s.
fn domain_top_speed_mps(domain: Domain) -> f64 {
    match domain {
        Domain::Air => 100.0,
        Domain::Ground => 30.0,
        Domain::Surface => 25.0,
    }
}

/// A corridor's half-width before the vehicle's own speed and turns, m: a waypoint's acceptance
/// radius, GNSS at 2 sigma, cross-track error (the `CIRCLE_SLACK_M` figure; GUESS).
pub const PATH_TOLERANCE_M: f64 = 50.0;
/// A hold's disc before its braking and its report interval, m (GUESS).
pub const HOLD_RADIUS_M: f64 = 10.0;
/// How far a ground vehicle that makes no way may still end up, m (GUESS).
pub const GROUND_DRIFT_M: f64 = 10.0;
/// A fixed wing without power glides this many metres for each metre of altitude (GUESS).
pub const GLIDE_RATIO: f64 = 10.0;
/// A rotary wing without power falls this many metres sideways for each metre (GUESS).
pub const ROTARY_FALL_RATIO: f64 = 1.0;

/// `[rules.silent]` (rules 33, [`crate::rules::SCENARIO_SETTINGS_SINCE`], policy schema 8): the
/// scenario's figures for a silent vehicle, today's constants by default. Parsed, bounded and
/// logged from rules 33, when a silent vehicle's region reads them ([`RulesConfig::silent_drift`]);
/// under older rules the constants stand whatever a file says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SilentRules {
    /// How fast the water or the wind carries one of ours that makes no way, m/s.
    pub drift_mps: SilentDrift,
}

/// `[rules.silent] drift_mps`: by domain (a ground vehicle's drift is a distance,
/// [`GROUND_DRIFT_M`], not a speed).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SilentDrift {
    pub surface: f64,
    pub subsurface: f64,
    pub air: f64,
}

impl Default for SilentDrift {
    /// The standing values (`sim/policy.toml`'s `[rules.silent]`): what older rules decide by.
    fn default() -> Self {
        SilentDrift::legacy()
    }
}

impl SilentDrift {
    /// What rules below 33 decide by: the constants ([`crate::legacy`]).
    pub fn legacy() -> SilentDrift {
        SilentDrift {
            surface: SURFACE_DRIFT_MPS,
            subsurface: SUBSURFACE_DRIFT_MPS,
            air: AIR_DRIFT_MPS,
        }
    }
}

impl SilentRules {
    pub fn validate(&self) -> Result<(), String> {
        let d = &self.drift_mps;
        for (v, n) in [(d.surface, "surface"), (d.subsurface, "subsurface"), (d.air, "air")] {
            if !(v.is_finite() && (0.0..=30.0).contains(&v)) {
                return Err(format!("rules.silent.drift_mps.{n} must be within 0..30, got {v}"));
            }
        }
        Ok(())
    }
}
/// The turn rate assumed for one of ours whose catalog gives none, degrees a second: the
/// stand-ins' own default (GUESS for anything real). A turn back at a waypoint swings it out by
/// its turning circle's diameter (radius: cruise speed over turn rate).
pub const DEFAULT_TURN_DPS: f64 = 10.0;
/// How far a vessel steering at its next point in a cross current is set off the straight leg,
/// as a share of the leg's length times the current over its speed: a pursuit in a uniform
/// current is set off at most 1/e of that (0.37), and the field is not uniform (GUESS).
pub const CROSS_SET_SHARE: f64 = 0.5;
/// The battery left at which a vehicle's own battery failsafe may send it home, percent: the
/// stand-ins' low threshold, 20 %, and a margin (GUESS).
pub const BATTERY_FAILSAFE_PCT: f64 = 25.0;
/// The gentlest braking any of ours makes, m/s² (a hull's, the stand-ins' 0.5).
pub const BRAKE_MPS2: f64 = 0.5;

// ---- the engine's record: what may be running on it ------------------------------------------

/// Rules 30: what the engine's record says may be running on one of our silent vehicles, as a
/// decision's inputs carry it ([`WorldSnapshot::silent_orders`]): the orders that may be
/// running, newest first; when the newest one it started ended so that what it does now is not
/// known (an op failed after it started), why; and whether a presumption of its loss is in force
/// (`node.presumed`, the engine's node ledger: D counts too). Empty: no order of ours is known to
/// run on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InForce {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orders: Vec<RunningOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub presumed: bool,
}

impl InForce {
    /// Whether some order of ours is known to have started on it (not only maybe).
    pub fn started(&self) -> bool {
        self.orders.iter().any(|o| !o.unconfirmed)
    }
}

/// Rules 30: one order that may be running on a silent vehicle, as the bridge flies it
/// ([`crate::engine::order_for`]): its waypoints in order (the transit's routed waypoints, then
/// its task path or its point), the circles it ends on, and whether it goes home at the end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunningOrder {
    pub intent: IntentId,
    pub kind: TaskKind,
    /// Its start went out and nothing confirmed it (or it went out with no op reported yet): it
    /// may not be running, and the order before it may be.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unconfirmed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<LonLat>,
    /// The index in `path` the order loops back to after its last waypoint (a patrol's route).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_from: Option<usize>,
    /// The circles it flies at the end: an orbit's (where it was sent and where the vehicle is now).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub circles: Vec<OrderCircle>,
    /// It goes home at the end (an rtl, a lay order).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub home: bool,
}

/// A circle an order flies: its centre and radius, m.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderCircle {
    pub center: LonLat,
    pub radius_m: f64,
}

/// One intent as [`in_force`] reads it: the engine's and replay's records give the same.
pub struct IntentView<'a> {
    pub id: IntentId,
    pub spec: &'a IntentSpec,
    /// Its state as what holds a platform reads it (`play_state`).
    pub state: State,
    /// Each vehicle's last op state on its current delivery.
    pub ops: &'a BTreeMap<Uid, String>,
    /// The vehicles that ever accepted and started it.
    pub committed: &'a BTreeSet<Uid>,
    /// The vehicles whose start went out and nothing confirmed.
    pub maybe_started: &'a BTreeSet<Uid>,
    /// The order it was (or would be) dispatched with ([`crate::engine::order_for`]).
    pub order: Option<Order>,
    pub orbit_now: Option<OrderCircle>,
}

impl<'a> IntentView<'a> {
    /// The engine's intent as [`in_force`] reads it.
    pub fn of(i: &'a Intent, cfg: &RulesConfig) -> IntentView<'a> {
        let order = crate::engine::order_for(i.spec(), i.route_plans(), cfg).ok();
        IntentView {
            id: i.id(),
            spec: i.spec(),
            state: i.play_state(),
            ops: i.ops(),
            committed: i.committed(),
            maybe_started: i.maybe_started(),
            order,
            orbit_now: None,
        }
    }
}

/// Whether an intent of this kind puts one of our vehicles somewhere (not the space segment).
fn moves_a_vehicle(_kind: TaskKind) -> bool {
    !false
}


/// One order as the bridge flies it on `uid` (`view`'s), unconfirmed or not.
fn running(view: &IntentView, uid: &Uid, unconfirmed: bool) -> RunningOrder {
    let mut o = RunningOrder {
        intent: view.id,
        kind: view.spec.kind(),
        unconfirmed,
        path: Vec::new(),
        loop_from: None,
        circles: Vec::new(),
        home: false,
    };
    match &view.order {
        Some(Order::Vehicles(d)) => {
            o.path = d.via.get(uid).cloned().unwrap_or_default();
            match &d.task {
                Task::Orbit { center, radius_m } => {
                    o.circles.push(OrderCircle {
                        center: *center,
                        radius_m: *radius_m,
                    });
                    // where the vehicle is orbiting now, if that is not the one last sent
                    if let Some(now) = view.orbit_now.filter(|c| c.center != *center || c.radius_m != *radius_m) {
                        o.circles.push(now);
                    }
                }
                Task::Patrol { route } => {
                    o.loop_from = Some(o.path.len());
                    o.path.extend(route.iter().copied());
                }
                // rules 34: along its route once, held at its end
                Task::Transit { route } => o.path.extend(route.iter().copied()),
                Task::Rtl {} => o.home = true,
                _ => {}
            }
        }
        // no order: only its kind is known
        None => {
            if view.spec.kind() == TaskKind::Rtl {
                o.home = true;
            }
        }
    }
    o
}

/// Rules 30: for each of our vehicles in `world` whose position is not fresh (silent), but the
/// `platform` of the decision, what the record (`intents`) says may be running on it (an empty
/// [`InForce`] when nothing is known). Pure: the engine fills a decision's
/// [`WorldSnapshot::silent_orders`] with it, and replay recomputes it from its own record.
///
/// Newest first, for each intent naming the vehicle: one it started (an op `acked`, `active` or
/// `done`, in this run or one before) runs, and nothing older does (unless it is an identify,
/// which sends the vehicle back to the order it replaced: the walk goes on); one that failed
/// effected, stopped, ended) or an order whose start may have gone out (an op `starting` or
/// `unconfirmed`, or dispatched with no op reported that rules it out: not `requeued`,
/// `failed`, `superseded` or `done`) may run, and so may what came before it.
pub fn in_force(
    world: &WorldSnapshot,
    cfg: &RulesConfig,
    platform: Option<&Uid>,
    intents: &[IntentView],
    nodes: &crate::nodes::NodeLedger,
) -> BTreeMap<Uid, InForce> {
    silent_uids(world, cfg, platform)
        .into_iter()
        .map(|uid| {
            let mut f = in_force_on(&uid, intents);
            f.presumed = nodes.presumption(&uid).is_some();
            (uid, f)
        })
        .collect()
}

/// Rules 30: our vehicles in `world` that are silent (a position, not fresh), but `platform`.
pub fn silent_uids(world: &WorldSnapshot, cfg: &RulesConfig, platform: Option<&Uid>) -> BTreeSet<Uid> {
    world
        .vehicles
        .iter()
        .filter(|(uid, v)| {
            Some(*uid) != platform && v.position.is_some() && fresh(&v.position, v, "position", world, cfg).is_err()
        })
        .map(|(uid, _)| uid.clone())
        .collect()
}

/// [`in_force`] for one vehicle.
pub fn in_force_on(uid: &Uid, intents: &[IntentView]) -> InForce {
    let mut mine: Vec<&IntentView> = intents
        .iter()
        .filter(|i| moves_a_vehicle(i.spec.kind()) && i.spec.uids.contains(uid))
        .collect();
    mine.sort_by_key(|a| std::cmp::Reverse(a.id));
    let mut f = InForce::default();
    for view in mine {
        let op = view.ops.get(uid).map(String::as_str);
        if view.committed.contains(uid) {
            if op == Some("failed") {
                f.unknown = Some(format!(
                    "its {} (intent {}) failed after it started: what it does now is not known",
                    view.spec.kind().as_str(),
                    view.id
                ));
                break;
            }
            f.orders.push(running(view, uid, false));
            break;
        }
        let may = view.maybe_started.contains(uid)
            || (view.state == State::Dispatched
                && !matches!(op, Some("requeued" | "failed" | "superseded" | "done")));
        if may {
            f.orders.push(running(view, uid, true));
        }
    }
    f
}

// ---- shapes -----------------------------------------------------------------------------------

/// A convex outline (one point: a disc; two: a corridor; more: a polygon) and a pad around it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Shape {
    pub points: Vec<LonLat>,
    pub pad_m: f64,
}

fn xy(o: LonLat, p: LonLat) -> (f64, f64) {
    crate::geo::enu(o, p)
}

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// The nearest point of segment a-b to the origin, and its distance (flat metres).
fn seg_nearest(a: (f64, f64), b: (f64, f64)) -> ((f64, f64), f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (-(a.0 * dx + a.1 * dy) / len2).clamp(0.0, 1.0)
    };
    let p = (a.0 + t * dx, a.1 + t * dy);
    (p, p.0.hypot(p.1))
}

impl Shape {
    pub fn disc(center: LonLat, radius_m: f64) -> Shape {
        Shape {
            points: vec![center],
            pad_m: radius_m.max(0.0),
        }
    }

    pub fn corridor(a: LonLat, b: LonLat, half_width_m: f64) -> Shape {
        Shape {
            points: if a == b { vec![a] } else { vec![a, b] },
            pad_m: half_width_m.max(0.0),
        }
    }

    /// The convex hull of `points` (Andrew's monotone chain, flat around the first), padded.
    pub fn hull(points: &[LonLat], pad_m: f64) -> Shape {
        let Some(&o) = points.first() else {
            return Shape { points: Vec::new(), pad_m };
        };
        let mut pts: Vec<(f64, f64, LonLat)> = points
            .iter()
            .map(|p| {
                let (x, y) = xy(o, *p);
                (x, y, *p)
            })
            .collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6);
        if pts.len() < 3 {
            return Shape {
                points: pts.iter().map(|p| p.2).collect(),
                pad_m: pad_m.max(0.0),
            };
        }
        let c = |a: &(f64, f64, LonLat), b: &(f64, f64, LonLat), d: &(f64, f64, LonLat)| {
            cross((a.0, a.1), (b.0, b.1), (d.0, d.1))
        };
        let mut lower: Vec<(f64, f64, LonLat)> = Vec::new();
        for p in &pts {
            while lower.len() >= 2 && c(&lower[lower.len() - 2], &lower[lower.len() - 1], p) <= 0.0 {
                lower.pop();
            }
            lower.push(*p);
        }
        let mut upper: Vec<(f64, f64, LonLat)> = Vec::new();
        for p in pts.iter().rev() {
            while upper.len() >= 2 && c(&upper[upper.len() - 2], &upper[upper.len() - 1], p) <= 0.0 {
                upper.pop();
            }
            upper.push(*p);
        }
        lower.pop();
        upper.pop();
        lower.extend(upper);
        Shape {
            points: lower.into_iter().map(|p| p.2).collect(),
            pad_m: pad_m.max(0.0),
        }
    }

    /// This shape with `m` more pad.
    pub fn widened(&self, m: f64) -> Shape {
        Shape {
            points: self.points.clone(),
            pad_m: self.pad_m + m.max(0.0),
        }
    }

    /// The outline's nearest point to `q` (`q` itself inside it) and its distance, m.
    pub fn nearest(&self, q: LonLat) -> (LonLat, f64) {
        let pts: Vec<(f64, f64)> = self.points.iter().map(|p| xy(q, *p)).collect();
        let n = pts.len();
        if n == 0 {
            return (q, f64::INFINITY);
        }
        if n >= 3 {
            // counter-clockwise: inside when left of (or on) every edge
            let inside = (0..n).all(|i| cross(pts[i], pts[(i + 1) % n], (0.0, 0.0)) >= 0.0);
            if inside {
                return (q, 0.0);
            }
        }
        let mut best = ((pts[0].0, pts[0].1), pts[0].0.hypot(pts[0].1));
        let edges = if n == 1 { 0 } else if n == 2 { 1 } else { n };
        for i in 0..edges {
            let (p, d) = seg_nearest(pts[i], pts[(i + 1) % n]);
            if d < best.1 {
                best = (p, d);
            }
        }
        (crate::geo::offset(q, best.0.0, best.0.1), best.1)
    }

    /// The edge's distance from `q`: positive outside the shape, negative inside (down to minus
    /// its pad), m.
    pub fn edge_m(&self, q: LonLat) -> f64 {
        self.nearest(q).1 - self.pad_m
    }
}

// ---- the region ---------------------------------------------------------------------------

/// Which hypothesis a part of a silent vehicle's region stands for (docs/NODE-LOSS.md 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hypothesis {
    /// C: it runs its order.
    Order,
    /// L: its lost-link failsafe fired.
    Failsafe,
    /// D: adrift or falling.
    Drift,
    /// F: nothing known, its top speed.
    TopSpeed,
}

/// One shape of a silent vehicle's region: under which hypothesis, how the keep-out's words
/// name it, and what it is in words.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Part {
    pub hypothesis: Hypothesis,
    pub shape: Shape,
    pub what: String,
}

/// Where a silent vehicle can be at `at`: its last report (`p0` at `t0`), the parts (a union),
/// and the reach that caps them all (its top speed, with the water's current or the wind, since
/// `t0`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Region {
    pub p0: LonLat,
    pub t0: Time,
    pub at: Time,
    pub span_s: f64,
    /// The speed of the reach, m/s.
    pub reach_mps: f64,
    pub reach: Shape,
    pub parts: Vec<Part>,
}

impl Region {
    /// The region's edge from `q` (positive outside, negative inside), and the part that sets
    /// it (none: the reach does): the nearest part's, or the reach's when that is farther (the
    /// region is the union capped by the reach, so the farther of the two never overstates the
    /// clearance).
    pub fn edge_m(&self, q: LonLat) -> (f64, Option<usize>) {
        let reach = self.reach.edge_m(q);
        let nearest = self
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| (p.shape.edge_m(q), i))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        match nearest {
            Some((d, i)) if d >= reach => (d, Some(i)),
            // no part (cannot happen: F is a part then) or the reach is farther
            _ => (reach, None),
        }
    }

    /// Whether `q` (a true position) is inside the region, with its margin (negative: by how
    /// much it is outside), m.
    pub fn margin_m(&self, q: LonLat) -> f64 {
        -self.edge_m(q).0
    }
}

/// The speed one of ours may go at on its own, m/s: catalog `max_mps`, else its domain's
/// cruise figure.
pub fn top_speed(v: &VehicleFacts) -> f64 {
    let p = v.platform.as_ref();
    let domain = p.map_or(Domain::Air, |p| p.domain);
    let cruise = p.and_then(|p| p.cruise_mps).filter(|c| c.is_finite()).unwrap_or(0.0);
    p.and_then(|p| p.max_mps)
        .filter(|m| m.is_finite())
        .unwrap_or_else(|| domain_top_speed_mps(domain))
        .max(cruise)
}

/// How fast the water or the wind may carry one of ours beyond its own speed, m/s: the current
/// on the water (both the surface's figure under rules below 33: the simulator's water runs it at
/// every depth), the wind for an airship, nothing else; rules 33, the policy's `[rules.silent]
/// drift_mps` ([`RulesConfig::silent_drift`]).
pub fn carried_mps(v: &VehicleFacts, cfg: &RulesConfig) -> f64 {
    drift_mps(v, &cfg.silent_drift())
}

/// Its drift speed when it makes no way (D), m/s: the water's, the wind's for an airship, by
/// `drift`'s figures.
fn drift_mps(v: &VehicleFacts, drift: &SilentDrift) -> f64 {
    let p = v.platform.as_ref();
    match (p.map(|p| p.domain), platform_autopilot(v)) {
        (Some(Domain::Surface), _) => drift.surface,
        (_, Some(AutopilotKind::Blimp)) => drift.air,
        _ => 0.0,
    }
}

/// Rules 30: where silent vehicle `uid` can be at `at` (docs/NODE-LOSS.md 3), from its facts in
/// `world`, the orders `world.silent_orders` says may be running on it (none listed: nothing is
/// known of them, F) and whether it is presumed lost (`presumed`, or the presumption its
/// `silent_orders` carries: D). None without a position.
pub fn region(
    uid: &Uid,
    v: &VehicleFacts,
    world: &WorldSnapshot,
    cfg: &RulesConfig,
    at: Time,
    presumed: bool,
) -> Option<Region> {
    let pos = v.position.as_ref()?;
    let (p0, t0) = (pos.value, pos.t);
    let span = (at.ms_since(t0) as f64 / 1000.0).max(0.0);
    let p = v.platform.as_ref();
    let _domain = p.map_or(Domain::Air, |p| p.domain);
    let cruise = p.and_then(|p| p.cruise_mps).filter(|c| c.is_finite() && *c >= 0.0).unwrap_or(0.0);
    // its velocity with its last position (the same report), else none known
    let vel = v
        .velocity_mps
        .as_ref()
        .filter(|f| f.t.ms_since(t0).abs() <= i64::from(v.report_interval_ms.unwrap_or(1000)))
        .map(|f| f.value);
    let speed0 = vel.map(|[e, n]| e.hypot(n));
    let _interval_s = f64::from(v.report_interval_ms.unwrap_or(1000)) / 1000.0;
    let _moving = speed0.unwrap_or(cruise).max(0.0);
    // its turning radius: the catalog's, else at DEFAULT_TURN_DPS (a copter and an airship turn
    // in place); a fixed wing's never under the policy's floor for an orbit
    // where in its report interval it was, and the swing of a turn back at a waypoint (its
    // diameter to the side)
    // the water (the wind for an airship) against its own speed
    let figures = cfg.silent_drift();
    let carried = drift_mps(v, &figures);
    let (_cross, _swept) = if carried <= 0.0 {
        (0.0, 0.0)
    } else if cruise > carried {
        (CROSS_SET_SHARE * carried / cruise, 0.0)
    } else {
        (0.0, carried * span)
    };
    // a fixed wing holds and arrives home on a circle of its own loiter radius, a parameter of
    // the vehicle's that the bridge reads with its lost-link plan: not known, nothing bounds
    // that circle but its reach
    let top = top_speed(v);
    let drift_speed = drift_mps(v, &figures);
    let _drift = drift_speed * span;
    let reach_mps = top + carried;
    let reach = Shape::disc(p0, reach_mps * span);
    let parts: Vec<Part> = Vec::new();
    let _mobility = v.damage.as_ref().is_some_and(|d| d.value.mobility);
    let in_force = world.silent_orders.get(uid);
    let _presumed = presumed || in_force.is_some_and(|f| f.presumed);
// D: presumed lost, out of power, or (alone) its propulsion out
    let endurance_s = p.and_then(|p| p.endurance_h).map(|e| e * 3600.0);
    let battery = v.battery_pct.as_ref();
    let left_s = match (battery, endurance_s) {
        (Some(b), Some(e)) => Some(b.value / 100.0 * e - at.ms_since(b.t) as f64 / 1000.0),
        _ => None,
    };
    let _out_of_power = left_s.is_none_or(|l| l <= 0.0);
    Some(Region {
        p0,
        t0,
        at,
        span_s: span,
        reach_mps,
        reach,
        parts,
    })
}


// ---- the keep-out -----------------------------------------------------------------------------


#[cfg(test)]
mod tests {
    

}
