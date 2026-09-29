// SPDX-License-Identifier: MPL-2.0

#![allow(unused_variables)]

use rand::{rngs::StdRng, Error as RandError, RngCore};
use spin::Once;

use crate::prelude::*;

static RNG: Once<SpinLock<StdRng>> = Once::new();

/// Fill `dest` with random bytes.
///
/// It's cryptographically secure, as documented in [`rand::rngs::StdRng`].
pub fn getrandom(dst: &mut [u8]) -> Result<()> {
    Ok(RNG.get().unwrap().lock().try_fill_bytes(dst)?)
}

pub fn init() {
    // The seed used to initialize the RNG is required to be secure and unpredictable.

    cfg_if::cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            use rand::SeedableRng;
            use ostd::arch::read_random;

            let mut seed = <StdRng as SeedableRng>::Seed::default();
            let mut chunks = seed.as_mut().chunks_exact_mut(size_of::<u64>());
            for chunk in chunks.by_ref() {
                let src = read_random().expect("read_random failed multiple times").to_ne_bytes();
                chunk.copy_from_slice(&src);
            }
            let tail = chunks.into_remainder();
            let n = tail.len();
            if n > 0 {
                let src = read_random().expect("read_random failed multiple times").to_ne_bytes();
                tail.copy_from_slice(&src[..n]);
            }

            RNG.call_once(|| SpinLock::new(StdRng::from_seed(seed)));
        } else if #[cfg(target_arch = "riscv64")] {
            use rand::SeedableRng;
            use ostd::arch::{boot::DEVICE_TREE, read_tsc};

            // Try DTB-provided rng-seed first for best entropy.
            // If not present (e.g., external DTB passed via -dtb), fall back to a
            // time-based software seed to avoid boot panic. The fallback is NOT
            // cryptographically strong and should be replaced when true entropy is available.
            let dtb_seed: Option<[u8; 32]> = DEVICE_TREE
                .get()
                .and_then(|f| f.find_node("/chosen"))
                .and_then(|c| c.property("rng-seed"))
                .and_then(|p| <[u8; 32]>::try_from(p.value).ok());

            let seed = if let Some(seed) = dtb_seed {
                seed
            } else {
                // Fallback: derive a 32-byte seed from time counter and addresses.
                // This is a best-effort initializer to keep the system running.
                let mut seed = <StdRng as SeedableRng>::Seed::default();
                // Precompute a salt from the seed's address before taking a mutable borrow.
                let seed_salt = (&seed as *const _ as usize) as u64;
                let mut chunks = seed.as_mut().chunks_exact_mut(core::mem::size_of::<u64>());
                for (i, chunk) in chunks.by_ref().enumerate() {
                    // Mix TSC with a varying offset and pointer bits.
                    let mut v = read_tsc();
                    v ^= seed_salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    v ^= (i as u64).wrapping_mul(0x94d0_49bb_1331_11eb);
                    chunk.copy_from_slice(&v.to_ne_bytes());
                }
                let tail = chunks.into_remainder();
                if !tail.is_empty() {
                    let mut v = read_tsc();
                    v ^= ((tail.as_ptr() as usize) as u64).wrapping_add(0x517c_c1b7_2722_0a95);
                    let bytes = v.to_ne_bytes();
                    tail.copy_from_slice(&bytes[..tail.len()]);
                }
                seed
            };

            RNG.call_once(|| SpinLock::new(StdRng::from_seed(seed)));
        } else {
            compile_error!("unsupported target");
        }
    }
}

impl From<RandError> for Error {
    fn from(value: RandError) -> Self {
        Error::with_message(Errno::ENOSYS, "cannot generate random bytes")
    }
}
