//! The measurement model of the company's own sensors: bearing, elevation, range and range rate
//! from a sensor at a known position, as functions of the track state.

use crate::kf::{Lin, MAXM, V6};

/// Effective Earth radius for radio and optical paths near the surface (4/3 Earth), metres.
pub const R_EFF: f64 = 4.0 / 3.0 * 6_371_000.0;

/// One sensor detection, in local metres.
#[derive(Debug, Clone, PartialEq)]
pub struct Polar {
    /// Sensor position x, y and geometric altitude.
    pub s: [f64; 3],
    /// Sensor velocity, when the platform is moving and known (for range rate).
    pub sv: Option<[f64; 3]>,
    /// Convergence at the sensor (grid bearing of true north), radians.
    pub gamma: f64,
    /// True bearing and sigma, radians.
    pub bearing: (f64, f64),
    pub elev: Option<(f64, f64)>,
    pub range: Option<(f64, f64)>,
    pub rspeed: Option<(f64, f64)>,
}

impl Polar {
    pub fn dim(&self) -> usize {
        1 + self.elev.is_some() as usize
            + self.range.is_some() as usize
            + self.rspeed.is_some() as usize
    }

    /// Linearise at state `x`. `z_off` maps the track's altitude to geometric (the track may hold
    /// pressure altitude), `z_var` is that mapping's variance.
    pub fn lin(&self, x: &V6, z_off: f64, z_var: f64, surface: bool) -> Lin {
        let mut l = Lin::new(self.dim());
        let dx = x[0] - self.s[0];
        let dy = x[1] - self.s[1];
        let rho2 = (dx * dx + dy * dy).max(1.0);
        // The target sits lower by the Earth's curvature over the path (4/3 Earth, the
        // standard allowance for refraction): 15 m at 16 km, 150 m at 50 km.
        let dz = x[2] + z_off - self.s[2] - rho2 / (2.0 * R_EFF);
        let rho = rho2.sqrt();
        let r2 = rho2 + dz * dz;
        let r = r2.sqrt();
        let mut k = 0;
        // Bearing (true): grid bearing minus convergence.
        l.z[k] = self.bearing.0;
        l.hx[k] = dx.atan2(dy) - self.gamma;
        l.h[k][0] = dy / rho2;
        l.h[k][1] = -dx / rho2;
        l.r[k][k] = self.bearing.1 * self.bearing.1;
        l.angle[k] = true;
        k += 1;
        if let Some((e, se)) = self.elev {
            l.z[k] = e;
            l.hx[k] = dz.atan2(rho);
            l.h[k][0] = -dz * dx / (r2 * rho);
            l.h[k][1] = -dz * dy / (r2 * rho);
            if !surface {
                l.h[k][2] = rho / r2;
            }
            l.r[k][k] = se * se;
            l.angle[k] = true;
            k += 1;
        }
        if let Some((rg, sr)) = self.range {
            l.z[k] = rg;
            l.hx[k] = r;
            l.h[k][0] = dx / r;
            l.h[k][1] = dy / r;
            if !surface {
                l.h[k][2] = dz / r;
            }
            l.r[k][k] = sr * sr;
            k += 1;
        }
        if let Some((rr, srr)) = self.rspeed {
            let sv = self.sv.unwrap_or([0.0; 3]);
            let rv = [
                x[3] - sv[0],
                x[4] - sv[1],
                if surface { 0.0 } else { x[5] - sv[2] },
            ];
            let rdot = (dx * rv[0] + dy * rv[1] + dz * rv[2]) / r;
            l.z[k] = rr;
            l.hx[k] = rdot;
            l.h[k][0] = (rv[0] - rdot * dx / r) / r;
            l.h[k][1] = (rv[1] - rdot * dy / r) / r;
            if !surface {
                l.h[k][2] = (rv[2] - rdot * dz / r) / r;
                l.h[k][5] = dz / r;
            }
            l.h[k][3] = dx / r;
            l.h[k][4] = dy / r;
            l.r[k][k] = srr * srr;
            k += 1;
        }
        debug_assert_eq!(k, l.m);
        // The altitude reference's own uncertainty enters through the z column.
        if z_var > 0.0 && !surface {
            for a in 0..l.m {
                for b in 0..l.m {
                    l.r[a][b] += l.h[a][2] * z_var * l.h[b][2];
                }
            }
        }
        l
    }

    /// The detected position (when the detection has a range), local metres, and its
    /// horizontal covariance. For a detection without elevation the slant range is taken as
    /// horizontal and `z_guess` as the altitude.
    pub fn fix(&self, z_guess: f64) -> Option<([f64; 3], [[f64; 2]; 2])> {
        let (r, sr) = self.range?;
        let bg = self.bearing.0 + self.gamma;
        let (rho, z) = match self.elev {
            Some((e, _)) => {
                let rho = r * e.cos();
                (rho, self.s[2] + r * e.sin() + rho * rho / (2.0 * R_EFF))
            }
            None => {
                let dz = z_guess - self.s[2];
                ((r * r - dz * dz).max(0.0).sqrt().max(r * 0.5), z_guess)
            }
        };
        let (sb, cb) = bg.sin_cos();
        let p = [self.s[0] + rho * sb, self.s[1] + rho * cb, z];
        let along = sr * sr;
        let cross = (rho * self.bearing.1).powi(2);
        // Rotate diag(along, cross) to grid axes: along is (sin b, cos b), cross (cos b, -sin b).
        let c = [
            [along * sb * sb + cross * cb * cb, (along - cross) * sb * cb],
            [(along - cross) * sb * cb, along * cb * cb + cross * sb * sb],
        ];
        Some((p, c))
    }
}

/// Linear (component) measurement with an explicit noise matrix.
pub fn noise2(c: [[f64; 2]; 2]) -> [[f64; MAXM]; MAXM] {
    let mut r = [[0.0; MAXM]; MAXM];
    r[0][0] = c[0][0];
    r[0][1] = c[0][1];
    r[1][0] = c[1][0];
    r[1][1] = c[1][1];
    r
}

pub fn noise1(v: f64) -> [[f64; MAXM]; MAXM] {
    let mut r = [[0.0; MAXM]; MAXM];
    r[0][0] = v;
    r
}

/// Covariance of a velocity measured as speed and course: sigma along the course, and speed
/// times the course sigma across it (at least the speed sigma).
pub fn speed_course_cov(
    speed: f64,
    course_grid: f64,
    s_speed: f64,
    s_course: f64,
) -> [[f64; 2]; 2] {
    let (s, c) = course_grid.sin_cos();
    let a = s_speed * s_speed;
    let x = (speed * s_course).max(s_speed).powi(2);
    // u = (s, c) along, w = (c, -s) across.
    [
        [a * s * s + x * c * c, (a - x) * s * c],
        [(a - x) * s * c, a * c * c + x * s * s],
    ]
}
