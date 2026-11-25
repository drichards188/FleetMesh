//! A constant-velocity Kalman filter in local metres: state [x, y, z, vx, vy, vz].
//!
//! Process noise is continuous white-noise acceleration, horizontal and vertical densities
//! apart; a surface track holds z at sea level. Measurements are linearised at the predicted
//! state (an extended filter for the polar sensor measurements). A measurement slightly older
//! than the filter is fused by retrodiction: the predicted measurement is taken from the state
//! carried back to the measurement's time, with the process noise of that interval added to the
//! innovation covariance (an approximation that ignores the noise's correlation with the state).

use crate::geo::wrap_pi;

pub type V6 = [f64; 6];
pub type M6 = [[f64; 6]; 6];

/// Up to four measured components.
pub const MAXM: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Kf {
    pub t_ms: i64,
    pub x: V6,
    pub p: M6,
    pub qh: f64,
    pub qv: f64,
    pub surface: bool,
}

pub fn transition(dt: f64) -> M6 {
    let mut f = ident();
    f[0][3] = dt;
    f[1][4] = dt;
    f[2][5] = dt;
    f
}

pub fn ident() -> M6 {
    let mut m = [[0.0; 6]; 6];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

pub fn process_noise(dt: f64, qh: f64, qv: f64, surface: bool) -> M6 {
    let dt = dt.abs();
    let (a, b, c) = (dt * dt * dt / 3.0, dt * dt / 2.0, dt);
    let mut q = [[0.0; 6]; 6];
    for (i, qq) in [(0, qh), (1, qh), (2, if surface { 0.0 } else { qv })] {
        q[i][i] = a * qq;
        q[i][i + 3] = b * qq;
        q[i + 3][i] = b * qq;
        q[i + 3][i + 3] = c * qq;
    }
    q
}

pub fn mat_mul(a: &M6, b: &M6) -> M6 {
    let mut c = [[0.0; 6]; 6];
    for i in 0..6 {
        for k in 0..6 {
            let aik = a[i][k];
            if aik == 0.0 {
                continue;
            }
            for j in 0..6 {
                c[i][j] += aik * b[k][j];
            }
        }
    }
    c
}

pub fn transpose(a: &M6) -> M6 {
    let mut t = [[0.0; 6]; 6];
    for i in 0..6 {
        for j in 0..6 {
            t[i][j] = a[j][i];
        }
    }
    t
}

pub fn mat_vec(a: &M6, v: &V6) -> V6 {
    let mut r = [0.0; 6];
    for i in 0..6 {
        r[i] = (0..6).map(|j| a[i][j] * v[j]).sum();
    }
    r
}

/// F P F' + Q for one step of dt seconds (dt may be negative: retrodiction).
pub fn propagate(x: &V6, p: &M6, dt: f64, qh: f64, qv: f64, surface: bool) -> (V6, M6) {
    if dt == 0.0 {
        return (*x, *p);
    }
    let f = transition(dt);
    let xn = mat_vec(&f, x);
    let mut pn = mat_mul(&mat_mul(&f, p), &transpose(&f));
    let q = process_noise(dt, qh, qv, surface);
    for i in 0..6 {
        for j in 0..6 {
            pn[i][j] += q[i][j];
        }
    }
    (xn, pn)
}

/// A linearised measurement: predicted values, Jacobian rows, noise.
#[derive(Debug, Clone, Copy)]
pub struct Lin {
    pub m: usize,
    pub z: [f64; MAXM],
    pub hx: [f64; MAXM],
    pub h: [[f64; 6]; MAXM],
    pub r: [[f64; MAXM]; MAXM],
    /// Components that are angles (residuals wrapped to +-pi).
    pub angle: [bool; MAXM],
}

impl Lin {
    pub fn new(m: usize) -> Lin {
        Lin {
            m,
            z: [0.0; MAXM],
            hx: [0.0; MAXM],
            h: [[0.0; 6]; MAXM],
            r: [[0.0; MAXM]; MAXM],
            angle: [false; MAXM],
        }
    }
}

/// An innovation, ready to gate on or to apply.
#[derive(Debug, Clone)]
pub struct Innov {
    pub m: usize,
    pub nu: [f64; MAXM],
    pub s: [[f64; MAXM]; MAXM],
    pub s_inv: [[f64; MAXM]; MAXM],
    /// Mahalanobis distance squared.
    pub d2: f64,
    pub ln_det: f64,
    h_eff: [[f64; 6]; MAXM],
    x0: V6,
    p0: M6,
    t_new: i64,
}

impl Innov {
    /// Cost for comparing candidates of the same measurement dimension: d2 + ln|S|.
    pub fn cost(&self) -> f64 {
        self.d2 + self.ln_det
    }
}

/// Invert an m x m matrix (m <= 4) by Gauss-Jordan with partial pivoting; returns the inverse
/// and ln|det|, or None when singular.
pub fn invert(a: &[[f64; MAXM]; MAXM], m: usize) -> Option<([[f64; MAXM]; MAXM], f64)> {
    let mut w = *a;
    let mut inv = [[0.0; MAXM]; MAXM];
    for (i, row) in inv.iter_mut().enumerate().take(m) {
        row[i] = 1.0;
    }
    let mut ln_det = 0.0;
    for col in 0..m {
        let mut piv = col;
        for r in col + 1..m {
            if w[r][col].abs() > w[piv][col].abs() {
                piv = r;
            }
        }
        let pv = w[piv][col];
        if !pv.is_finite() || pv.abs() < 1e-300 {
            return None;
        }
        w.swap(piv, col);
        inv.swap(piv, col);
        ln_det += pv.abs().ln();
        for j in 0..m {
            w[col][j] /= pv;
            inv[col][j] /= pv;
        }
        for r in 0..m {
            if r != col {
                let f = w[r][col];
                if f != 0.0 {
                    for j in 0..m {
                        w[r][j] -= f * w[col][j];
                        inv[r][j] -= f * inv[col][j];
                    }
                }
            }
        }
    }
    Some((inv, ln_det))
}

impl Kf {
    pub fn new(t_ms: i64, x: V6, p: M6, qh: f64, qv: f64, surface: bool) -> Kf {
        Kf {
            t_ms,
            x,
            p,
            qh,
            qv,
            surface,
        }
    }

    /// State and covariance at `t_ms` (forward or back), without changing the filter.
    pub fn at(&self, t_ms: i64) -> (V6, M6) {
        let dt = (t_ms - self.t_ms) as f64 / 1000.0;
        propagate(&self.x, &self.p, dt, self.qh, self.qv, self.surface)
    }

    pub fn predict_to(&mut self, t_ms: i64) {
        if t_ms > self.t_ms {
            let (x, p) = self.at(t_ms);
            self.x = x;
            self.p = p;
            self.t_ms = t_ms;
        }
    }

    /// The innovation of a measurement taken at `t_meas`. `model` linearises at the state it is
    /// given (the state at the measurement's time).
    pub fn innov(&self, t_meas: i64, model: impl Fn(&V6) -> Lin) -> Option<Innov> {
        let (x0, p0, h_eff, extra, t_new, lin);
        if t_meas >= self.t_ms {
            let (xp, pp) = self.at(t_meas);
            lin = model(&xp);
            x0 = xp;
            p0 = pp;
            h_eff = lin.h;
            extra = [[0.0; MAXM]; MAXM];
            t_new = t_meas;
        } else {
            let dt = (t_meas - self.t_ms) as f64 / 1000.0;
            let f = transition(dt);
            let xb = mat_vec(&f, &self.x);
            lin = model(&xb);
            // H_eff = H F(dt): the measurement as a function of the current state.
            let mut he = [[0.0; 6]; MAXM];
            for r in 0..lin.m {
                for j in 0..6 {
                    he[r][j] = (0..6).map(|k| lin.h[r][k] * f[k][j]).sum();
                }
            }
            let q = process_noise(dt, self.qh, self.qv, self.surface);
            let mut ex = [[0.0; MAXM]; MAXM];
            for a in 0..lin.m {
                for b in 0..lin.m {
                    let mut s = 0.0;
                    for i in 0..6 {
                        for j in 0..6 {
                            s += lin.h[a][i] * q[i][j] * lin.h[b][j];
                        }
                    }
                    ex[a][b] = s;
                }
            }
            x0 = self.x;
            p0 = self.p;
            h_eff = he;
            extra = ex;
            t_new = self.t_ms;
        }
        let m = lin.m;
        let mut nu = [0.0; MAXM];
        for i in 0..m {
            nu[i] = lin.z[i] - lin.hx[i];
            if lin.angle[i] {
                nu[i] = wrap_pi(nu[i]);
            }
        }
        let mut s = [[0.0; MAXM]; MAXM];
        for a in 0..m {
            for b in 0..m {
                let mut v = 0.0;
                for i in 0..6 {
                    if h_eff[a][i] == 0.0 {
                        continue;
                    }
                    for j in 0..6 {
                        v += h_eff[a][i] * p0[i][j] * h_eff[b][j];
                    }
                }
                s[a][b] = v + lin.r[a][b] + extra[a][b];
            }
        }
        let (s_inv, ln_det) = invert(&s, m)?;
        let mut d2 = 0.0;
        for a in 0..m {
            for b in 0..m {
                d2 += nu[a] * s_inv[a][b] * nu[b];
            }
        }
        if !d2.is_finite() {
            return None;
        }
        Some(Innov {
            m,
            nu,
            s,
            s_inv,
            d2,
            ln_det,
            h_eff,
            x0,
            p0,
            t_new,
        })
    }

    /// Fuse an innovation computed by [`Kf::innov`] on this filter.
    pub fn apply(&mut self, inn: &Innov) {
        let m = inn.m;
        // K = P0 H' S^-1 (6 x m)
        let mut ph = [[0.0; MAXM]; 6];
        for i in 0..6 {
            for a in 0..m {
                ph[i][a] = (0..6).map(|j| inn.p0[i][j] * inn.h_eff[a][j]).sum();
            }
        }
        let mut k = [[0.0; MAXM]; 6];
        for i in 0..6 {
            for a in 0..m {
                k[i][a] = (0..m).map(|b| ph[i][b] * inn.s_inv[b][a]).sum();
            }
        }
        let mut x = inn.x0;
        for (i, xi) in x.iter_mut().enumerate() {
            *xi += (0..m).map(|a| k[i][a] * inn.nu[a]).sum::<f64>();
        }
        // P = P0 - K S K'
        let mut p = inn.p0;
        for i in 0..6 {
            for j in 0..6 {
                let mut v = 0.0;
                for a in 0..m {
                    for b in 0..m {
                        v += k[i][a] * inn.s[a][b] * k[j][b];
                    }
                }
                p[i][j] -= v;
            }
        }
        for i in 0..6 {
            for j in i + 1..6 {
                let v = 0.5 * (p[i][j] + p[j][i]);
                p[i][j] = v;
                p[j][i] = v;
            }
            if p[i][i] < 1e-6 {
                p[i][i] = 1e-6;
            }
        }
        self.x = x;
        self.p = p;
        self.t_ms = inn.t_new;
        if self.surface {
            self.x[2] = 0.0;
            self.x[5] = 0.0;
        }
    }
}

/// Semi-major and semi-minor 1-sigma axes of a 2x2 covariance, and the major axis's grid
/// bearing (radians).
pub fn ellipse(pxx: f64, pxy: f64, pyy: f64) -> (f64, f64, f64) {
    let tr = pxx + pyy;
    let det = pxx * pyy - pxy * pxy;
    let disc = ((tr * tr / 4.0) - det).max(0.0).sqrt();
    let l1 = (tr / 2.0 + disc).max(0.0);
    let l2 = (tr / 2.0 - disc).max(0.0);
    // Eigenvector of l1: (pxy, l1 - pxx) or (l1 - pyy, pxy); bearing = atan2(x, y).
    let (vx, vy) = if pxy.abs() > 1e-12 {
        (pxy, l1 - pxx)
    } else if pxx >= pyy {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };
    (l1.sqrt(), l2.sqrt(), vx.atan2(vy))
}

/// Linear measurements on the state's components.
pub fn lin_components(
    idx: &[usize],
    z: &[f64],
    r: &[[f64; MAXM]; MAXM],
    offset: f64,
    x: &V6,
) -> Lin {
    let mut l = Lin::new(idx.len());
    for (a, &i) in idx.iter().enumerate() {
        l.h[a][i] = 1.0;
        l.hx[a] = x[i] + if i == 2 { offset } else { 0.0 };
        l.z[a] = z[a];
    }
    l.r = *r;
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_matches() {
        let a = [
            [4.0, 1.0, 0.0, 0.0],
            [1.0, 3.0, 0.0, 0.0],
            [0.0; 4],
            [0.0; 4],
        ];
        let (inv, ld) = invert(&a, 2).expect("invertible");
        assert!((inv[0][0] - 3.0 / 11.0).abs() < 1e-12);
        assert!((inv[0][1] + 1.0 / 11.0).abs() < 1e-12);
        assert!((ld - 11.0f64.ln()).abs() < 1e-12);
    }

    #[test]
    fn position_updates_converge() {
        let mut p = [[0.0; 6]; 6];
        for (i, v) in [100.0, 100.0, 100.0, 400.0, 400.0, 25.0].iter().enumerate() {
            p[i][i] = *v;
        }
        let mut kf = Kf::new(0, [0.0; 6], p, 1.0, 1.0, false);
        let mut r = [[0.0; MAXM]; MAXM];
        r[0][0] = 25.0;
        r[1][1] = 25.0;
        for k in 1..=20 {
            let t = k * 1000;
            let z = [10.0 * k as f64, 5.0 * k as f64];
            let inn = kf
                .innov(t, |x| lin_components(&[0, 1], &z, &r, 0.0, x))
                .expect("innov");
            kf.apply(&inn);
        }
        assert!((kf.x[3] - 10.0).abs() < 0.5, "{:?}", kf.x);
        assert!((kf.x[4] - 5.0).abs() < 0.5, "{:?}", kf.x);
    }
}
