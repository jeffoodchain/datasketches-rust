use std::cell::Cell;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

fn seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    // xorshift64 degenerates to all zeros if seeded with zero.
    if nanos == 0 {
        0x2545_F491_4F6C_DD1D
    } else {
        nanos
    }
}

// Returns a pseudo-random bit.
pub(crate) fn random_bit() -> bool {
    thread_local! {
        static RNG_STATE: Cell<u64> = Cell::new(seed());
    }
    RNG_STATE.with(|state| {
        let mut x = state.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        state.set(x);
        (x >> 63) != 0
    })
}
