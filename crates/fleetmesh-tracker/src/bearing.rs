//! Bearing-only tracks and triangulation.
//!
//! A detect-and-avoid camera gives a bearing and an elevation, no range. One such line cannot
//! place a target, so a bearing-only detection that matches no track starts a line-of-bearing
//! track: the history of its lines (sensor position, bearing, elevation, time), shown as the line
//! itself with the position at the middle of the sensor's detection range and a sigma covering
//! the whole range interval, flagged `bearing_only`.
//!
//! A position is claimed only when the lines give geometry: two sensors seeing the target, or one
//! sensor that has moved across the line of sight. Each time a line is added, the track's lines in
//! the last `tri_window_ms` are fitted with a constant-velocity target (x, y, vx, vy) by weighted
//! least squares on the bearing residuals (a pseudo-linear first guess, then Gauss-Newton, with a
//! weak zero-mean prior on velocity). The fit is accepted only when the lines' crossing angle at
//! the target, the residuals, the speed, the range and the position's 1-sigma ellipse all pass;
//! then the track becomes an ordinary position track flagged `triangulated`.

use crate::config::Config;
use crate::geo::wrap_pi;
use crate::kf::{MAXM, ellipse, invert};
use crate::wire::SensorKind;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub struct LobObs {
    pub t: i64,
    /// Sensor position, local metres.
    pub s: [f64; 3],
    pub platform: String,
    pub kind: SensorKind,
    /// Grid bearing (true bearing plus convergence), radians.
    pub beta: f64,
    /// True bearing, radians, for output.
    pub beta_true: f64,
    pub sb: f64,
    pub elev: Option<(f64, f64)>,
    /// Far end of the line: the sensor's detection range for the target's size.
    pub r_max: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lob {
    pub obs: VecDeque<LobObs>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    /// x, y, vx, vy at `t`.
    pub x: [f64; 4],
    pub c: [[f64; 4]; 4],
    pub t: i64,
    /// Altitude from the elevations, and its sigma.
    pub z: Option<(f64, f64)>,
    pub sigma_major: f64,
    pub range: f64,
}

/// Minimum range along a line, metres.
pub const R_MIN: f64 = 20.0;

/// A predicted bearing and its variance, and the elevation likewise when known.
pub type AnglePrediction = ((f64, f64), Option<(f64, f64)>);

impl Lob {
    pub fn latest(&self) -> Option<&LobObs> {
        self.obs.back()
    }

    pub fn push(&mut self, o: LobObs, window_ms: i64) {
        let t = o.t;
        self.obs.push_back(o);
        while let Some(f) = self.obs.front() {
            if t - f.t > window_ms || self.obs.len() > 64 {
                self.obs.pop_front();
            } else {
                break;
            }
        }
    }

    /// Predicted bearing (and elevation) from the same platform at `t`: a straight line through
    /// its last few lines. Returns ((bearing, variance), Option<(elevation, variance)>).
    pub fn predict_same(
        &self,
        platform: &str,
        kind: SensorKind,
        t: i64,
    ) -> Option<AnglePrediction> {
        let mine: Vec<&LobObs> = self
            .obs
            .iter()
            .rev()
            .filter(|o| {
                o.platform == platform && o.kind == kind && t - o.t <= 10_000 && o.t <= t + 2_000
            })
            .take(6)
            .collect();
        let last = *mine.first()?;
        let dt = (t - last.t) as f64 / 1000.0;
        // Rate from a fit over the recent lines (unwrapped about the last bearing).
        let (rate, rate_var) = if mine.len() >= 2 {
            let pts: Vec<(f64, f64)> = mine
                .iter()
                .map(|o| ((o.t - last.t) as f64 / 1000.0, wrap_pi(o.beta - last.beta)))
                .collect();
            let n = pts.len() as f64;
            let mt = pts.iter().map(|p| p.0).sum::<f64>() / n;
            let mb = pts.iter().map(|p| p.1).sum::<f64>() / n;
            let stt: f64 = pts.iter().map(|p| (p.0 - mt).powi(2)).sum();
            if stt > 0.04 {
                let b = pts.iter().map(|p| (p.0 - mt) * (p.1 - mb)).sum::<f64>() / stt;
                (b, last.sb * last.sb / stt + 1.5f64.to_radians().powi(2))
            } else {
                (0.0, 6.0f64.to_radians().powi(2))
            }
        } else {
            (0.0, 6.0f64.to_radians().powi(2))
        };
        let b = last.beta + rate * dt;
        let bv = last.sb * last.sb + rate_var * dt * dt;
        let e = last
            .elev
            .map(|(e, se)| (e, se * se + (2.0f64.to_radians() * dt).powi(2)));
        Some(((b, bv), e))
    }

    /// Whether a line from a different platform crosses this track's latest other-platform line
    /// inside both detection ranges, with enough crossing angle and consistent elevations.
    /// Returns a cost (lower is better).
    pub fn cross_cost(&self, o: &LobObs, cfg: &Config) -> Option<f64> {
        let other = self.obs.iter().rev().find(|p| {
            (p.platform != o.platform || p.kind != o.kind) && (o.t - p.t).abs() <= 2_000
        })?;
        let (ra, rb) = intersect(&other.s, other.beta, &o.s, o.beta)?;
        let cross = wrap_pi(other.beta - o.beta).abs();
        let cross = cross.min(std::f64::consts::PI - cross);
        if cross < cfg.tri_min_cross_deg.to_radians() {
            return None;
        }
        if ra < R_MIN || rb < R_MIN || ra > 1.3 * other.r_max || rb > 1.3 * o.r_max {
            return None;
        }
        let mut cost = 2.0 + (o.sb * o.sb).ln();
        if let (Some((ea, sa)), Some((eb, sb))) = (other.elev, o.elev) {
            let za = other.s[2] + ra * ea.tan();
            let zb = o.s[2] + rb * eb.tan();
            let var = (ra * sa).powi(2) + (rb * sb).powi(2) + 30.0f64.powi(2);
            let d2 = (za - zb).powi(2) / var;
            if d2 > 9.0 {
                return None;
            }
            cost += d2;
        }
        Some(cost)
    }

    /// Fit a constant-velocity target to the lines. None when the geometry does not fix it.
    pub fn fit(&self, cfg: &Config) -> Option<Fit> {
        let n = self.obs.len();
        if n < cfg.tri_min_obs.max(3) {
            return None;
        }
        let last = self.obs.back()?;
        let t_ref = last.t;
        let tau = |o: &LobObs| (o.t - t_ref) as f64 / 1000.0;
        let sv = cfg.tri_max_speed_mps / 2.0;
        let prior = 1.0 / (sv * sv);

        // Pseudo-linear first guess: each line says (p - s) . (cos b, -sin b) = 0.
        let r_guess = (last.r_max / 2.0).max(100.0);
        let mut a = [[0.0; MAXM]; MAXM];
        let mut g = [0.0; 4];
        for o in &self.obs {
            let (sb, cb) = o.beta.sin_cos();
            let tt = tau(o);
            let row = [cb, -sb, tt * cb, -tt * sb];
            let rhs = o.s[0] * cb - o.s[1] * sb;
            let w = 1.0 / (r_guess * o.sb).powi(2);
            for i in 0..4 {
                g[i] += w * row[i] * rhs;
                for j in 0..4 {
                    a[i][j] += w * row[i] * row[j];
                }
            }
        }
        a[2][2] += prior;
        a[3][3] += prior;
        // A weak pull to the middle of the latest line keeps the guess in front of the sensor.
        let mid = [
            last.s[0] + r_guess * last.beta.sin(),
            last.s[1] + r_guess * last.beta.cos(),
        ];
        let wp = 1.0 / (2.0 * r_guess).powi(2);
        a[0][0] += wp;
        a[1][1] += wp;
        g[0] += wp * mid[0];
        g[1] += wp * mid[1];
        let (ai, _) = invert(&a, 4)?;
        let mut th = [0.0; 4];
        for i in 0..4 {
            th[i] = (0..4).map(|j| ai[i][j] * g[j]).sum();
        }

        // Gauss-Newton on the bearing residuals.
        let mut cov = [[0.0; 4]; 4];
        let mut chi2 = 0.0;
        for _ in 0..8 {
            let mut nm = [[0.0; MAXM]; MAXM];
            let mut gr = [0.0; 4];
            chi2 = 0.0;
            for o in &self.obs {
                let tt = tau(o);
                let dx = th[0] + th[2] * tt - o.s[0];
                let dy = th[1] + th[3] * tt - o.s[1];
                let rho2 = (dx * dx + dy * dy).max(1.0);
                let res = wrap_pi(o.beta - dx.atan2(dy));
                let j = [dy / rho2, -dx / rho2, tt * dy / rho2, -tt * dx / rho2];
                let w = 1.0 / (o.sb * o.sb);
                chi2 += res * res * w;
                for i in 0..4 {
                    gr[i] += w * j[i] * res;
                    for k in 0..4 {
                        nm[i][k] += w * j[i] * j[k];
                    }
                }
            }
            nm[2][2] += prior;
            nm[3][3] += prior;
            gr[2] -= prior * th[2];
            gr[3] -= prior * th[3];
            let (ni, _) = invert(&nm, 4)?;
            let mut step = [0.0; 4];
            for i in 0..4 {
                step[i] = (0..4).map(|k| ni[i][k] * gr[k]).sum();
            }
            for i in 0..4 {
                th[i] += step[i];
                for k in 0..4 {
                    cov[i][k] = ni[i][k];
                }
            }
            if step[0].abs() + step[1].abs() < 0.05 && step[2].abs() + step[3].abs() < 0.01 {
                break;
            }
        }
        if th.iter().any(|v| !v.is_finite()) {
            return None;
        }

        // Every line must point at the solution (not away from it), inside its range.
        let mut min_dir = f64::INFINITY;
        let mut max_dir = f64::NEG_INFINITY;
        let mut ref_dir = None;
        for o in &self.obs {
            let tt = tau(o);
            let dx = th[0] + th[2] * tt - o.s[0];
            let dy = th[1] + th[3] * tt - o.s[1];
            let rho = (dx * dx + dy * dy).sqrt();
            if rho < R_MIN || rho > 1.3 * o.r_max {
                return None;
            }
            if wrap_pi(o.beta - dx.atan2(dy)).abs() > 4.0 * o.sb + 2.0f64.to_radians() {
                return None;
            }
            // Direction from the target to the sensor: the spread is the crossing angle.
            let d = (-dx).atan2(-dy);
            let r0 = *ref_dir.get_or_insert(d);
            let rel = wrap_pi(d - r0);
            min_dir = min_dir.min(rel);
            max_dir = max_dir.max(rel);
        }
        if max_dir - min_dir < cfg.tri_min_cross_deg.to_radians() {
            return None;
        }
        // Observability: a constant-velocity target is fixed by bearings only when the sensors'
        // positions depart from one straight, constant-speed path (two sensors, or one that
        // turned). The departure must be a fair fraction of the range.
        let dx = th[0] - last.s[0];
        let dy = th[1] - last.s[1];
        let range = (dx * dx + dy * dy).sqrt();
        if self.sensor_spread() < cfg.tri_min_cross_deg.to_radians().tan() * range {
            return None;
        }
        let dof = (n as f64 - 4.0).max(1.0);
        if chi2 > dof + 3.0 * (2.0 * dof).sqrt() + 4.0 {
            return None;
        }
        let speed = (th[2] * th[2] + th[3] * th[3]).sqrt();
        if speed > cfg.tri_max_speed_mps {
            return None;
        }
        // Profile test: the linearised covariance is overconfident when the range is weakly
        // observed (a short, biased range looks well fixed). Require the fitted range to beat a
        // range 40% shorter and one 60% longer on the same line, velocity refitted at each,
        // unless that range is physically excluded (inside R_MIN, or past the detection range).
        let chi2_min = chi2 + prior * (th[2] * th[2] + th[3] * th[3]);
        for frac in [0.6, 1.6] {
            if range * frac < R_MIN || range * frac > 1.3 * last.r_max {
                continue;
            }
            let c = self.profile(&th, frac, prior, t_ref)?;
            if c - chi2_min < cfg.tri_profile_dchi2 {
                return None;
            }
        }
        // The linearised covariance of a bearings-only fit is optimistic; report it doubled
        // (sigma times 1.4) as an allowance for the nonlinearity.
        for row in cov.iter_mut() {
            for v in row.iter_mut() {
                *v *= 2.0;
            }
        }
        let (major, _, _) = ellipse(cov[0][0], cov[0][1], cov[1][1]);
        if major > cfg.tri_max_sigma_abs_m.max(cfg.tri_max_sigma_rel * range) {
            return None;
        }

        // Altitude from the elevations at the fitted ranges.
        let mut sw = 0.0;
        let mut sz = 0.0;
        for o in &self.obs {
            if let Some((e, se)) = o.elev {
                let tt = tau(o);
                let ddx = th[0] + th[2] * tt - o.s[0];
                let ddy = th[1] + th[3] * tt - o.s[1];
                let rho = (ddx * ddx + ddy * ddy).sqrt();
                let z = o.s[2] + rho * e.tan();
                let c2 = e.cos().powi(2).max(1e-3);
                let var = (rho * se / c2).powi(2) + (e.tan() * major).powi(2) + 4.0;
                sw += 1.0 / var;
                sz += z / var;
            }
        }
        let z = if sw > 0.0 {
            Some((sz / sw, (1.0 / sw).sqrt()))
        } else {
            None
        };
        Some(Fit {
            x: th,
            c: cov,
            t: t_ref,
            z,
            sigma_major: major,
            range,
        })
    }
}

impl Lob {
    /// Chi-square (with the velocity prior) of the best fit whose position at `t_ref` is the
    /// fitted one scaled by `frac` along the line from the latest sensor position.
    fn profile(&self, th: &[f64; 4], frac: f64, prior: f64, t_ref: i64) -> Option<f64> {
        let last = self.obs.back()?;
        let p = [
            last.s[0] + frac * (th[0] - last.s[0]),
            last.s[1] + frac * (th[1] - last.s[1]),
        ];
        let mut v = [th[2] * frac, th[3] * frac];
        let eval = |v: &[f64; 2]| -> (f64, [[f64; MAXM]; MAXM], [f64; 2]) {
            let mut a = [[0.0; MAXM]; MAXM];
            let mut g = [0.0; 2];
            let mut chi2 = 0.0;
            for o in &self.obs {
                let tt = (o.t - t_ref) as f64 / 1000.0;
                let dx = p[0] + v[0] * tt - o.s[0];
                let dy = p[1] + v[1] * tt - o.s[1];
                let rho2 = (dx * dx + dy * dy).max(1.0);
                let res = wrap_pi(o.beta - dx.atan2(dy));
                let j = [tt * dy / rho2, -tt * dx / rho2];
                let w = 1.0 / (o.sb * o.sb);
                chi2 += res * res * w;
                for i in 0..2 {
                    g[i] += w * j[i] * res;
                    for k in 0..2 {
                        a[i][k] += w * j[i] * j[k];
                    }
                }
            }
            a[0][0] += prior;
            a[1][1] += prior;
            g[0] -= prior * v[0];
            g[1] -= prior * v[1];
            (chi2 + prior * (v[0] * v[0] + v[1] * v[1]), a, g)
        };
        for _ in 0..8 {
            let (_, a, g) = eval(&v);
            let (ai, _) = invert(&a, 2)?;
            let step = [
                ai[0][0] * g[0] + ai[0][1] * g[1],
                ai[1][0] * g[0] + ai[1][1] * g[1],
            ];
            v[0] += step[0];
            v[1] += step[1];
            if step[0].abs() + step[1].abs() < 0.01 {
                break;
            }
        }
        Some(eval(&v).0)
    }

    /// How far the sensors' positions depart from a single constant-velocity path: the largest
    /// residual of a straight-line fit of position against time, metres.
    pub fn sensor_spread(&self) -> f64 {
        let n = self.obs.len() as f64;
        if n < 2.0 {
            return 0.0;
        }
        let t0 = self.obs[0].t;
        let ts: Vec<f64> = self
            .obs
            .iter()
            .map(|o| (o.t - t0) as f64 / 1000.0)
            .collect();
        let mt = ts.iter().sum::<f64>() / n;
        let stt: f64 = ts.iter().map(|t| (t - mt).powi(2)).sum();
        let mut worst: f64 = 0.0;
        let mut fit = [(0.0, 0.0); 2];
        for (axis, f) in fit.iter_mut().enumerate() {
            let m = self.obs.iter().map(|o| o.s[axis]).sum::<f64>() / n;
            let b = if stt > 1e-9 {
                self.obs
                    .iter()
                    .zip(&ts)
                    .map(|(o, t)| (t - mt) * (o.s[axis] - m))
                    .sum::<f64>()
                    / stt
            } else {
                0.0
            };
            *f = (m, b);
        }
        for (o, t) in self.obs.iter().zip(&ts) {
            let ex = o.s[0] - (fit[0].0 + fit[0].1 * (t - mt));
            let ey = o.s[1] - (fit[1].0 + fit[1].1 * (t - mt));
            worst = worst.max((ex * ex + ey * ey).sqrt());
        }
        worst
    }
}

/// Ranges along two rays (origin, grid bearing) to their intersection, if they meet ahead of
/// both.
pub fn intersect(a: &[f64; 3], ba: f64, b: &[f64; 3], bb: f64) -> Option<(f64, f64)> {
    let (da0, da1) = (ba.sin(), ba.cos());
    let (db0, db1) = (bb.sin(), bb.cos());
    // a + ra*da = b + rb*db  ->  ra*da - rb*db = b - a
    let det = da0 * (-db1) - (-db0) * da1;
    if det.abs() < 1e-6 {
        return None;
    }
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let ra = (ex * (-db1) - (-db0) * ey) / det;
    let rb = (da0 * ey - da1 * ex) / det;
    if ra <= 0.0 || rb <= 0.0 {
        None
    } else {
        Some((ra, rb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_meet() {
        // From (0,0) looking north-east and from (1000,0) looking north-west: meet at (500,500).
        let (ra, rb) = intersect(
            &[0.0, 0.0, 0.0],
            45f64.to_radians(),
            &[1000.0, 0.0, 0.0],
            -45f64.to_radians(),
        )
        .expect("meet");
        assert!((ra - 707.1).abs() < 0.2 && (rb - 707.1).abs() < 0.2);
        assert!(intersect(&[0.0, 0.0, 0.0], 0.0, &[1000.0, 0.0, 0.0], 0.0).is_none());
    }
}
