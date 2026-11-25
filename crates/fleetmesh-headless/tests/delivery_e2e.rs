//! End to end: a delivery scenario on stand-ins at fast forward, in one process on a paused clock
//! (the bridge's own code, the headless simulator), and what comes out of it.
//!
//! The dispatcher proposes pickups and deliveries as an agent; policy sends what it routes to a
//! person to the inbox; a scripted test operator (this file, and no code of the bridge's or of an
//! agent's) approves what waits there through the operator API; the vehicles fly, load and release.
//! Then: parcels were delivered, the audit chain verifies, and replay re-derives every decision
//! and every dispatch plan.
//!
//! Rules 2 (management by exception): under the shipped policy routine deliveries reach no one (no
//! drop zone is confirmed, every zone being an approved delivery point); under the strict
//! comparison procedure (`sim/policy-strict.toml`) a person confirms the zones, as in rules 1.
//!
//! The episode runs in a child process (`fleetmesh-headless`, as the Python driver runs it) so the
//! environment the bridge reads (`FLEETMESH_*`) is the child's own. It needs the repository's
//! `.venv` (sim/swarm.py plans the fleet); without it the test says so and passes.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo")
}

struct Episode {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
}

impl Episode {
    fn start(name: &str, scale: f64, workdir: &std::path::Path, port: u16, policy: Option<&str>) -> Option<Episode> {
        let root = repo();
        let py = root.join(".venv/bin/python");
        if !py.exists() {
            eprintln!("skipped: no {} (make setup)", py.display());
            return None;
        }
        let spec = Command::new(&py)
            .args(["scripts/scenario_spec.py", name, "--scale", &scale.to_string(), "--workdir"])
            .arg(workdir)
            .args(["--http-port", &port.to_string(), "--vehicle-base", &(port + 5000).to_string(), "--bridge-base", &(port + 6000).to_string()])
            .current_dir(&root)
            .output()
            .expect("scenario_spec.py");
        assert!(spec.status.success(), "scenario_spec.py: {}", String::from_utf8_lossy(&spec.stderr));
        let spec_line = String::from_utf8(spec.stdout).expect("utf8").trim().to_string();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_fleetmesh-headless"));
        match policy {
            Some(p) => cmd.env("FLEETMESH_POLICY", root.join(p)),
            None => cmd.env_remove("FLEETMESH_POLICY"),
        };
        let mut child = cmd
            .args(["--reply-fd", "1"])
            .current_dir(&root)
            .env("FLEETMESH_ROOT", &root)
            .env("FLEETMESH_SCENARIO_DIR", format!("sim/scenarios/{name}"))
            .env("FLEETMESH_SCENARIO_SCALE", scale.to_string())
            .env("FLEETMESH_AREAS", format!("sim/scenarios/{name}/areas"))
            .env("FLEETMESH_WEATHER_ARCHIVE", "off")
            .env("FLEETMESH_LAND_WATER_GRID", root.join("sim/terrain/golden-gate.water.grid.gz"))
            .env("FLEETMESH_ANCHORS", "0")
            .env("FLEETMESH_RESUME", "0")
            .env("FLEETMESH_RUN_ID", "e2e")
            .env_remove("FLEETMESH_TIDEWATER_URL")
            .env_remove("FLEETMESH_AT")
            .env_remove("FLEETMESH_RATE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("fleetmesh-headless");
        let mut stdin = child.stdin.take().expect("stdin");
        let out = BufReader::new(child.stdout.take().expect("stdout"));
        writeln!(stdin, "{spec_line}").expect("spec");
        let mut ep = Episode { child, stdin, out };
        let hello = ep.read_reply();
        assert_eq!(hello["ok"], json!(true), "episode did not start: {hello}");
        Some(ep)
    }

    /// The next line the episode answers with: a JSON object with `ok` (the bridge's own prints
    /// share this descriptor and are skipped).
    fn read_reply(&mut self) -> Value {
        loop {
            let mut line = String::new();
            let n = self.out.read_line(&mut line).expect("read");
            assert!(n > 0, "the episode ended");
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && v.get("ok").is_some()
            {
                return v;
            }
        }
    }

    fn rpc(&mut self, req: Value) -> Value {
        writeln!(self.stdin, "{req}").expect("request");
        self.stdin.flush().expect("flush");
        self.read_reply()
    }

    fn api(&mut self, method: &str, path: &str, operator: Option<&str>) -> Value {
        let r = self.rpc(json!({"op": "api", "method": method, "path": path, "operator": operator}));
        assert_eq!(r["ok"], json!(true), "{path}: {r}");
        r
    }
}

impl Drop for Episode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the scripted operator saw and did: each inbox item once, by kind and the reason codes of
/// its checks that did not pass, and the approvals it gave.
#[derive(Default)]
struct Seen {
    items: std::collections::BTreeMap<u64, (String, Vec<String>)>,
    approved: u32,
    alerts: std::collections::BTreeMap<String, String>,
}

impl Seen {
    fn kinds(&self) -> std::collections::BTreeMap<String, u32> {
        let mut out = std::collections::BTreeMap::new();
        for (kind, codes) in self.items.values() {
            *out.entry(format!("{kind} [{}]", codes.join(","))).or_insert(0) += 1;
        }
        out
    }

    fn any(&self, f: impl Fn(&str, &[String]) -> bool) -> bool {
        self.items.values().any(|(k, c)| f(k, c))
    }
}

/// Run `ep` to `end_s` in steps of 2 s; the scripted operator approves whatever waits in the
/// inbox, as the person at the console would, through the operator API (never an agent, never the
/// bridge itself).
fn operate(ep: &mut Episode, end_s: f64) -> (Seen, Value) {
    let mut seen = Seen::default();
    let mut t = 0.0;
    let mut last_parcels = Value::Null;
    while t < end_s {
        t += 2.0;
        let r = ep.rpc(json!({"op": "step", "t": t}));
        assert_eq!(r["ok"], json!(true), "{r}");
        let inbox = ep.api("GET", "/api/inbox", None);
        for a in inbox["body"]["alerts"].as_array().into_iter().flatten() {
            let kind = a["kind"].as_str().unwrap_or("?").to_string();
            // every issue automation raised says what happened, what it did and what to do
            if matches!(kind.as_str(), "drop_not_clear" | "zone_not_approved") {
                for part in ["problem", "did", "recommend"] {
                    assert!(a["issue"][part].as_str().is_some_and(|s| !s.is_empty()), "{kind} without its {part}: {a}");
                }
            }
            let said: String = a["detail"].as_str().unwrap_or("").chars().take(300).collect();
            seen.alerts.entry(a["id"].as_str().unwrap_or("?").to_string()).or_insert(format!("{kind}: {said}"));
        }
        if let Some(pending) = inbox["body"]["pending"].as_array() {
            for item in pending {
                let id = item["id"].as_u64().expect("an intent id");
                let kind = item["kind"].as_str().unwrap_or("?").to_string();
                let codes: Vec<String> = item["checks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| c["outcome"] != json!("pass"))
                    .flat_map(|c| c["codes"].as_array().cloned().unwrap_or_default())
                    .filter_map(|c| c.as_str().map(str::to_string))
                    .collect();
                seen.items.entry(id).or_insert((kind, codes));
                let r = ep.api("POST", &format!("/api/intents/{id}/approve"), Some("op-1"));
                if r["status"] == json!(200) {
                    seen.approved += 1;
                }
            }
        }
        if (t as u64).is_multiple_of(100) {
            last_parcels = ep.api("GET", "/api/parcels", None)["body"].clone();
        }
    }
    (seen, last_parcels)
}

/// The run's audit log: the chain verifies, there are plans and delivery events, and every
/// decision and plan re-derives.
fn audit_replays(ep: Episode) -> String {
    let mut ep = ep;
    let end = ep.rpc(json!({"op": "end"}));
    assert_eq!(end["ok"], json!(true), "{end}");
    let audit = end["audit"].as_str().expect("audit path").to_string();
    drop(ep);
    let text = std::fs::read_to_string(&audit).expect("audit log");
    if let Some(p) = fleetmesh_decide::replay::chain_problem(&text) {
        panic!("the audit chain does not verify: {p}");
    }
    assert!(text.contains("\"dispatch.plan\""), "no dispatch plan was logged");
    assert!(text.contains("\"delivery.event\""), "no delivery event was logged");
    let report = fleetmesh_decide::replay::replay_str(&text).expect("replay");
    assert!(report.mismatches.is_empty(), "replay found {} mismatches, first: {:?}", report.mismatches.len(), report.mismatches.first());
    assert_eq!(report.rules_version, fleetmesh_decide::rules::RULES_VERSION);
    text
}

#[test]
fn harbour_city_delivers_parcels_and_its_log_replays() {
    let dir = std::env::temp_dir().join(format!("fleetmesh-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let Some(mut ep) = Episode::start("harbour-city", 0.25, &dir, 19100 + (std::process::id() % 400) as u16, None) else {
        return;
    };
    let (seen, last_parcels) = operate(&mut ep, 1500.0);
    let parcels = ep.api("GET", "/api/parcels", None)["body"].clone();
    eprintln!("inbox items {:?}; approved {}; alerts {:?}; parcels {}", seen.kinds(), seen.approved, seen.alerts.values().collect::<Vec<_>>(), parcels["counts"]);
    if std::env::var("E2E_DEBUG").is_ok() {
        let intents = ep.api("GET", "/api/intents", None);
        for i in intents["body"]["intents"].as_array().into_iter().flatten().take(40) {
            eprintln!("intent {} {} {} {:?} uids {} :: {}", i["id"], i["task"]["kind"], i["state"], i["history"].as_array().and_then(|h| h.last()).map(|h| h["reason"].clone()), i["uids"], i["tasking"]["ops"]);
            if i["state"] == json!("rejected") || i["state"] == json!("failed") {
                eprintln!("   BAD {:?}", i["checks"].as_array().into_iter().flatten().filter(|c| c["outcome"] != json!("pass")).map(|c| c["rule"].to_string() + " " + &c["outcome"].to_string() + " " + &c["detail"].to_string()).collect::<Vec<_>>());
            }
        }
    }
    let delivered = parcels["counts"]["delivered"].as_u64().unwrap_or(0);
    assert!(delivered >= 3, "parcels delivered: {delivered}; {parcels}; earlier {last_parcels}");
    // rules 2: routine deliveries reached no one: no zone confirmation, no delivery held for its
    // zone's approval, no delivery held for its look (automation waits that out)
    assert!(!seen.any(|k, _| k == "confirm_drop_zone"), "a zone confirmation reached the person: {:?}", seen.kinds());
    assert!(
        !seen.any(|_, c| c.iter().any(|c| matches!(c.as_str(), "DZ_UNCONFIRMED" | "DZ_NOT_APPROVED" | "DZ_PEOPLE_NEAR" | "DZ_OBSERVATION_STALE"))),
        "a routine drop-zone hold reached the person: {:?}",
        seen.kinds()
    );
    // the payload is on the vehicles' reports
    let state = ep.api("GET", "/api/state", None);
    assert!(state["body"].to_string().contains("\"payload\""), "no vehicle reports its payload");
    let hubs = ep.api("GET", "/api/hubs", None);
    assert!(hubs["body"]["hubs"].as_array().is_some_and(|h| !h.is_empty()));
    // the console's map reads the places and each vehicle's navigation trust
    let places = ep.api("GET", "/api/places", None);
    assert!(places["body"]["hubs"][0]["packs"].is_number(), "places: {places}");
    assert!(places["body"]["drop_zones"].as_array().is_some_and(|z| !z.is_empty() && z[0]["at"].is_array()), "places: {places}");
    let nav = ep.api("GET", "/api/nav", None);
    assert!(nav["body"]["vehicles"].as_object().is_some_and(|v| !v.is_empty()), "nav: {nav}");
    // the weather panel reads the weather the rules read: the grid says so, with a wind in every region
    let wx = ep.api("GET", "/api/weather/grid?bbox=-122.6,37.7,-122.3,37.9", None);
    assert_eq!(wx["body"]["available"], json!(true), "weather grid: {wx}");
    assert!(wx["body"]["regions"]["outside"]["wind_east_mps"].is_number(), "weather grid: {wx}");
    let pt = ep.api("GET", "/api/weather?lat=37.8&lon=-122.44", None);
    assert_eq!(pt["body"]["available"], json!(true), "weather point: {pt}");
    audit_replays(ep);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn under_the_strict_procedure_a_person_confirms_the_zones() {
    // the comparison setting (sim/policy-strict.toml): rules 1's per-hour confirmation of each zone
    let dir = std::env::temp_dir().join(format!("fleetmesh-e2e-strict-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let Some(mut ep) = Episode::start("harbour-city", 0.25, &dir, 19600 + (std::process::id() % 300) as u16, Some("sim/policy-strict.toml")) else {
        return;
    };
    let (seen, _) = operate(&mut ep, 900.0);
    let parcels = ep.api("GET", "/api/parcels", None)["body"].clone();
    eprintln!("strict: inbox items {:?}; approved {}; parcels {}", seen.kinds(), seen.approved, parcels["counts"]);
    assert!(
        seen.any(|k, c| k == "confirm_drop_zone" || c.iter().any(|c| c == "DZ_UNCONFIRMED")),
        "the strict procedure asks a person about the zones: {:?}",
        seen.kinds()
    );
    assert!(seen.approved >= 1);
    let text = audit_replays(ep);
    assert!(text.contains("\"standing_approvals\":false"), "the run logged the setting it decided under");
    let _ = std::fs::remove_dir_all(&dir);
}
