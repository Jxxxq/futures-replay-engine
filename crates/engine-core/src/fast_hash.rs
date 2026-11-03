use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

#[derive(Clone, Copy, Debug, Default)]
pub struct FastHasher {
    state: u64,
}

impl FastHasher {
    #[inline]
    fn add(&mut self, value: u64) {
        self.state = self.state.rotate_left(5) ^ value;
    }
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
        self.add(hash);
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
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

    #[test]
    fn every_field_of_a_key_changes_the_hash() {
        use crate::types::{InstrumentId, InstrumentKey, PublisherId};
        use std::hash::BuildHasher;

        let build = BuildHasherDefault::<FastHasher>::default();
        let key = |instrument: u32, epoch: &str| InstrumentKey {
            dataset: "GLBX.MDP3".to_string(),
            publisher_id: PublisherId::new(1),
            instrument_id: InstrumentId::new(instrument),
            symbol_epoch: epoch.to_string(),
        };

        assert_ne!(build.hash_one(key(1, "a")), build.hash_one(key(2, "a")));
        assert_ne!(build.hash_one(key(1, "a")), build.hash_one(key(1, "b")));
        assert_ne!(
            build.hash_one((1_u16, 7_u32)),
            build.hash_one((2_u16, 7_u32))
        );
    }
}
