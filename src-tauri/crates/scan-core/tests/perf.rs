//! Batch 1 target: a 100k-file folder compares in a few seconds. Run with
//! `cargo test -p scan-core --release --test perf -- --ignored --nocapture`.

use scan_core::{compare, scan, ScanState};
use std::fs;
use std::time::{Duration, Instant};

#[test]
#[ignore = "creates 200k files; run manually"]
fn hundred_thousand_files_compare_quickly() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    for (root, skip_every) in [(left.path(), 0), (right.path(), 97)] {
        let mut written = 0;
        for a in 0..100 {
            for b in 0..10 {
                let dir = root.join(format!("d{a}")).join(format!("e{b}"));
                fs::create_dir_all(&dir).unwrap();
                for c in 0..100 {
                    written += 1;
                    if skip_every > 0 && written % skip_every == 0 {
                        continue;
                    }
                    fs::write(dir.join(format!("f{c}.txt")), b"data").unwrap();
                }
            }
        }
    }

    let started = Instant::now();
    let state = ScanState::default();
    let left_tree = scan(left.path(), &state).unwrap();
    let right_tree = scan(right.path(), &state).unwrap();
    let scanned = started.elapsed();
    let diff = compare(&left_tree, &right_tree);
    let total = started.elapsed();

    println!(
        "scan 2x100k: {scanned:?}, compare: {:?}, total: {total:?}, missing: {}",
        total - scanned,
        diff.summary().missing
    );
    assert_eq!(left_tree.root().files, 100_000);
    assert_eq!(diff.summary().missing, 100_000 / 97);
    assert!(total < Duration::from_secs(5), "took {total:?}");
}
