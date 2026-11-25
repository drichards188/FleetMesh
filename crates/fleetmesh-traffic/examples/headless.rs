//! Run a scenario headless and print what the world holds and what the receivers heard:
//! `cargo run -p fleetmesh-traffic --release --example headless -- harbour-city 3600`.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use fleetmesh_traffic::{Options, Traffic};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = args.get(1).map_or("harbour-city", String::as_str);
    let secs: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(600);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut t = Traffic::from_files(
        &root.join("sim/traffic.toml"),
        Some(&root.join(format!("sim/scenarios/{scenario}/traffic.toml"))),
        Options::default(),
    )
    .unwrap();
    let start = t.start_ms();
    let wall = Instant::now();
    let mut lines: BTreeMap<String, usize> = BTreeMap::new();
    for s in 1..=secs {
        for e in t.step_to(start + s * 1000) {
            *lines.entry(format!("{:?}/{}", e.stream, e.kind)).or_insert(0) += 1;
        }
        if s % 600 == 0 || s == 1 {
            let mut by_stream: BTreeMap<String, usize> = BTreeMap::new();
            for o in t.truth() {
                *by_stream.entry(format!("{}/{}", o.stream, o.class)).or_insert(0) += 1;
            }
            println!("t={s:>5}s objects {:?}", t.counts());
            println!("        by stream {by_stream:?}");
        }
    }
    println!("{secs} s of scenario in {:.2} s wall", wall.elapsed().as_secs_f64());
    println!("lines {lines:?}");
    println!("{}", serde_json::to_string_pretty(&t.stats()).unwrap());
}
