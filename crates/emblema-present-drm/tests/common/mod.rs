//! What the test binaries that drive a real card share.
//!
//! Modesetting master is exclusive per device, and two things contend for it
//! here. Within one binary it is threads, which a mutex settles and which
//! `kms.rs` has always done. Across binaries it is processes: `cargo test`
//! runs each test target at the same time, so `kms.rs` and `writeback.rs` can
//! both reach for the same card, and the one that loses reports itself
//! skipped and passes.
//!
//! That is the worse failure, because a skip is green. It was observed once,
//! in a gate run whose census carried `no card offers writeback` while the
//! suite reported no failures -- the count was a test short and nothing said
//! so except one line nobody reads twice.
//!
//! A file lock is what crosses the process boundary. It serializes only our
//! own binaries, which is exactly the contention seen: something else holding
//! master is a different situation and is still reported as a skip, correctly,
//! because waiting for a compositor to exit is not something a test should do.

use std::fs::File;
use std::sync::{Mutex, MutexGuard};

/// Threads within one binary.
///
/// A poisoned lock is taken anyway: a panicking test says nothing about
/// whether the card is usable.
static CARD: Mutex<()> = Mutex::new(());

/// Held for as long as a test is driving the card.
///
/// Both halves release on drop, the file lock when its descriptor closes.
pub struct CardGuard {
    _process: MutexGuard<'static, ()>,
    _across_processes: Option<File>,
}

/// Wait until this process may drive the card, then hold the right to.
///
/// Waits rather than refuses. The alternative is the skip this exists to
/// prevent, and the wait is bounded by how long the other binary's card tests
/// take -- a couple of seconds.
pub fn take_the_card() -> CardGuard {
    let process = CARD.lock().unwrap_or_else(|e| e.into_inner());

    // A fixed path rather than a temporary one: two processes have to agree
    // on which file they are contending for, so it cannot be unique per run.
    let path = std::env::temp_dir().join("emblema-drm-master.lock");
    let across_processes = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .ok()
        .and_then(|file| {
            // A machine where this cannot be locked -- a filesystem without
            // it, a path that is not writable -- is one where the old
            // behavior applies, and that is better than refusing to run.
            rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)
                .ok()
                .map(|()| file)
        });

    CardGuard {
        _process: process,
        _across_processes: across_processes,
    }
}
