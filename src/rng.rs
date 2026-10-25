//! Small, fast pseudo-random numbers (xorshift32) for faces and
//! animations. Not for anything that needs real randomness.

pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed | 1)
    }

    /// Seeded differently from boot to boot (and run to run on the host).
    pub fn from_entropy() -> Self {
        Self::new(seed())
    }

    pub fn next(&mut self) -> u32 {
        let s = &mut self.0;
        *s ^= *s << 13;
        *s ^= *s >> 17;
        *s ^= *s << 5;
        *s
    }

    /// Uniform-ish in 0..n (n > 0).
    pub fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }

    /// Uniform-ish in lo..hi (lo < hi).
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo) as u32) as i32
    }
}

/// A seed that differs from boot to boot (and run to run on the host).
pub fn seed() -> u32 {
    #[cfg(target_os = "espidf")]
    {
        unsafe { esp_idf_svc::sys::esp_random() }
    }
    #[cfg(not(target_os = "espidf"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos())
    }
}
