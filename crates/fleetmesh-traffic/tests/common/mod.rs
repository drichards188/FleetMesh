//! A small AIS payload decoder for the tests, checked against published decodes in
//! reference.rs.
#![allow(dead_code)]

/// A payload as bits.
pub struct Rd {
    bits: Vec<bool>,
    at: usize,
}

impl Rd {
    pub fn new(payload: &str) -> Rd {
        let mut bits = Vec::new();
        for c in payload.bytes() {
            let mut v = c - 48;
            if v > 40 {
                v -= 8;
            }
            for i in (0..6).rev() {
                bits.push((v >> i) & 1 == 1);
            }
        }
        Rd { bits, at: 0 }
    }

    pub fn u(&mut self, n: usize) -> u64 {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | u64::from(self.bits.get(self.at).copied().unwrap_or(false));
            self.at += 1;
        }
        v
    }

    pub fn i(&mut self, n: usize) -> i64 {
        let v = self.u(n);
        if v >> (n - 1) & 1 == 1 { (v as i64) - (1i64 << n) } else { v as i64 }
    }

    pub fn b(&mut self) -> bool {
        self.u(1) == 1
    }

    /// Text as sent (padding included).
    pub fn text(&mut self, chars: usize) -> String {
        (0..chars)
            .map(|_| {
                let v = self.u(6) as u8;
                (if v < 32 { v + 64 } else { v }) as char
            })
            .collect()
    }
}

