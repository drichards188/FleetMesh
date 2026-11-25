//! Determinism. Every random draw is a pure function of the seed and a stable key (the object,
//! the channel, the message counter, the receiver), so the world and the bytes it emits do not
//! depend on how the caller steps it, on hash-map order or on thread timing. Schedules (the
//! sequence of arrivals in a stream) use a ChaCha generator seeded from the same kind of key.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// SplitMix64's finalizer: a good 64-bit mix.
pub const fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A key of several parts.
pub fn hash(seed: u64, parts: &[u64]) -> u64 {
    let mut h = mix(seed ^ 0x5EED_0F7A_FF1C);
    for p in parts {
        h = mix(h ^ mix(*p));
    }
    h
}

/// FNV-1a of a string: a stable tag for names in keys.
pub fn tag(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Uniform in [0, 1).
pub fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Uniform in [lo, hi).
pub fn range(h: u64, lo: f64, hi: f64) -> f64 {
    lo + (hi - lo) * unit(h)
}

/// Standard normal (Box-Muller).
pub fn normal(h: u64) -> f64 {
    let u1 = unit(mix(h ^ 0x1)).max(1e-12);
    let u2 = unit(mix(h ^ 0x2));
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// A generator for a schedule.
pub fn chacha(seed: u64, parts: &[u64]) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(hash(seed, parts))
}

/// Smooth, zero-mean noise of about unit standard deviation with a correlation time of
/// `tau_s`: a sum of sinusoids with drawn frequencies and phases. A pure function of time,
/// so a GNSS error or a wander evaluated at any instant is the same however it is stepped.
#[derive(Clone, Debug)]
pub struct Smooth {
    terms: [(f64, f64); 4],
}

impl Smooth {
    pub fn new(key: u64, tau_s: f64) -> Smooth {
        let mut terms = [(0.0, 0.0); 4];
        for (i, t) in terms.iter_mut().enumerate() {
            let k = mix(key ^ (i as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407));
            // frequencies spread around 1 / tau
            let f = range(k, 0.3, 1.7) / tau_s.max(1e-3);
            let ph = range(mix(k), 0.0, std::f64::consts::TAU);
            *t = (f, ph);
        }
        Smooth { terms }
    }

    pub fn at(&self, t_s: f64) -> f64 {
        // four unit-amplitude sinusoids: variance 4 * 1/2 = 2, scaled to 1
        let s: f64 = self.terms.iter().map(|(f, ph)| (std::f64::consts::TAU * f * t_s + ph).sin()).sum();
        s / std::f64::consts::SQRT_2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_are_stable_and_spread() {
        assert_eq!(hash(1, &[2, 3]), hash(1, &[2, 3]));
        assert_ne!(hash(1, &[2, 3]), hash(1, &[3, 2]));
        let mean: f64 = (0..10_000).map(|i| unit(hash(7, &[i]))).sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
        let var: f64 = (0..10_000).map(|i| normal(hash(7, &[i])).powi(2)).sum::<f64>() / 10_000.0;
        assert!((var - 1.0).abs() < 0.08, "{var}");
        let s = Smooth::new(5, 60.0);
        let v: f64 = (0..20_000).map(|i| s.at(i as f64).powi(2)).sum::<f64>() / 20_000.0;
        assert!((v - 1.0).abs() < 0.35, "{v}");
    }
}
