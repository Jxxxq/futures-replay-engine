use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

#[derive(Clone, Copy, Debug, Default)]
pub struct FastHasher {
    state: u64,
}

impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        mix_u64(self.state)
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.state = hash;
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.state = u64::from(i);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.state = u64::from(i);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.state = u64::from(i);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.state = i;
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.state = i as u64;
    }
}

#[inline]
fn mix_u64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_hashes_are_stable_and_distinct() {
        let mut first = FastHasher::default();
        first.write_u64(1);
        let mut second = FastHasher::default();
        second.write_u64(2);

        assert_ne!(first.finish(), second.finish());
        assert_eq!(first.finish(), first.finish());
    }
}
