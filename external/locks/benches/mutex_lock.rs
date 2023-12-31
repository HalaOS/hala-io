use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use divan::Bencher;
use locks::Locker;

fn main() {
    // Run registered benchmarks.
    divan::main();
}

#[divan::bench(threads)]
fn mutex_lock(bench: Bencher) {
    let mutex = Arc::new(Mutex::new(1));

    let mutex_cloned = mutex.clone();

    let dropping = Arc::new(AtomicBool::new(false));

    let dropping_cloned = dropping.clone();

    std::thread::spawn(move || loop {
        while !dropping_cloned.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
            let _ = mutex_cloned.sync_lock();
        }
    });

    bench.bench(|| {
        let _ = mutex.sync_lock();
    });

    dropping.store(true, Ordering::Release);
}

#[divan::bench]
fn mutex_lock_seq(bench: Bencher) {
    let mutex = Mutex::new(1);

    bench.bench_local(|| {
        let _ = mutex.sync_lock();
    });
}
