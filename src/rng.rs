use std::time::{SystemTime, UNIX_EPOCH};

/// A tiny SplitMix64 generator. Not cryptographic, but plenty good for
/// shuffling and sampling a product list.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn from_seed(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Seed from the current time, the process id, and a stack address (which
    /// differs between threads).
    pub fn from_entropy() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let local = 0u8;
        let addr = &local as *const u8 as u64;
        Self::from_seed(nanos ^ addr.rotate_left(17) ^ ((std::process::id() as u64) << 32))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n`. `n` must be greater than zero.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}
