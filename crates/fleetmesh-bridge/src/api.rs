//! HTTP and WebSocket API (same routes and JSON as the Python bridge, keyed by uid), plus
//! the decision layer's intents and inbox.
//!
//! Every task goes through `crate::decision`: `POST /api/tasks` (the console) is a thin
//! wrapper that creates a human intent. Its response is the tasking as before plus
//! `intent_id`, `state` and `dispatched`; an intent that needs approval answers 202 with
//! `dispatched: false` and why, one approved but held for a vehicle the link can't reach now
//! (rules 5, state `queued`) answers 202 saying so, one the rules reject answers 422 with the
//! reasons. `GET /api/inbox` lists the queue as `queued` (each with `reach_now`).
//!
//! Intents: `POST /api/intents` (the task request plus optional `constraints` and
//! `rationale`), `GET /api/intents`, `GET /api/intents/{id}`, `POST
//! /api/intents/{id}/approve|reject|cancel` (optional body `{"note": "..."}`; an approval whose
//! dispatch would stop an order in play needs `"acknowledge_supersede": true`), `GET
//! /api/inbox`, `GET /api/operators`, `GET /api/policy` (the policy in force, as MCP
//! `get_policy` serves it, with `rules_version`: read-only, no identity needed). Who acts is
//! `crate::auth`'s answer: the operator signed in on this browser (the session cookie; `POST
//! /api/auth/login`), or in a dev run (`FLEETMESH_AUTH=dev`) the `X-FleetMesh-Operator` header. An
//! approval the decision marks consequential carries `"password"`, the signer's own, checked
//! again before it is signed (rules 8, a step-up). The WebSocket also sends
//! `{"type": "decisions", "inbox": ..., "intents": [...]}` on connect and whenever they change
//! (existing clients ignore message types they don't know).
//!
//! Predicted link coverage (`crate::coverage`, information only): `GET /api/links/coverage`
//! (`class` or `uid`, `alt`, `bbox`, `res`: a grid; or `point`) and `POST /api/links/coverage`
//! (the same as JSON, a `route`, an `order` as the dry run takes it, or `queries`); the dry run
//! carries the order's answer as `link_coverage`.
//!
//! Operator groups (`crate::groups`) and the watch officer's tasking (`crate::tasking`),
//! conveniences an operator keeps, never decision inputs and never logged: `GET /api/groups`,
//! `POST /api/groups`, `PATCH|DELETE /api/groups/{id}`; `GET /api/tasking`, `POST /api/tasking`,
//! `POST /api/tasking/{id}/deactivate`. Anyone reads them; only an operator writes them (an
//! agent's request, by its id header or a key, is refused 403), with no step-up. The one
//! exception is the watch officer's acknowledgement, `POST /api/tasking/{id}/ack` (an agent only:
//! an operator's request is refused 403), which moves an item's delivery state forward (seen,
//! processed) and nothing else; and a read of `GET /api/tasking` by an agent is remembered, so the
//! console can say whether an officer is connected. Neither is logged or read by a decision. The
//! officer's other writes to these stores, a group made for a tasking item and a reply to one, go
//! through its own MCP tools only (`crate::mcp::tools`: `make_group`, `reply_tasking`), never
//! these routes; each group says who made it (`by`) and each reply which agent wrote it.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use fleetmesh_decide::OperatorView;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use crate::clock;
use crate::decision::{Decision, DecisionError, IntentRequest, Outcome};
use crate::platforms;
use crate::push::{self, ClientView, FrameCache, LodSettings, PushFrame, Subscription};
use crate::tasks::TaskRequest;
use crate::world::World;

#[derive(Clone)]
pub struct App {
    pub world: Arc<World>,
    pub frames: Arc<watch::Sender<Option<Arc<PushFrame>>>>,
    pub decision: Arc<Decision>,
    /// the console feed's level-of-detail limits (`/ws?lod=1`, see `push`)
    pub lod: LodSettings,
    /// Who is acting (rules 8): logins, step-ups and agent keys, or dev-mode headers.
    pub auth: Arc<crate::auth::Auth>,
    /// operator groups: non-decision, in memory, never logged
    pub groups: Arc<RwLock<crate::groups::GroupStore>>,
    /// operator guidance for the live watch officer: non-decision, in memory, never logged
    pub tasking: Arc<RwLock<crate::tasking::TaskingStore>>,
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/api/state", get(state))
        .route("/api/weather", get(weather))
        .route("/api/weather/grid", get(weather_grid))
        .route("/api/tasks", post(tasks))
        .route("/api/intents", post(create_intent).get(list_intents))
        .route("/api/intents/preview", post(preview_intent))
        .route("/api/intents/{id}", get(get_intent))
        .route("/api/intents/{id}/{action}", post(intent_action))
        .route("/api/inbox", get(inbox))
        .route("/api/triage", get(triage))
        .route("/api/swarms", get(swarms))
        .route("/api/parcels", get(parcels))
        .route("/api/hubs", get(hubs))
        .route("/api/fleet", get(fleet))
        .route("/api/places", get(places))
        .route("/api/nav", get(nav))
        .route("/api/traffic", get(traffic))
        .route("/api/traffic/sensors", get(traffic_sensors))
        .route("/api/traffic/truth", get(traffic_truth))
        .route("/api/groups", get(groups_list).post(groups_create))
        .route("/api/tasking", get(tasking_list).post(tasking_post))
        .route("/api/tasking/{id}/deactivate", post(tasking_deactivate))
        .route("/api/tasking/{id}/ack", post(tasking_ack))
        .route("/api/groups/{id}", axum::routing::patch(groups_patch).delete(groups_delete))
        // one of ours silent: confirmed lost, its loss revoked, a silent one flagged by hand
        .route("/api/nodes", get(nodes))
        .route("/api/review/nodes/{uid}/lost", post(node_lost))
        .route("/api/review/nodes/{uid}/revoke", post(node_revoke))
        .route("/api/review/nodes/{uid}/flag", post(node_flag))
        .route("/api/review/nodes/{uid}/dismiss", post(node_dismiss))
        .route("/api/operator/view", post(operator_view))
        .route("/api/operators", get(operators))
        .route("/api/policy", get(policy))
        .route("/api/platforms", get(platforms))
        .route("/api/stats", get(stats))
        .route("/api/links/coverage", get(coverage_get).post(coverage_post))
        .route("/ws", get(ws))
        .merge(crate::auth::router(app.auth.clone()))
        .with_state(app)
}

fn error(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

fn decision_error(e: DecisionError) -> Response {
    let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut body = json!({ "detail": e.detail });
    if let Some(i) = e.intent {
        body["intent_id"] = i["id"].clone();
        body["state"] = i["state"].clone();
        body["intent"] = i;
    }
    (status, Json(body)).into_response()
}

fn raw_json(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// `GET /api/parcels`: the run's parcels and their states (dispatch). Read-only.
async fn parcels(State(app): State<App>) -> Response {
    Json(app.decision.parcels_json()).into_response()
}

/// `GET /api/hubs`: the hubs with the charged packs each has now. Read-only.
async fn hubs(State(app): State<App>) -> Response {
    Json(app.decision.hubs_json()).into_response()
}

/// `GET /api/fleet`: the fleet by home base, each vehicle's operational state and why
/// (docs/design/fleet-organisation.md). Read-only.
async fn fleet(State(app): State<App>) -> Response {
    Json(app.decision.fleet_json()).into_response()
}

/// `GET /api/places`: hubs with their packs, drop zones with the latest look at each. Read-only.
async fn places(State(app): State<App>) -> Response {
    Json(app.decision.places_json()).into_response()
}

/// `GET /api/nav`: each vehicle's navigation trust, and what the monitor did for it. Read-only.
async fn nav(State(app): State<App>) -> Response {
    Json(app.decision.nav_json()).into_response()
}

/// Full state, the same message a WebSocket client gets first.
/// `?bbox=west,south,east,north`: only the tracks in (or near) that view.
#[derive(Deserialize)]
struct TrafficQuery {
    #[serde(default)]
    bbox: Option<String>,
}

fn bbox(q: Option<&str>) -> Result<Option<[f64; 4]>, Box<Response>> {
    let Some(b) = q else { return Ok(None) };
    let v: Result<Vec<f64>, _> = b.split(',').map(|x| x.trim().parse::<f64>()).collect();
    match v.ok().and_then(|v| <[f64; 4]>::try_from(v).ok()) {
        Some([w, s, e, n]) if [w, s, e, n].iter().all(|x| x.is_finite()) && s <= n => Ok(Some([w, s, e, n])),
        _ => Err(Box::new((StatusCode::BAD_REQUEST, Json(json!({"error": "bbox must be west,south,east,north"}))).into_response())),
    }
}

/// `GET /api/traffic` (docs/design/traffic.md section 5): the company's picture of the traffic
/// the fleet does not control, `{"t", "tracks": [...], "disagreements": [...], "picture":
/// {"state", "reasons", "feeds", ...}, "automation": [...]}`. Read-only; 404 when the run has no
/// picture (no traffic source, or none delivered yet).
async fn traffic(State(app): State<App>, Query(q): Query<TrafficQuery>) -> Response {
    if !app.decision.traffic().active() || app.decision.traffic().built().is_none() {
        return error(StatusCode::NOT_FOUND, "no traffic picture in this run (no traffic source, FLEETMESH_TRAFFIC_URL, or it has delivered nothing yet)");
    }
    match bbox(q.bbox.as_deref()) {
        Ok(b) => Json(app.decision.traffic_json(b)).into_response(),
        Err(r) => *r,
    }
}

/// `GET /api/traffic/sensors`: the receivers and sites the picture is built from.
async fn traffic_sensors(State(app): State<App>) -> Response {
    Json(app.decision.traffic().sensors_json()).into_response()
}

/// `GET /api/traffic/truth`: the simulator's truth, for evaluation and a debug overlay, in a dev
/// run only (404 in a credentials run, and when the run has no simulator).
async fn traffic_truth(State(app): State<App>) -> Response {
    match app.decision.traffic_truth_json().await {
        Some(v) => Json(v).into_response(),
        None => error(StatusCode::NOT_FOUND, "no simulator truth here: it is served in a dev run with a traffic simulator only"),
    }
}

async fn state(State(app): State<App>) -> Response {
    raw_json(push::state_json(&push::build_frame(
        &app.world,
        &mut FrameCache::default(),
    )))
}

fn last_reason(intent: &Value) -> String {
    intent["history"]
        .as_array()
        .and_then(|h| h.last())
        .and_then(|h| h["reason"].as_str())
        .unwrap_or("")
        .to_string()
}

/// The console's endpoint: a human intent, answered in the tasking shape it always had.
async fn tasks(
    State(app): State<App>,
    headers: HeaderMap,
    body: Result<Json<TaskRequest>, JsonRejection>,
) -> Response {
    let req = match body {
        Ok(Json(req)) => req,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.body_text()),
    };
    let op = match app.auth.actor(&headers, true) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    let task = req.task.clone();
    let params = serde_json::to_value(&req.params).unwrap_or(Value::Null);
    let Outcome {
        id,
        state,
        intent,
        tasking,
        authorization,
        ..
    } = match app.decision.submit(&op, IntentRequest::from(req)).await {
        Ok(o) => o,
        Err(e) => return decision_error(e),
    };
    let state_name = serde_json::to_value(state).unwrap_or(Value::Null);
    if let Some(t) = tasking {
        let mut v = serde_json::to_value(t).unwrap_or(Value::Null);
        v["intent_id"] = json!(id.0);
        v["state"] = state_name;
        v["dispatched"] = json!(true);
        // A human-tier task the operator authorized by sending it: say which tier and why.
        if let Some(a) = authorization {
            v["authorization"] = a;
        }
        return Json(v).into_response();
    }
    let reason = last_reason(&intent);
    let (status, detail) = if state == fleetmesh_decide::State::PendingApproval {
        (
            StatusCode::ACCEPTED,
            format!("not dispatched: waiting for an operator to approve intent {id} ({reason})"),
        )
    } else if state == fleetmesh_decide::State::Queued {
        // rules 5: approved, held for a vehicle the link can't carry it to now
        (
            StatusCode::ACCEPTED,
            format!(
                "queued: intent {id} is approved and goes when the link can carry it, re-checked first ({reason})"
            ),
        )
    } else {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "not dispatched: intent {id} {}: {reason}",
                state_name.as_str().unwrap_or("?")
            ),
        )
    };
    let body = json!({
        "id": null,
        "intent_id": id.0,
        "state": state_name,
        "dispatched": false,
        "detail": detail,
        "task": task,
        "params": params,
        "ops": [],
        "expires_t": intent["expires_t"].clone(),
        "intent": intent,
    });
    (status, Json(body)).into_response()
}

async fn create_intent(
    State(app): State<App>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Response {
    let mut v = match body {
        Ok(Json(v)) => v,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.body_text()),
    };
    let password = v
        .as_object_mut()
        .and_then(|o| o.remove("password"))
        .and_then(|p| p.as_str().map(str::to_string));
    // rules 10: or a passkey's assertion (made for these acceptances)
    let passkey = v.as_object_mut().and_then(|o| o.remove("passkey"));
    let proof = crate::auth::Proof { password, passkey };
    let req: IntentRequest = match serde_json::from_value(v) {
        Ok(r) => r,
        Err(e) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Failed to deserialize the JSON body into the target type: {e}"),
            );
        }
    };
    let op = match app.auth.actor(&headers, false) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    if proof.is_some() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "a password or passkey goes with an approval (a step-up) only",
        );
    }
    match app.decision.submit_stepped(&op, req, None).await {
        Ok(o) => (
            StatusCode::CREATED,
            Json(json!({
                "dispatched": o.dispatched(),
                "intent": o.intent,
                "tasking": o.tasking,
                "space": o.space,
                "authorization": o.authorization,
            })),
        )
            .into_response(),
        Err(e) => decision_error(e),
    }
}

/// `POST /api/intents/preview`: an operator's dry run of a vehicle task (the checks and the
/// policy decision now, and whether the submission would be its own approval). Nothing is
/// created or logged, and it does not keep a session alive (it is not an act).
async fn preview_intent(
    State(app): State<App>,
    headers: HeaderMap,
    body: Result<Json<IntentRequest>, JsonRejection>,
) -> Response {
    let req = match body {
        Ok(Json(req)) => req,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.body_text()),
    };
    let op = match app.auth.actor_peek(&headers) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    match app.decision.operator_dry_run(&op, req).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => decision_error(e),
    }
}

/// `GET /api/links/coverage`: predicted link coverage (`crate::coverage`), read-only.
async fn coverage_get(
    State(app): State<App>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let q = crate::coverage::from_query_string(&params);
    match crate::coverage::query(&app.world, &app.decision, &q).await {
        Ok(v) => Json(v).into_response(),
        Err((status, detail)) => error(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST), detail),
    }
}

/// `POST /api/links/coverage`: the same as JSON, and routes, orders and several queries.
async fn coverage_post(State(app): State<App>, body: Result<Json<Value>, JsonRejection>) -> Response {
    let q = match body {
        Ok(Json(q)) => q,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.body_text()),
    };
    match crate::coverage::query(&app.world, &app.decision, &q).await {
        Ok(v) => Json(v).into_response(),
        Err((status, detail)) => error(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST), detail),
    }
}

async fn list_intents(State(app): State<App>) -> Response {
    Json(app.decision.intents_json()).into_response()
}

async fn get_intent(State(app): State<App>, Path(id): Path<u64>) -> Response {
    match app.decision.intent_json(id) {
        Some(v) => Json(v).into_response(),
        None => error(StatusCode::NOT_FOUND, format!("unknown intent {id}")),
    }
}

async fn intent_action(
    State(app): State<App>,
    Path((id, action)): Path<(u64, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // rules 8: an approval may carry the approver's password again (a step-up); it is
    // checked and dropped here, never logged or echoed; rules 10: or a passkey's assertion
    // made for this act (`passkey`), which is logged
    let (note, token, acknowledge, password, passkey) = if body.iter().all(u8::is_ascii_whitespace) {
        (None, None, false, None, None)
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) => {
                let token = match v.get("token").filter(|s| !s.is_null()) {
                    None => None,
                    Some(t) => match t.as_u64() {
                        Some(t) => Some(t),
                        None => {
                            return error(StatusCode::UNPROCESSABLE_ENTITY, "token: a picture token id".to_string());
                        }
                    },
                };
                (
                    v["note"].as_str().map(str::to_string),
                    token,
                    v["acknowledge_supersede"].as_bool().unwrap_or(false),
                    v["password"].as_str().map(str::to_string),
                    v.get("passkey").filter(|p| !p.is_null()).cloned(),
                )
            }
            Err(_) => return error(StatusCode::UNPROCESSABLE_ENTITY, "body: not JSON"),
        }
    };
    let proof = crate::auth::Proof { password, passkey };
    let op = match app.auth.actor(&headers, false) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    if proof.is_some() && action != "approve" {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "a password or passkey goes with an approval (a step-up) only",
        );
    }
    let result = match action.as_str() {
        "approve" => {
            // a consequential signature re-enters the credential first (credentials run)
            let needed = app.decision.needs_step_up(id);
            let step = match app.auth.step_up(&op, &format!("i{id}"), proof, needed).await {
                Ok(s) => s,
                Err(r) => return *r,
            };
            app.decision.approve_stepped(&op, id, note, token, acknowledge, step).await
        }
        "reject" => app.decision.reject(&op, id, note),
        "cancel" => app.decision.cancel(&op, id, note),
        _ => {
            return error(
                StatusCode::NOT_FOUND,
                format!(
                    "unknown action {action:?} (approve, reject, cancel)"
                ),
            );
        }
    };
    match result {
        Ok(o) => Json(json!({
            "dispatched": o.dispatched(),
            "intent": o.intent,
            "tasking": o.tasking,
            "space": o.space,
        }))
        .into_response(),
        Err(e) => decision_error(e),
    }
}

async fn inbox(State(app): State<App>) -> Response {
    Json(app.decision.inbox_json()).into_response()
}

/// `GET /api/triage` (rules 7, docs/design/watch-officer.md): every item the AI watch officer
/// triaged this run, with its verdict, note and recommendation: the person's audit of what the
/// officer handled. Read-only; the audit log's `triage.*` events are the record.
async fn triage(State(app): State<App>) -> Response {
    Json(app.decision.triage_json()).into_response()
}

/// `GET /api/swarms` (murmur-z07i): each swarm the fleet manifest names, with its members'
/// readiness, lost links, low batteries, centroid, hull and tasks (`crate::swarms`), made at
/// most once a second. Read-only, never logged.
async fn swarms(State(app): State<App>) -> Response {
    let v = crate::swarms::api_json(&app.world, app.decision.reserve_pct(), || app.decision.group_views());
    Json(v.as_ref().clone()).into_response()
}

/// `POST /api/operator/view` (Phase G): the console says what it showed for a pending
/// intent. `{"intent_id": n, "event": "open"|"close"|"decide", "action": "approve"|"reject"
/// (decide only), "shown": {"checks": [rule ids], "warnings": [lines], "rationale": bool},
/// "open_ms": n, "client_t": ms}`. Logged as `operator.view` under
/// the operator's session; it decides nothing. The operator header is required (no default
/// operator): a display event attributed to someone who did not send it would corrupt the
/// measurement.
#[derive(Deserialize)]
struct ViewBody {
    intent_id: u64,
    #[serde(flatten)]
    view: OperatorView,
}

async fn operator_view(
    State(app): State<App>,
    headers: HeaderMap,
    body: Result<Json<ViewBody>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(b) => b,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, e.body_text()),
    };
    // a display report is not an operator's action: it does not keep the session alive
    let op = match app.auth.actor_peek(&headers) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    match app.decision.record_view(&op, body.intent_id, body.view) {
        Ok(()) => Json(json!({ "ok": true, "intent_id": body.intent_id })).into_response(),
        Err(e) => decision_error(e),
    }
}



/// The operators the bridge accepts, those with consequential authority, and how they are
/// authenticated (`credentials`: the credential file; `dev`: `FLEETMESH_OPERATORS`,
/// `FLEETMESH_CONSEQUENTIAL_OPERATORS`, asserted by header).
async fn operators(State(app): State<App>) -> Response {
    Json(app.decision.operators_json()).into_response()
}

async fn platforms(State(app): State<App>) -> Response {
    let world = app.world.clone();
    // rules 34: transit beside patrol where the run's rules take one
    let transit = true;
    match tokio::task::spawn_blocking(move || platforms::catalog(&world, transit)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Per-vehicle packet counters (for load tests; kept out of the UI stream), and the GCS
/// heartbeat's jitter (`crate::heartbeat`).
async fn stats(State(app): State<App>) -> Response {
    let vehicles: Map<String, Value> = app
        .world
        .vehicles()
        .iter()
        .map(|v| {
            (
                v.uid.to_string(),
                serde_json::to_value(v.link_counters()).unwrap_or(Value::Null),
            )
        })
        .collect();
    // the heartbeat thread's jitter: worst send delay and longest gap per minute
    let heartbeat = crate::heartbeat::stats().map_or(Value::Null, |s| s.json());
    // how long the periodic work takes and how regularly it runs (`crate::perf`)
    let loops = crate::perf::json();
    Json(json!({ "t": clock::wall(), "vehicles": vehicles, "gcs_heartbeat": heartbeat, "loops": loops }))
        .into_response()
}

/// `/ws` is the feed as it always was; `/ws?lod=1` asks for level of detail by the view
/// the client then sends (`{"type":"view",...}` messages, see `push`).
#[derive(Deserialize)]
struct WsQuery {
    #[serde(default)]
    lod: Option<String>,
}

async fn ws(State(app): State<App>, Query(q): Query<WsQuery>, upgrade: WebSocketUpgrade) -> Response {
    let lod = matches!(q.lod.as_deref(), Some("1" | "true" | "yes"));
    upgrade.on_upgrade(move |socket| client(socket, app, lod))
}

async fn client(mut socket: WebSocket, app: App, lod: bool) {
    let mut frames = app.frames.subscribe();
    let mut view = ClientView::new(lod.then_some(app.lod));
    let full = view.full_state(&push::build_frame(&app.world, &mut FrameCache::default()));
    if socket.send(Message::Text(full.into())).await.is_err() {
        return;
    }
    // the inbox and intents: a message type of its own, after the state
    let mut decisions = app.decision.subscribe();
    let first = decisions.borrow_and_update().clone();
    if socket
        .send(Message::Text(first.as_str().into()))
        .await
        .is_err()
    {
        return;
    }
    // the traffic picture, about once a second, cut to the client's view when it sends one
    let mut traffic = app.decision.traffic().subscribe();
    let mut traffic_bounds: Option<[f64; 4]> = None;
    loop {
        tokio::select! {
            changed = traffic.changed() => {
                if changed.is_err() {
                    break;
                }
                let has = traffic.borrow_and_update().is_some();
                if !has {
                    continue;
                }
                let mut msg = app.decision.traffic_json(traffic_bounds);
                if let Some(o) = msg.as_object_mut() {
                    o.insert("type".into(), json!("traffic"));
                }
                if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                    break;
                }
            }
            changed = decisions.changed() => {
                if changed.is_err() {
                    break;
                }
                let msg = decisions.borrow_and_update().clone();
                if socket.send(Message::Text(msg.as_str().into())).await.is_err() {
                    break;
                }
            }
            changed = frames.changed() => {
                if changed.is_err() {
                    break;
                }
                let frame = frames.borrow_and_update().clone();
                let Some(frame) = frame else { continue };
                if socket.send(Message::Text(view.delta(&frame).into())).await.is_err() {
                    break;
                }
            }
            // tasks go over REST; the only message a client sends here is its view, which
            // gets a delta at once so a pan does not wait for the next tick
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(text))) => {
                    if let Some(sub) = Subscription::parse(&text) {
                        traffic_bounds = sub.bounds.filter(|b| b.iter().all(|x| x.is_finite()));
                        view.subscribe(sub);
                        let frame = frames.borrow().clone();
                        if let Some(frame) = frame
                            && socket.send(Message::Text(view.delta(&frame).into())).await.is_err()
                        {
                            break;
                        }
                    }
                }
                Some(Ok(_)) => {}
            }
        }
    }
}

#[derive(Deserialize)]
struct WeatherQuery {
    lat: f64,
    lon: f64,
}
async fn weather(State(app): State<App>, Query(q): Query<WeatherQuery>) -> Response {
    if !fleetmesh_weather::valid_point(q.lat, q.lon) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "lat/lon out of range"})),
        )
            .into_response();
    }
    let mut body = app.decision.weather_point_json(q.lat, q.lon).unwrap_or_else(
        || json!({"available": false, "lat": q.lat, "lon": q.lon, "note": "No scenario weather in this run"}),
    );
    // the scenario clock beside the conditions it is the time of (bd murmur-iobd.5): the
    // console's top bar shows the world's time from it, never the browser's. Display only.
    body["scenario"] = clock::console_clock().map_json();
    Json(body).into_response()
}


/// `GET /api/policy` (read-only, no identity needed): the policy in force as MCP `get_policy`
/// them: `Decision::policy_api_json`).
async fn policy(State(app): State<App>) -> Response {
    Json(app.decision.policy_api_json()).into_response()
}

#[derive(Deserialize)]
struct WeatherGridQuery { bbox: String }
async fn weather_grid(State(app): State<App>, Query(q): Query<WeatherGridQuery>) -> Response {
    let bounds = q.bbox.split(',').map(str::parse::<f64>).collect::<Result<Vec<_>,_>>();
    if let Ok(b) = bounds && let Ok([w,s,e,n]) = <[f64;4]>::try_from(b)
        && fleetmesh_weather::valid_point(s,w) && fleetmesh_weather::valid_point(n,e) && w < e && s < n
    {
        // the same weather the rules read (the scenario's sampler), not a separate feed
        return match app.decision.weather_grid_json([w, s, e, n]) {
            Some(grid) => Json(grid).into_response(),
            None => Json(json!({"available": false, "note": "No scenario weather in this run"})).into_response(),
        };
    }
    (StatusCode::BAD_REQUEST, Json(json!({"error": "bbox must be west,south,east,north within lat/lon bounds"}))).into_response()
}

// ---- operator groups: non-decision, never logged ----

fn now_ms() -> u64 {
    (clock::wall() * 1000.0) as u64
}

/// Who writes an operator-owned convenience store (groups, tasking): an operator, never an
/// agent (403, whatever operator it names), and no step-up since nothing here is a decision.
fn store_writer(app: &App, headers: &HeaderMap) -> Result<crate::decision::Actor, Box<Response>> {
    // as the node-loss routes tell an agent: its id header, or any key (an operator's request
    // carries a session cookie or, in a dev run, the operator header, never Authorization)
    if headers.contains_key(crate::mcp::AGENT_HEADER) || headers.contains_key(header::AUTHORIZATION) {
        return Err(Box::new(error(StatusCode::FORBIDDEN, "an agent may read this, only an operator writes it")));
    }
    app.auth.actor(headers, false).map_err(|e| Box::new(decision_error(e)))
}

fn known_uids(app: &App) -> HashSet<crate::vehicle::Uid> {
    app.world.vehicles().iter().map(|v| v.uid).collect()
}

fn group_error(e: crate::groups::GroupError) -> Response {
    error(StatusCode::BAD_REQUEST, e.to_string())
}

/// `GET /api/groups`: the operator groups. Read-only, no identity needed.
async fn groups_list(State(app): State<App>) -> Response {
    let groups = app.groups.read().unwrap_or_else(|e| e.into_inner()).list();
    Json(json!({ "t": now_ms(), "groups": groups })).into_response()
}

#[derive(Deserialize)]
struct GroupCreate {
    name: String,
    #[serde(default)]
    members: Vec<crate::vehicle::Uid>,
    #[serde(default)]
    color: Option<String>,
}

/// `POST /api/groups`: 201 with the group and any `dropped` uids (not in the fleet).
async fn groups_create(State(app): State<App>, headers: HeaderMap, body: Result<Json<GroupCreate>, JsonRejection>) -> Response {
    let op = match store_writer(&app, &headers) {
        Ok(op) => op,
        Err(r) => return *r,
    };
    let req = match body {
        Ok(Json(r)) => r,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let known = known_uids(&app);
    let made = app
        .groups
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .create(&req.name, &req.members, req.color, &known, now_ms(), op.operator().as_str());
    match made {
        Ok((g, dropped)) => {
            let mut v = serde_json::to_value(g).unwrap_or(Value::Null);
            v["dropped"] = json!(dropped);
            (StatusCode::CREATED, Json(v)).into_response()
        }
        Err(e) => group_error(e),
    }
}

fn uid_list(v: Option<&Value>) -> Result<Option<Vec<crate::vehicle::Uid>>, &'static str> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|u| u.as_u64().ok_or("uids are integers"))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err("uids are a list of integers"),
    }
}

/// `PATCH /api/groups/{id}`: `name`, `color` (null clears), `add`, `remove`, `members`. Any group,
/// one the watch officer made included: the operator's change takes it from the officer
/// (`changed_by`), which may change it no more.
async fn groups_patch(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap, body: Result<Json<Map<String, Value>>, JsonRejection>) -> Response {
    let op = match store_writer(&app, &headers) {
        Ok(op) => op,
        Err(r) => return *r,
    };
    let b = match body {
        Ok(Json(b)) => b,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let name = match b.get("name") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return error(StatusCode::BAD_REQUEST, "a name is text"),
    };
    let color = match b.get("color") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => return error(StatusCode::BAD_REQUEST, "a colour is text or null"),
    };
    let (add, remove, members) = match (uid_list(b.get("add")), uid_list(b.get("remove")), uid_list(b.get("members"))) {
        (Ok(a), Ok(r), Ok(m)) => (a.unwrap_or_default(), r.unwrap_or_default(), m),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return error(StatusCode::BAD_REQUEST, r),
    };
    let known = known_uids(&app);
    let patch = crate::groups::Patch { name, color, add, remove, members };
    let done = app.groups.write().unwrap_or_else(|e| e.into_inner()).patch(&id, patch, &known, now_ms(), op.operator().as_str());
    match done {
        Ok(Some((g, dropped))) => {
            let mut v = serde_json::to_value(g).unwrap_or(Value::Null);
            v["dropped"] = json!(dropped);
            Json(v).into_response()
        }
        Ok(None) => error(StatusCode::NOT_FOUND, format!("no group {id}")),
        Err(e) => group_error(e),
    }
}

/// `DELETE /api/groups/{id}`: 204, or 404.
async fn groups_delete(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Err(r) = store_writer(&app, &headers) {
        return *r;
    }
    if app.groups.write().unwrap_or_else(|e| e.into_inner()).delete(&id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        error(StatusCode::NOT_FOUND, format!("no group {id}"))
    }
}

// ---- operator tasking for the watch officer: guidance, not a decision ----

/// `GET /api/tasking`: every item, newest first (the officer sends only the active ones), with
/// how far the officer has each and its replies, `officer`: the agent that last read it and when
/// (null: none has), and `officer_replies` (this bridge offers the officer `reply_tasking`, so an
/// item it had and has not answered can be shown as such; false where it does not). No identity needed; a read that names an agent
/// (its id header or key, as `/mcp` takes them) is remembered as the officer's last read, one that
/// names none or a wrong one is served all the same.
async fn tasking_list(State(app): State<App>, headers: HeaderMap) -> Response {
    let now = now_ms();
    let mut store = app.tasking.write().unwrap_or_else(|e| e.into_inner());
    if let Ok(Some(agent)) = app.auth.agent_of(&headers) {
        store.read_by(agent.as_str(), now);
    }
    let officer = store.reader().map(|(agent, t)| json!({ "agent": agent, "read_t": t }));
    Json(json!({ "t": now, "items": store.list(), "officer": officer, "officer_replies": store.replies_kept() })).into_response()
}

#[derive(Deserialize)]
struct TaskingPost {
    text: String,
}

/// `POST /api/tasking`: 201 the item; 400 on empty or over-long text; 403 for an agent.
async fn tasking_post(State(app): State<App>, headers: HeaderMap, body: Result<Json<TaskingPost>, JsonRejection>) -> Response {
    let op = match store_writer(&app, &headers) {
        Ok(op) => op,
        Err(r) => return *r,
    };
    let req = match body {
        Ok(Json(r)) => r,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let made = app
        .tasking
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .post(&req.text, op.operator().as_str(), now_ms());
    match made {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskingAck {
    state: crate::tasking::AckState,
    #[serde(default)]
    wake: Option<u64>,
    #[serde(default)]
    why: Option<String>,
}

/// `POST /api/tasking/{id}/ack` `{state: waiting|seen|processed|failed, wake, why}`: the watch
/// officer says how far it has an item (`crate::tasking`): 200 the item; 403 for a request that
/// names no agent (an operator's); 400 for an unknown state, a field other than these (an
/// acknowledgement never carries text or `active`), or a state without its `wake` or `why`; 404
/// for an unknown id; 409 for a state that would move the item back. Display only: never logged.
async fn tasking_ack(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap, body: Result<Json<TaskingAck>, JsonRejection>) -> Response {
    let agent = match app.auth.agent_of(&headers) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error(
                StatusCode::FORBIDDEN,
                "only the watch officer, an agent, acknowledges tasking (its X-FleetMesh-Agent header in a dev run, else its key)",
            );
        }
        Err(e) => return decision_error(e),
    };
    let req = match body {
        Ok(Json(r)) => r,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.body_text()),
    };
    let ack = crate::tasking::Ack { state: req.state, wake: req.wake, why: req.why.as_deref(), agent: agent.as_str() };
    match app.tasking.write().unwrap_or_else(|e| e.into_inner()).ack(&id, ack, now_ms()) {
        Ok(item) => Json(item).into_response(),
        Err(crate::tasking::AckError::Unknown) => error(StatusCode::NOT_FOUND, format!("no tasking {id}")),
        Err(crate::tasking::AckError::Invalid(why)) => error(StatusCode::BAD_REQUEST, why),
        Err(crate::tasking::AckError::Behind(why)) => error(StatusCode::CONFLICT, why),
    }
}

/// `POST /api/tasking/{id}/deactivate`: retire guidance that no longer applies.
async fn tasking_deactivate(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Err(r) = store_writer(&app, &headers) {
        return *r;
    }
    match app.tasking.write().unwrap_or_else(|e| e.into_inner()).deactivate(&id) {
        Some(item) => Json(item).into_response(),
        None => error(StatusCode::NOT_FOUND, format!("no tasking {id}")),
    }
}

// ---- one of ours lost: a person's decision ----

/// What the node routes answer an agent: 403 for a request that comes as an agent (its id header,
/// or a key). An agent never confirms, revokes, flags or dismisses a loss, and has no MCP tool for
/// any of them. None: an operator's request, to be read on.
fn node_route_refusal(headers: &HeaderMap) -> Option<Response> {
    if headers.contains_key(crate::mcp::AGENT_HEADER) || headers.contains_key(header::AUTHORIZATION) {
        return Some(error(
            StatusCode::FORBIDDEN,
            "an agent never confirms, revokes or flags the loss of one of ours: an operator decides it, at the console",
        ));
    }
    None
}

/// `GET /api/nodes`: each of ours not reporting (silent, presumed lost, lost) with the region it
/// can be in (read-only, never logged).
async fn nodes(State(app): State<App>) -> Response {
    match app.decision.nodes_json() {
        Ok(v) => Json(v).into_response(),
        Err(e) => decision_error(e),
    }
}

/// `POST /api/review/nodes/{uid}/lost` `{since, note, password | passkey}`: an operator the roster
/// allows confirms one of ours lost on the silence the review showed (`since`: the engine's
/// milliseconds of its last position report; null: it never reported one), saying why where
/// nothing presumes it lost. In that order: 403 for an agent or an operator without the authority,
/// 409 where the rules refuse it now (both before any credential is asked for), then in a
/// credentials run 401 without a valid step-up. Logged as `node.lost`, then what it ends.
async fn node_lost(State(app): State<App>, Path(uid): Path<u64>, headers: HeaderMap, body: Bytes) -> Response {
    #[derive(Deserialize)]
    struct NodeLost {
        #[serde(default)]
        since: Option<i64>,
        #[serde(default)]
        note: String,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        passkey: Option<Value>,
    }
    if let Some(r) = node_route_refusal(&headers) {
        return r;
    }
    let req = match serde_json::from_slice::<NodeLost>(&body) {
        Ok(r) => r,
        Err(e) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("lost wants {{\"since\": ms or null, \"note\": \"why\"}}: {e}"),
            );
        }
    };
    let op = match app.auth.actor(&headers, false) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    let since = req.since.map(fleetmesh_decide::Time);
    // only the roster's authority confirms, and only what the rules take now: both said before any
    // credential is asked for (a refused confirmation never takes, nor counts, a password)
    if let Err(e) = app.decision.node_lost_check(&op, uid, since, &req.note) {
        return decision_error(e);
    }
    let proof = crate::auth::Proof {
        password: req.password,
        passkey: req.passkey.filter(|p| !p.is_null()),
    };
    let step = match app.auth.step_up(&op, &format!("confirm vehicle {uid} lost"), proof, true).await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    match app.decision.confirm_node_lost(&op, uid, since, &req.note, step) {
        Ok(v) => Json(v).into_response(),
        Err(e) => decision_error(e),
    }
}

#[derive(Deserialize)]
struct NodeNote {
    #[serde(default)]
    note: String,
}

/// A node route's note (an empty body: none), or its 422.
fn node_note(body: &[u8], what: &str) -> Result<String, Box<Response>> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(String::new());
    }
    serde_json::from_slice::<NodeNote>(body)
        .map(|b| b.note)
        .map_err(|e| Box::new(error(StatusCode::UNPROCESSABLE_ENTITY, format!("{what} wants {{\"note\": \"why\"}}: {e}"))))
}

/// `POST /api/review/nodes/{uid}/revoke` `{note}`: any operator revokes the loss of one of ours,
/// saying why (it only restricts). No step-up. 409 without a note or for a vehicle not lost.
async fn node_revoke(State(app): State<App>, Path(uid): Path<u64>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(r) = node_route_refusal(&headers) {
        return r;
    }
    let note = match node_note(&body, "revoke") {
        Ok(n) => n,
        Err(r) => return *r,
    };
    let op = match app.auth.actor(&headers, false) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    match app.decision.revoke_node_lost(&op, uid, &note) {
        Ok(v) => Json(v).into_response(),
        Err(e) => decision_error(e),
    }
}

/// `POST /api/review/nodes/{uid}/flag` `{note}`: any operator puts a silent vehicle on the review
/// list by hand, saying why; it decides nothing. 409 for a vehicle that is reporting or lost.
async fn node_flag(State(app): State<App>, Path(uid): Path<u64>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(r) = node_route_refusal(&headers) {
        return r;
    }
    let note = match node_note(&body, "flag") {
        Ok(n) => n,
        Err(r) => return *r,
    };
    let op = match app.auth.actor(&headers, false) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    match app.decision.flag_node(&op, uid, &note) {
        Ok(v) => Json(v).into_response(),
        Err(e) => decision_error(e),
    }
}

/// `POST /api/review/nodes/{uid}/dismiss` `{note}`: any operator takes a hand flag off the review
/// list.
async fn node_dismiss(State(app): State<App>, Path(uid): Path<u64>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(r) = node_route_refusal(&headers) {
        return r;
    }
    let note = match node_note(&body, "dismiss") {
        Ok(n) => n,
        Err(r) => return *r,
    };
    let op = match app.auth.actor(&headers, false) {
        Ok(op) => op,
        Err(e) => return decision_error(e),
    };
    match app.decision.dismiss_node_flag(&op, uid, &note) {
        Ok(v) => Json(v).into_response(),
        Err(e) => decision_error(e),
    }
}
