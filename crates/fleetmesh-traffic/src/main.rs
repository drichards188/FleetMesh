//! fleetmesh-traffic: the live binary. Steps the world on the scenario clock (`FLEETMESH_AT`
//! plus elapsed times `FLEETMESH_RATE`) and serves docs/design/traffic.md section 5:
//!
//! - HTTP `FLEETMESH_TRAFFIC_HTTP_PORT` (8767): `/health`, `/sensors`, `/truth` (evaluation
//!   only), `/stats`
//! - TCP `FLEETMESH_TRAFFIC_SBS_PORT` (8768): SBS-1 BaseStation lines
//! - TCP `FLEETMESH_TRAFFIC_AIS_PORT` (8769): `!AIVDM` sentences with NMEA 4.10 tag blocks
//! - TCP `FLEETMESH_TRAFFIC_JSON_PORT` (8771): Remote ID and detection JSON lines
//!
//! Own vehicles (for the onboard sensors) are polled read-only from the bridge's `GET
//! /api/state`; without a bridge the onboard sensors simply have no platforms.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use clap::Parser;
use fleetmesh_traffic::{Options, OwnKind, OwnVehicle, Stream, Traffic};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

#[derive(Parser, Debug)]
#[command(name = "fleetmesh-traffic", about = "Other traffic and the company's receivers, in real wire formats")]
struct Cli {
    /// The shared world
    #[arg(long, default_value = "sim/traffic.toml")]
    config: PathBuf,
    /// A scenario's traffic (rates, seed, issue events)
    #[arg(long)]
    scenario: Option<PathBuf>,
    /// Overrides the scenario's seed
    #[arg(long)]
    seed: Option<u64>,
    /// The bridge to read own vehicles from (`off` for none)
    #[arg(long, default_value = "http://127.0.0.1:8765")]
    bridge: String,
    /// How often to read the bridge, wall milliseconds
    #[arg(long, default_value_t = 1000)]
    bridge_poll_ms: u64,
    /// Step interval, wall milliseconds
    #[arg(long, default_value_t = 100)]
    step_ms: u64,
    /// Address to listen on (default `FLEETMESH_TRAFFIC_BIND`, else 127.0.0.1)
    #[arg(long)]
    bind: Option<String>,
}

fn port(var: &str, default: u16) -> Result<u16, String> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v.trim().parse().map_err(|_| format!("{var}={v:?} is not a port")),
        _ => Ok(default),
    }
}

#[derive(Default, Serialize, Clone)]
struct BridgeStatus {
    url: Option<String>,
    state: &'static str,
    last_error: Option<String>,
    polls: u64,
    failures: u64,
    vehicles: usize,
}

struct Shared {
    traffic: Mutex<Traffic>,
    bridge: Mutex<BridgeStatus>,
    clients: [AtomicUsize; 3],
    lagged: AtomicU64,
    rate: f64,
    started: Instant,
    steps: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn json_response<T: Serialize>(v: &T) -> Response {
    match serde_json::to_string(v) {
        Ok(s) => ([(header::CONTENT_TYPE, "application/json")], s).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn health(State(s): State<Arc<Shared>>) -> Response {
    let t = lock(&s.traffic).now_ms();
    let b = lock(&s.bridge).state;
    json_response(&serde_json::json!({"ok": true, "t": t, "rate": s.rate, "bridge": b}))
}

async fn sensors(State(s): State<Arc<Shared>>) -> Response {
    let v = lock(&s.traffic).sensors();
    json_response(&v)
}

async fn truth(State(s): State<Arc<Shared>>) -> Response {
    let tr = lock(&s.traffic);
    #[derive(Serialize)]
    struct T<'a> {
        t: i64,
        note: &'a str,
        objects: Vec<fleetmesh_traffic::TruthObject>,
    }
    json_response(&T { t: tr.now_ms(), note: "evaluation only: never an input to the tracker or the engine", objects: tr.truth() })
}

async fn stats(State(s): State<Arc<Shared>>) -> Response {
    let st = lock(&s.traffic).stats();
    #[derive(Serialize)]
    struct S {
        #[serde(flatten)]
        traffic: fleetmesh_traffic::Stats,
        bridge: BridgeStatus,
        clients: [usize; 3],
        lagged_batches: u64,
        steps: u64,
        wall_s: f64,
        rate: f64,
    }
    let b = lock(&s.bridge).clone();
    json_response(&S {
        traffic: st,
        bridge: b,
        clients: [0, 1, 2].map(|i| s.clients[i].load(Ordering::Relaxed)),
        lagged_batches: s.lagged.load(Ordering::Relaxed),
        steps: s.steps.load(Ordering::Relaxed),
        wall_s: s.started.elapsed().as_secs_f64(),
        rate: s.rate,
    })
}

/// Serve one stream: every client gets every batch from the moment it connects.
async fn serve_stream(listener: TcpListener, tx: broadcast::Sender<Arc<String>>, shared: Arc<Shared>, idx: usize) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else { continue };
        let mut rx = tx.subscribe();
        let sh = shared.clone();
        tokio::spawn(async move {
            sh.clients[idx].fetch_add(1, Ordering::Relaxed);
            let _ = sock.set_nodelay(true);
            loop {
                match rx.recv().await {
                    Ok(batch) => {
                        if sock.write_all(batch.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        sh.lagged.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            sh.clients[idx].fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// A plain HTTP/1.0 GET (no chunking to undo), read-only.
async fn http_get(url: &str, timeout: Duration) -> Result<String, String> {
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("{url}: only http:// is supported"))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let fut = async {
        let mut s = TcpStream::connect(hostport).await.map_err(|e| format!("connect {hostport}: {e}"))?;
        let req = format!("GET {path} HTTP/1.0\r\nHost: {hostport}\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        s.take(256 * 1024 * 1024).read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
        Ok::<Vec<u8>, String>(buf)
    };
    let buf = tokio::time::timeout(timeout, fut).await.map_err(|_| format!("{url}: timed out"))??;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| format!("{url}: no HTTP header"))?;
    let status = head.lines().next().unwrap_or("");
    if !status.split_whitespace().nth(1).is_some_and(|c| c == "200") {
        return Err(format!("{url}: {status}"));
    }
    Ok(body.to_string())
}

/// Own vehicles from the bridge's `/api/state` (`vehicles[]`: uid, kind, position [lon, lat],
/// alt_msl, alt_rel, heading, groundspeed).
fn own_vehicles(state: &Value) -> Vec<OwnVehicle> {
    let Some(vs) = state["vehicles"].as_array() else { return Vec::new() };
    vs.iter()
        .filter_map(|v| {
            let pos = v["position"].as_array()?;
            let (lon, lat) = (pos.first()?.as_f64()?, pos.get(1)?.as_f64()?);
            let uid = match &v["uid"] {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => return None,
            };
            let kind = match v["kind"].as_str().unwrap_or("") {
                "copter" | "vtol" | "blimp" => OwnKind::Drone,
                "plane" => OwnKind::Plane,
                "boat" => OwnKind::Boat,
                _ => OwnKind::Ground,
            };
            let alt_rel = v["alt_rel"].as_f64();
            Some(OwnVehicle {
                uid,
                kind,
                lat,
                lon,
                alt_msl_m: v["alt_msl"].as_f64().or(alt_rel).unwrap_or(0.0),
                alt_rel_m: alt_rel,
                heading_deg: v["heading"].as_f64().unwrap_or(0.0),
                speed_mps: v["groundspeed"].as_f64().unwrap_or(0.0),
            })
        })
        .collect()
}

async fn poll_bridge(shared: Arc<Shared>, url: String, every: Duration) {
    let state_url = format!("{}/api/state", url.trim_end_matches('/'));
    loop {
        let r = http_get(&state_url, Duration::from_secs(3)).await.and_then(|body| serde_json::from_str::<Value>(&body).map_err(|e| e.to_string()));
        let vs = {
            let mut b = lock(&shared.bridge);
            b.polls += 1;
            match r {
                Ok(v) => {
                    let vs = own_vehicles(&v);
                    b.state = "ok";
                    b.last_error = None;
                    b.vehicles = vs.len();
                    vs
                }
                Err(e) => {
                    b.failures += 1;
                    if b.state != "unreachable" {
                        eprintln!("fleetmesh-traffic: bridge {state_url}: {e} (no own vehicles until it answers)");
                    }
                    b.state = "unreachable";
                    b.last_error = Some(e);
                    b.vehicles = 0;
                    Vec::new()
                }
            }
        };
        lock(&shared.traffic).set_own_vehicles(vs);
        tokio::time::sleep(every).await;
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("fleetmesh-traffic: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let clock = fleetmesh_space::scenario_clock::from_env()?;
    let wall_now_ms = chrono::Utc::now().timestamp_millis();
    let start_ms = clock.at.map_or(wall_now_ms, |t| t.timestamp_millis());
    let cfg = fleetmesh_traffic::config::WorldConfig::load(&cli.config)?;
    let horizon = cfg.world.horizon_s;
    let scn = match &cli.scenario {
        Some(p) => fleetmesh_traffic::config::ScenarioCfg::load(p)?,
        None => Default::default(),
    };
    let traffic = Traffic::new(cfg, scn, Options { seed: cli.seed, start_ms: Some(start_ms), duration_s: Some(horizon) })?;
    let ports = [
        port("FLEETMESH_TRAFFIC_HTTP_PORT", 8767)?,
        port("FLEETMESH_TRAFFIC_SBS_PORT", 8768)?,
        port("FLEETMESH_TRAFFIC_AIS_PORT", 8769)?,
        port("FLEETMESH_TRAFFIC_JSON_PORT", 8771)?,
    ];
    let st = traffic.stats();
    eprintln!(
        "fleetmesh-traffic: seed {}, start {} ({}), rate {}, terrain: {}, {} receivers",
        traffic.seed(),
        start_ms,
        chrono::DateTime::from_timestamp_millis(start_ms).map(|d| d.to_rfc3339()).unwrap_or_default(),
        clock.rate,
        st.terrain,
        traffic.sensors().len()
    );
    let shared = Arc::new(Shared {
        traffic: Mutex::new(traffic),
        bridge: Mutex::new(BridgeStatus { state: "off", ..Default::default() }),
        clients: [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)],
        lagged: AtomicU64::new(0),
        rate: clock.rate,
        started: Instant::now(),
        steps: AtomicU64::new(0),
    });
    let host = cli
        .bind
        .clone()
        .or_else(|| std::env::var("FLEETMESH_TRAFFIC_BIND").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "127.0.0.1".into());
    let bind = |p: u16| format!("{host}:{p}");
    let http = TcpListener::bind(bind(ports[0])).await.map_err(|e| format!("{}: {e}", bind(ports[0])))?;
    let mut senders = Vec::new();
    for (i, p) in ports[1..].iter().enumerate() {
        let l = TcpListener::bind(bind(*p)).await.map_err(|e| format!("{}: {e}", bind(*p)))?;
        let (tx, _) = broadcast::channel::<Arc<String>>(4096);
        tokio::spawn(serve_stream(l, tx.clone(), shared.clone(), i));
        senders.push(tx);
    }
    eprintln!(
        "fleetmesh-traffic: http {} (/health /sensors /truth /stats), sbs {}, ais {}, json {}",
        bind(ports[0]),
        bind(ports[1]),
        bind(ports[2]),
        bind(ports[3])
    );
    if cli.bridge != "off" && !cli.bridge.is_empty() {
        lock(&shared.bridge).url = Some(cli.bridge.clone());
        lock(&shared.bridge).state = "connecting";
        tokio::spawn(poll_bridge(shared.clone(), cli.bridge.clone(), Duration::from_millis(cli.bridge_poll_ms.max(100))));
    }
    let app = Router::new()
        .route("/health", get(health))
        .route("/sensors", get(sensors))
        .route("/truth", get(truth))
        .route("/stats", get(stats))
        .with_state(shared.clone());
    tokio::spawn(async move {
        let _ = axum::serve(http, app).await;
    });
    let stepper = {
        let shared = shared.clone();
        let step = Duration::from_millis(cli.step_ms.max(10));
        let wall0 = Instant::now();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(step);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let t = start_ms + (wall0.elapsed().as_secs_f64() * shared.rate * 1000.0) as i64;
                let lines = lock(&shared.traffic).step_to(t);
                shared.steps.fetch_add(1, Ordering::Relaxed);
                let mut batch = [String::new(), String::new(), String::new()];
                for e in lines {
                    let i = match e.stream {
                        Stream::Sbs => 0,
                        Stream::Ais => 1,
                        Stream::Json => 2,
                    };
                    batch[i].push_str(&e.line);
                }
                for (i, b) in batch.into_iter().enumerate() {
                    if !b.is_empty() {
                        let _ = senders[i].send(Arc::new(b));
                    }
                }
            }
        })
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = stepper => {}
    }
    Ok(())
}
