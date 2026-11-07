//! The hub: the HTTP reads this client makes and the shapes they return.
//!
//! Everything here follows the Epimemer repository's
//! `dev-docs/VIZ_CONTRACT.md`. The structs mirror the contract's field tables.
//! A field the contract defines is required, so a hub that drops one fails
//! with the field's name instead of drawing a wrong picture. A field this
//! client does not know is ignored, as the contract's versioning rule asks.
//! `schema_version` may be absent, because a hub older than the contract
//! sends none.
//!
//! The topic embeddings (VIZ_CONTRACT.md §6) are read the same way, in the
//! background, by [`embeddings`]: a hub without the route, older than the
//! contract, answers `404` from its router rather than the contract's
//! `{"error"}` body, which [`EmbeddingsError::NoRoute`] tells apart from a
//! session the hub no longer knows, which is chosen again by graph. The
//! session's retrieval records (VIZ_CONTRACT.md §4) are read by
//! [`retrievals`], in the shape the `retrieval_recorded` event carries too.
//! The graph list carries the reflect counter of the session's active
//! graph ([`Reflect`]), in the shape `reflect_counter_updated` carries it.
//!
//! The reads are futures, written once over a transport with one
//! implementation for each platform: natively a blocking read with ureq
//! (`hub/native.rs`), in a browser the page's fetch (`hub/web.rs`). The
//! first load happens before the window opens; every later read (a graph
//! switch, the session and graph lists the bar shows) runs in the
//! background (`background.rs`), so the window never waits on the hub.
//! Choosing a session is a pure function ([`choose_session`]) so it can be
//! tested without a hub.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use web_time::Instant;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native as transport;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web as transport;

/// The schema version this client was written for (VIZ_CONTRACT.md §3).
pub const SCHEMA_VERSION: u32 = 1;

/// The hub's own answer to `GET /api/health`.
const HUB_SERVICE: &str = "epimemer-viz-hub";

/// The most a snapshot may weigh. The development graph's is about 6 MB;
/// this leaves room for a graph forty times its size and still refuses an
/// answer that would exhaust memory.
const SNAPSHOT_LIMIT: u64 = 256 * 1024 * 1024;

/// The most any other answer may weigh.
const SMALL_LIMIT: u64 = 4 * 1024 * 1024;

/// The most the embeddings may weigh. The hub refuses a session's answer
/// past 16 MiB itself (VIZ_CONTRACT.md §6); this leaves room for the
/// encoding around it.
const EMBEDDINGS_LIMIT: u64 = 20 * 1024 * 1024;

/// `GET /api/health`.
#[derive(Debug, Clone, Deserialize)]
pub struct Health {
    pub service: String,
    pub version: String,
    #[serde(default)]
    pub schema_version: Option<u32>,
}

/// One entry of `GET /api/sessions`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub pid: i64,
    pub backend: String,
    pub active_graph: String,
    pub started_at: String,
    pub connected: bool,
    pub last_event_at: Option<String>,
    #[serde(default)]
    pub schema_version: Option<u32>,
}

/// `GET /api/graphs?session=`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GraphList {
    pub graphs: Vec<String>,
    pub active_graph: String,
    pub backend: String,
    /// The reflect counter of `active_graph`, and of no other graph.
    pub reflect: Reflect,
}

/// A graph's reflect counter: the stores since its last reflect, the
/// threshold at which reflect is suggested, and whether the count has
/// reached it. The graph list and `reflect_counter_updated` carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Reflect {
    pub count: i64,
    pub threshold: i64,
    pub suggested: bool,
}

/// `GET /api/snapshot?session=&graph=`, the parts this client reads so far.
/// Relation labels and boundary proposals ride along in the same document
/// and are ignored until a slice draws them.
#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub schema_version: Option<u32>,
    pub graph: String,
    pub nodes: Vec<NodeView>,
    pub edges: Vec<EdgeView>,
    pub timelines: Vec<TimelineView>,
    pub metacontexts: Vec<MetacontextView>,
}

/// The contract's TimelineView: a clock and the points on it that
/// `timelink` edges name. Its recurrences are not read yet.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TimelineView {
    pub timeline_id: String,
    pub name: String,
    pub description: String,
    pub timepoints: Vec<TimepointView>,
    /// The timeline's own "now"; `None` means the wall clock.
    pub reference_time: Option<String>,
    pub created_at: String,
    pub graph: String,
}

/// The contract's TimepointView. `kind` is kept as the hub wrote it, so
/// that a kind this version does not know can be named where it is listed
/// as having no place.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TimepointView {
    pub timepoint_id: String,
    /// Absent on a vague point.
    pub start: Option<String>,
    pub end: Option<String>,
    pub label: Option<String>,
    /// `instant`, `interval` or `vague`, derived from the dates each time.
    pub kind: String,
    /// Where the stated order puts a vague point; absent on a dated point
    /// and on a contested one.
    pub earliest: Option<String>,
    pub latest: Option<String>,
    /// The stated order cannot hold.
    pub contested: bool,
    pub temporal_contradiction_id: Option<String>,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// One end of a validity interval: an instant discriminated by
/// `instant_kind`. Only a precise instant has a coordinate. A kind this
/// version does not know reads as [`ImpreciseInstant::Other`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "instant_kind", rename_all = "lowercase")]
pub enum ImpreciseInstant {
    Precise {
        at: String,
        #[serde(default)]
        label: Option<String>,
    },
    Named {
        label: String,
    },
    /// Nobody supplied it, the default for an endpoint.
    Unknown,
    /// It runs without end.
    Unbounded,
    #[serde(other)]
    Other,
}

/// The contract's ValidityInterval: the period a source asserts a claim
/// held, half-open, `[start, end)`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ValidityInterval {
    pub start: ImpreciseInstant,
    pub end: ImpreciseInstant,
    /// The clock; `None` is the wall-clock timeline.
    pub timeline_id: Option<String>,
    /// A moment the source asserts the interval contains.
    pub witnessed_at: Option<ImpreciseInstant>,
    /// `stated` or `inferred`.
    pub basis: String,
}

/// The contract's MetacontextView: a world claims are made in, which
/// `has_metacontext` edges point at by id.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MetacontextView {
    pub metacontext_id: String,
    /// Its name.
    pub content: String,
    pub description: String,
    pub graph: String,
}

/// A node kind. `unknown` is the contract's name for a kind this version
/// does not know, and any kind added later reads as it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Topic,
    Fact,
    Inference,
    #[serde(other)]
    Unknown,
}

/// The contract's NodeView. The side panel shows every field (`panel.rs`),
/// and a test there compares its list with this struct's, serialised.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct NodeView {
    pub node_id: String,
    pub node_type: NodeType,
    pub content: String,
    pub description: String,
    pub status: String,
    pub source_id: Option<String>,
    pub extraction_method: String,
    pub confidence: Option<f64>,
    pub retrieved_at: Option<String>,
    pub created_at: String,
    pub graph: String,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// The contract's EdgeView. `src_id` and `dst_id` may name a segment, a
/// document, a metacontext or a timeline rather than a node
/// (VIZ_CONTRACT.md §5.1); the client keeps them as keys and resolves only
/// what it draws.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EdgeView {
    pub edge_id: String,
    pub src_id: String,
    pub dst_id: String,
    pub edge_type: String,
    pub weight: f64,
    /// Validity intervals, on `sourced_from` edges only; claim time draws
    /// them as brackets beside the focused node's column (DESIGN.md §4).
    pub validity: Vec<ValidityInterval>,
    pub created_at: String,
    pub graph: String,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// `GET /api/embeddings?session=&graph=&node_type=`: the stored embedding
/// vectors of a graph's active nodes of one kind.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Embeddings {
    #[serde(default)]
    pub schema_version: Option<u32>,
    pub graph: String,
    pub node_type: Option<String>,
    /// Every model present, with its vectors' length.
    pub models: BTreeMap<String, usize>,
    /// In the snapshot's node order, then by model; a node with no
    /// embedding is absent, and one embedded under two models is here twice.
    pub embeddings: Vec<EmbeddingView>,
}

/// One node's vector under one model.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EmbeddingView {
    pub node_id: String,
    pub node_type: NodeType,
    pub model_id: String,
    pub vector: Vec<f32>,
}

/// The embeddings, and what reading them took.
#[derive(Debug, Clone)]
pub struct EmbeddingsRead {
    pub embeddings: Embeddings,
    pub bytes: usize,
    pub download: Duration,
    pub parse: Duration,
    /// The session that answered.
    pub session: String,
}

/// Why the embeddings could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingsError {
    /// The hub has no embeddings route: it is older than the contract.
    NoRoute,
    /// The read failed, as described.
    Failed(String),
}

impl fmt::Display for EmbeddingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddingsError::NoRoute => f.write_str(
                "the hub has no embeddings route, so it runs an Epimemer older than the visualisation contract",
            ),
            EmbeddingsError::Failed(message) => f.write_str(message),
        }
    }
}

/// `GET /api/retrievals?session=`: the session's most recent retrieval
/// records, across every graph, oldest first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RetrievalList {
    pub records: Vec<RetrievalRecord>,
}

/// The contract's retrieval record: what one tool call handed the agent.
/// The route and the `retrieval_recorded` event carry the same shape.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RetrievalRecord {
    /// Counted from one in its session and padded to twelve digits, so its
    /// text order is its numeric order.
    pub record_id: String,
    /// When the tool answered.
    pub at: String,
    /// The tool's name, such as `epimemer.search`.
    pub tool: String,
    /// A one-line summary of the call's input, for display; empty on a hub
    /// bound beyond loopback.
    pub query: String,
    /// The session's active graph when the tool answered.
    pub graph: String,
    /// The nodes the response named. `None` means the tool declared nothing,
    /// which differs from an empty list, so the field must be present even
    /// when it is null.
    #[serde(deserialize_with = "present")]
    pub retrieved: Option<Vec<RetrievedNode>>,
    /// A cap cut `retrieved` or `response_text`.
    pub truncated: bool,
    /// The text the tool returned, for display; empty on a hub bound beyond
    /// loopback.
    pub response_text: String,
}

/// One node a retrieval record names, and how the tool reached it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RetrievedNode {
    pub node_id: String,
    /// `lexical`, `segment`, `vector`, `expanded` or `direct`, kept as the
    /// hub wrote it.
    pub provenance: String,
    /// Its similarity or BM25 score; `None` where the tool ranks nothing.
    pub score: Option<f64>,
}

/// A nullable field that must be present: serde reads a missing `Option`
/// as `None` unless the field is read through a function.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

/// What went wrong reaching or reading the hub, worded for the person at
/// the terminal.
#[derive(Debug)]
pub enum HubError {
    /// No answer, or no whole answer: the hub is not running, the
    /// connection failed, or the answer was larger than its limit. `source`
    /// is the transport's own account of it.
    Request { url: String, source: String },
    /// The hub answered with an error status and, usually, a message.
    Status {
        url: String,
        status: u16,
        message: String,
    },
    /// The answer was not the shape the contract gives.
    Json {
        url: String,
        source: serde_json::Error,
    },
    /// Something on the port answered, but it is not an Epimemer hub.
    NotAHub { url: String, service: String },
    /// No session or graph fits what was asked for.
    Choice(String),
}

impl fmt::Display for HubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HubError::Request { url, source } => write!(
                f,
                "reading {url} failed: {source}. Is an Epimemer MCP server or `epimemer-viz` running?"
            ),
            HubError::Status {
                url,
                status,
                message,
            } => write!(f, "{url} answered {status}: {message}"),
            HubError::Json { url, source } => write!(
                f,
                "{url} sent a shape this client cannot read ({source}); it was written for schema version {SCHEMA_VERSION}"
            ),
            HubError::NotAHub { url, service } => write!(
                f,
                "{url} is not an Epimemer hub: it calls itself {service:?}"
            ),
            HubError::Choice(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for HubError {}

/// What to read from the hub, from the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `host:port`. In a browser, the page's own, which is where the hub
    /// is reached (`hub/web.rs`).
    pub hub: String,
    /// A session id, or `None` to choose one by graph.
    pub session: Option<String>,
    pub graph: String,
}

/// The snapshot, the session it was read from, what the hub listed on the
/// way, and how long the reads took.
#[derive(Debug)]
pub struct Loaded {
    pub snapshot: Snapshot,
    /// The session followed.
    pub session: SessionInfo,
    /// Every session the hub listed.
    pub sessions: Vec<SessionInfo>,
    /// The followed session's graphs.
    pub graphs: GraphList,
    pub snapshot_bytes: usize,
    pub download: Duration,
    pub parse: Duration,
}

/// `GET /api/sessions`, from the hub at `host:port`.
pub async fn sessions(hub: &str) -> Result<Vec<SessionInfo>, HubError> {
    let url = transport::url(hub, "/api/sessions");
    get_json(&transport::client(), &url, &[], SMALL_LIMIT).await
}

/// `GET /api/graphs?session=`, from the hub at `host:port`.
pub async fn graphs(hub: &str, session: &str) -> Result<GraphList, HubError> {
    let url = transport::url(hub, "/api/graphs");
    get_json(
        &transport::client(),
        &url,
        &[("session", session)],
        SMALL_LIMIT,
    )
    .await
}

/// `GET /api/retrievals?session=`, from the hub at `host:port`: the
/// session's retrieval records, oldest first.
pub async fn retrievals(hub: &str, session: &str) -> Result<Vec<RetrievalRecord>, HubError> {
    let url = transport::url(hub, "/api/retrievals");
    let list: RetrievalList = get_json(
        &transport::client(),
        &url,
        &[("session", session)],
        SMALL_LIMIT,
    )
    .await?;
    Ok(list.records)
}

/// Reaches the hub, lists its sessions, chooses one, checks it holds the
/// graph and fetches that graph's snapshot. Logs what it found on the way.
pub async fn load(request: &Request) -> Result<Loaded, HubError> {
    let chosen = choose(request).await?;
    snapshot(request, chosen).await
}

/// What a load finds before it asks for the snapshot: the session to
/// follow, every session the hub listed, and the followed session's graphs.
#[derive(Debug)]
pub struct Chosen {
    pub session: SessionInfo,
    pub sessions: Vec<SessionInfo>,
    pub graphs: GraphList,
}

/// The first part of [`load`]: reaches the hub, lists its sessions, chooses
/// one and checks it holds the graph. Between this and [`snapshot`] is the
/// moment to subscribe to the session's events, so that none between the
/// snapshot and the subscription is missed.
pub async fn choose(request: &Request) -> Result<Chosen, HubError> {
    let client = transport::client();

    let health_url = transport::url(&request.hub, "/api/health");
    let health: Health = get_json(&client, &health_url, &[], SMALL_LIMIT).await?;
    if health.service != HUB_SERVICE {
        return Err(HubError::NotAHub {
            url: transport::origin(&request.hub),
            service: health.service,
        });
    }
    bevy::log::info!(
        "hub at {} runs Epimemer {}; {}",
        request.hub,
        health.version,
        schema_note(health.schema_version)
    );

    let sessions_url = transport::url(&request.hub, "/api/sessions");
    let sessions: Vec<SessionInfo> = get_json(&client, &sessions_url, &[], SMALL_LIMIT).await?;
    bevy::log::info!(
        "{} sessions listed, {} connected",
        sessions.len(),
        sessions.iter().filter(|s| s.connected).count()
    );
    let session = choose_session(&sessions, request.session.as_deref(), &request.graph)?.clone();
    bevy::log::info!(
        "following session {} (pid {}, {}, active graph {:?}, {})",
        session.session_id,
        session.pid,
        session.backend,
        session.active_graph,
        schema_note(session.schema_version)
    );

    let graphs: GraphList = get_json(
        &client,
        &transport::url(&request.hub, "/api/graphs"),
        &[("session", &session.session_id)],
        SMALL_LIMIT,
    )
    .await?;
    bevy::log::info!(
        "the session's {} backend holds {} graphs, {:?} active: {}",
        graphs.backend,
        graphs.graphs.len(),
        graphs.active_graph,
        graphs.graphs.join(", ")
    );
    if !graphs.graphs.contains(&request.graph) {
        return Err(HubError::Choice(format!(
            "session {} holds no graph named {:?}; it holds {}",
            session.session_id,
            request.graph,
            graphs.graphs.join(", ")
        )));
    }

    Ok(Chosen {
        session,
        sessions,
        graphs,
    })
}

/// The second part of [`load`]: fetches the snapshot of the graph asked for
/// from the session [`choose`] chose.
pub async fn snapshot(request: &Request, chosen: Chosen) -> Result<Loaded, HubError> {
    let Chosen {
        session,
        sessions,
        graphs,
    } = chosen;
    let url = transport::url(&request.hub, "/api/snapshot");
    let started = Instant::now();
    let text = get_text(
        &transport::client(),
        &url,
        &[("session", &session.session_id), ("graph", &request.graph)],
        SNAPSHOT_LIMIT,
    )
    .await?;
    let download = started.elapsed();
    let started = Instant::now();
    let snapshot = parse_snapshot(&text).map_err(|source| HubError::Json {
        url: url.clone(),
        source,
    })?;
    let parse = started.elapsed();
    if snapshot.graph != request.graph {
        bevy::log::warn!(
            "asked for graph {:?} and the snapshot says it is {:?}",
            request.graph,
            snapshot.graph
        );
    }
    Ok(Loaded {
        snapshot,
        session,
        sessions,
        graphs,
        snapshot_bytes: text.len(),
        download,
        parse,
    })
}

/// Reads the topic embeddings of `graph` from `session` on the hub at
/// `host:port`. A session the hub no longer knows, as after its MCP server
/// restarted, is chosen again by graph as the first load chose it, and
/// asked once more.
pub async fn embeddings(
    hub: &str,
    session: &str,
    graph: &str,
) -> Result<EmbeddingsRead, EmbeddingsError> {
    match embeddings_from(hub, session, graph).await {
        Err(Read::SessionGone(why)) => {
            bevy::log::info!(
                "topic embeddings: session {session} answered no ({why}); choosing a session again by graph"
            );
            let request = Request {
                hub: hub.to_owned(),
                session: None,
                graph: graph.to_owned(),
            };
            let chosen = choose(&request)
                .await
                .map_err(|e| EmbeddingsError::Failed(e.to_string()))?;
            embeddings_from(hub, &chosen.session.session_id, graph)
                .await
                .map_err(|e| match e {
                    Read::NoRoute => EmbeddingsError::NoRoute,
                    Read::SessionGone(message) | Read::Failed(message) => {
                        EmbeddingsError::Failed(message)
                    }
                })
        }
        Err(Read::NoRoute) => Err(EmbeddingsError::NoRoute),
        Err(Read::Failed(message)) => Err(EmbeddingsError::Failed(message)),
        Ok(read) => Ok(read),
    }
}

/// How one read of the embeddings ended, when it did not answer.
enum Read {
    NoRoute,
    /// A `404` with the contract's body: the session is not known, or not
    /// connected.
    SessionGone(String),
    Failed(String),
}

async fn embeddings_from(hub: &str, session: &str, graph: &str) -> Result<EmbeddingsRead, Read> {
    let url = transport::url(hub, "/api/embeddings");
    let started = Instant::now();
    let query = [
        ("session", session),
        ("graph", graph),
        ("node_type", "topic"),
    ];
    let (status, body) = transport::get(&transport::client(), &url, &query, EMBEDDINGS_LIMIT)
        .await
        .map_err(|source| {
            Read::Failed(
                HubError::Request {
                    url: url.clone(),
                    source,
                }
                .to_string(),
            )
        })?;
    let download = started.elapsed();
    match embeddings_status(status, &body) {
        Some(Read::Failed(message)) => {
            return Err(Read::Failed(
                HubError::Status {
                    url,
                    status,
                    message,
                }
                .to_string(),
            ));
        }
        Some(other) => return Err(other),
        None => {}
    }
    let started = Instant::now();
    let embeddings: Embeddings = serde_json::from_str(&body)
        .map_err(|source| Read::Failed(HubError::Json { url, source }.to_string()))?;
    if schema_check(embeddings.schema_version) != SchemaCheck::Current {
        bevy::log::warn!("embeddings: {}", schema_note(embeddings.schema_version));
    }
    Ok(EmbeddingsRead {
        embeddings,
        bytes: body.len(),
        download,
        parse: started.elapsed(),
        session: session.to_owned(),
    })
}

/// What an answer's status says about the embeddings route: nothing for a
/// success, a missing route for a `404` without the contract's body, a
/// session gone for one with it, and a failure with its message otherwise.
fn embeddings_status(status: u16, body: &str) -> Option<Read> {
    let contract_body = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .is_some_and(|value| value.get("error").is_some_and(|e| e.is_string()));
    match status {
        200..300 => None,
        404 if !contract_body => Some(Read::NoRoute),
        404 => Some(Read::SessionGone(error_message(body))),
        _ => Some(Read::Failed(error_message(body))),
    }
}

/// Parses a snapshot and names its schema version when it is not the one
/// this client was written for. When the snapshot does not parse, the
/// version is read on its own, so the warning comes before the error even
/// for a shape that has changed.
pub fn parse_snapshot(text: &str) -> Result<Snapshot, serde_json::Error> {
    #[derive(Deserialize)]
    struct Versioned {
        #[serde(default)]
        schema_version: Option<u32>,
    }
    let snapshot = serde_json::from_str::<Snapshot>(text);
    let version = match &snapshot {
        Ok(snapshot) => snapshot.schema_version,
        Err(_) => serde_json::from_str::<Versioned>(text)
            .ok()
            .and_then(|v| v.schema_version),
    };
    if schema_check(version) != SchemaCheck::Current {
        bevy::log::warn!("snapshot: {}", schema_note(version));
    }
    snapshot
}

/// How a schema version compares with the one this client speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaCheck {
    Current,
    /// No version, or a smaller one: a hub or session older than the
    /// contract. Its shapes are expected to read, since the contract was
    /// written from that code.
    Older,
    /// A larger version: fields may have been removed or renamed.
    Newer,
}

pub fn schema_check(version: Option<u32>) -> SchemaCheck {
    match version {
        Some(v) if v == SCHEMA_VERSION => SchemaCheck::Current,
        Some(v) if v > SCHEMA_VERSION => SchemaCheck::Newer,
        _ => SchemaCheck::Older,
    }
}

/// A plain sentence about a schema version, for the log.
pub fn schema_note(version: Option<u32>) -> String {
    let written = format!("the version {SCHEMA_VERSION} this client was written for");
    match (schema_check(version), version) {
        (SchemaCheck::Current, _) => format!("schema version {SCHEMA_VERSION}"),
        (SchemaCheck::Newer, Some(v)) => {
            format!("schema version {v}, newer than {written}; reading it anyway")
        }
        (_, Some(v)) => format!("schema version {v}, older than {written}"),
        (_, None) => format!("no schema version, so older than {written}"),
    }
}

/// Chooses the session to follow.
///
/// With an id, that session, which must be connected because the hub
/// answers reads only for connected sessions. Without one, the connected
/// session whose active graph is `graph`, else any connected session; among
/// several, the most recently active. A session's activity is the later of
/// its last event and its start, compared as instants rather than as text.
pub fn choose_session<'a>(
    sessions: &'a [SessionInfo],
    wanted: Option<&str>,
    graph: &str,
) -> Result<&'a SessionInfo, HubError> {
    if let Some(id) = wanted {
        return match sessions.iter().find(|s| s.session_id == id) {
            Some(s) if s.connected => Ok(s),
            Some(_) => Err(HubError::Choice(format!(
                "session {id} is listed but not connected, so the hub cannot read from it"
            ))),
            None => Err(HubError::Choice(format!(
                "the hub lists no session {id}; it lists {}",
                session_ids(sessions)
            ))),
        };
    }
    let connected = || sessions.iter().filter(|s| s.connected);
    let latest = |candidates: Vec<&'a SessionInfo>| {
        candidates
            .into_iter()
            .max_by_key(|s| (last_active(s), s.session_id.clone()))
    };
    latest(connected().filter(|s| s.active_graph == graph).collect())
        .or_else(|| latest(connected().collect()))
        .ok_or_else(|| {
            HubError::Choice(format!(
                "the hub lists no connected session ({} listed); start an Epimemer MCP server",
                sessions.len()
            ))
        })
}

/// The later of a session's last event and its start, in microseconds
/// since the Unix epoch; a time that does not parse counts as the earliest.
fn last_active(session: &SessionInfo) -> i64 {
    let started = parse_timestamp(&session.started_at);
    let event = session.last_event_at.as_deref().and_then(parse_timestamp);
    started.max(event).unwrap_or(i64::MIN)
}

fn session_ids(sessions: &[SessionInfo]) -> String {
    if sessions.is_empty() {
        return "none".into();
    }
    sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A contract timestamp as microseconds since the Unix epoch.
///
/// The contract's timestamps are ISO 8601 in UTC. The hub writes them with
/// a `Z` or a `+00:00` offset, with or without a fraction; a stamp with no
/// offset at all is read as UTC, because the contract says every stamp is.
pub fn parse_timestamp(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.timestamp_micros())
        .or_else(|_| {
            NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|t| t.and_utc().timestamp_micros())
        })
        .ok()
}

/// The body of an error answer: the contract's `{"error": "<message>"}`, or
/// the raw text when the body is something else.
pub fn error_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: String,
    }
    match serde_json::from_str::<ErrorBody>(body) {
        Ok(parsed) => parsed.error,
        Err(_) if body.trim().is_empty() => "no message".into(),
        Err(_) => body.trim().chars().take(300).collect(),
    }
}

async fn get_text(
    client: &transport::Client,
    url: &str,
    query: &[(&str, &str)],
    limit: u64,
) -> Result<String, HubError> {
    let (status, body) = transport::get(client, url, query, limit)
        .await
        .map_err(|source| HubError::Request {
            url: url.to_owned(),
            source,
        })?;
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(HubError::Status {
            url: url.to_owned(),
            status,
            message: error_message(&body),
        })
    }
}

async fn get_json<T: DeserializeOwned>(
    client: &transport::Client,
    url: &str,
    query: &[(&str, &str)],
    limit: u64,
) -> Result<T, HubError> {
    let text = get_text(client, url, query, limit).await?;
    serde_json::from_str(&text).map_err(|source| HubError::Json {
        url: url.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/snapshot.json");

    fn session(id: &str, graph: &str, connected: bool, last_event: Option<&str>) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            pid: 1,
            backend: "surrealdb".into(),
            active_graph: graph.into(),
            started_at: "2026-10-20T10:00:00Z".into(),
            connected,
            last_event_at: last_event.map(Into::into),
            schema_version: Some(1),
        }
    }

    #[test]
    fn the_fixture_parses_in_the_contract_shape() {
        let snapshot = parse_snapshot(FIXTURE).expect("fixture parses");
        assert_eq!(snapshot.schema_version, Some(1));
        assert_eq!(snapshot.graph, "fixture");
        assert_eq!(snapshot.nodes.len(), 6);
        assert_eq!(snapshot.edges.len(), 9);
        assert_eq!(snapshot.metacontexts.len(), 1);
        assert_eq!(snapshot.metacontexts[0].metacontext_id, "the-real");
        assert_eq!(snapshot.metacontexts[0].content, "The Real");
        let kinds: Vec<NodeType> = snapshot.nodes.iter().map(|n| n.node_type).collect();
        assert_eq!(
            kinds,
            [
                NodeType::Topic,
                NodeType::Topic,
                NodeType::Fact,
                NodeType::Fact,
                NodeType::Inference,
                NodeType::Unknown
            ]
        );
    }

    #[test]
    fn an_empty_metadata_dict_and_nulls_read_as_empty_and_none() {
        let snapshot = parse_snapshot(FIXTURE).unwrap();
        let topic = &snapshot.nodes[0];
        assert!(topic.metadata.is_empty());
        assert_eq!(topic.confidence, None);
        assert_eq!(topic.source_id, None);
        let fact = &snapshot.nodes[2];
        assert_eq!(fact.confidence, Some(0.9));
        assert_eq!(
            fact.metadata
                .get("confidence_basis")
                .and_then(|v| v.as_str()),
            Some("stated in the source")
        );
    }

    #[test]
    fn edges_whose_endpoints_are_not_nodes_are_kept() {
        let snapshot = parse_snapshot(FIXTURE).unwrap();
        let node_ids: Vec<&str> = snapshot.nodes.iter().map(|n| n.node_id.as_str()).collect();
        let unresolved: Vec<&str> = snapshot
            .edges
            .iter()
            .filter(|e| {
                !node_ids.contains(&e.src_id.as_str()) || !node_ids.contains(&e.dst_id.as_str())
            })
            .map(|e| e.edge_type.as_str())
            .collect();
        assert_eq!(
            unresolved,
            ["contains", "sourced_from", "has_metacontext", "timelink"]
        );
        let sourced = snapshot
            .edges
            .iter()
            .find(|e| e.edge_type == "sourced_from")
            .unwrap();
        assert_eq!(sourced.validity.len(), 1);
        let timelink = snapshot
            .edges
            .iter()
            .find(|e| e.edge_type == "timelink")
            .unwrap();
        assert!(timelink.metadata.contains_key("timepoint_id"));
    }

    #[test]
    fn timelines_carry_their_timepoints_and_a_validity_end_its_kind() {
        let snapshot = parse_snapshot(FIXTURE).unwrap();
        assert_eq!(snapshot.timelines.len(), 1);
        let timeline = &snapshot.timelines[0];
        assert_eq!(timeline.reference_time, None, "the wall clock");
        let point = &timeline.timepoints[0];
        assert_eq!(point.kind, "vague");
        assert_eq!(point.earliest.as_deref(), Some("2026-03-01T00:00:00Z"));
        assert_eq!(
            (point.start.as_deref(), point.latest.as_deref()),
            (None, None)
        );
        assert!(!point.contested);
        let sourced = snapshot
            .edges
            .iter()
            .find(|e| e.edge_type == "sourced_from")
            .unwrap();
        let validity = &sourced.validity[0];
        assert_eq!(
            validity.start,
            ImpreciseInstant::Precise {
                at: "2026-04-20T00:00:00Z".into(),
                label: None
            }
        );
        assert_eq!(validity.end, ImpreciseInstant::Unknown);
        assert_eq!(validity.basis, "stated");
        assert_eq!(
            (&validity.timeline_id, &validity.witnessed_at),
            (&None, &None)
        );
        for (text, instant) in [
            (
                r#"{"instant_kind": "named", "label": "spring"}"#,
                ImpreciseInstant::Named {
                    label: "spring".into(),
                },
            ),
            (
                r#"{"instant_kind": "unbounded"}"#,
                ImpreciseInstant::Unbounded,
            ),
            (
                r#"{"instant_kind": "unknown", "note": "extra"}"#,
                ImpreciseInstant::Unknown,
            ),
            (
                r#"{"instant_kind": "fuzzy", "width": 3}"#,
                ImpreciseInstant::Other,
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<ImpreciseInstant>(text).unwrap(),
                instant,
                "{text}"
            );
        }
    }

    #[test]
    fn unknown_fields_are_ignored_and_an_absent_version_reads_as_none() {
        let text = r#"{"graph": "g", "nodes": [], "edges": [], "timelines": [], "metacontexts": [],
            "a_field_from_the_future": 3}"#;
        let snapshot = parse_snapshot(text).unwrap();
        assert_eq!(snapshot.schema_version, None);
        assert_eq!(schema_check(snapshot.schema_version), SchemaCheck::Older);
    }

    #[test]
    fn a_node_kind_added_later_reads_as_unknown() {
        let node = r#"{"node_id": "n", "node_type": "observation", "content": "",
            "description": "", "status": "active", "source_id": null,
            "extraction_method": "unspecified", "confidence": null,
            "retrieved_at": null, "created_at": "2026-10-20T00:00:00Z",
            "graph": "g", "metadata": {}, "salience": 0.5}"#;
        let node: NodeView = serde_json::from_str(node).unwrap();
        assert_eq!(node.node_type, NodeType::Unknown);
    }

    #[test]
    fn a_missing_contract_field_is_an_error_naming_it() {
        let text = r#"{"graph": "g", "nodes": [{"node_id": "n"}], "edges": [], "timelines": [],
                "metacontexts": []}"#;
        let error = parse_snapshot(text).unwrap_err().to_string();
        assert!(error.contains("node_type"), "{error}");
    }

    #[test]
    fn schema_versions_compare_against_the_one_written_for() {
        assert_eq!(schema_check(Some(1)), SchemaCheck::Current);
        assert_eq!(schema_check(Some(0)), SchemaCheck::Older);
        assert_eq!(schema_check(None), SchemaCheck::Older);
        assert_eq!(schema_check(Some(2)), SchemaCheck::Newer);
        assert_eq!(
            schema_note(None),
            "no schema version, so older than the version 1 this client was written for"
        );
        assert_eq!(schema_note(Some(1)), "schema version 1");
    }

    #[test]
    fn timestamps_parse_in_every_form_the_hub_writes() {
        let z = parse_timestamp("2026-10-20T11:17:45.191899Z").unwrap();
        assert_eq!(parse_timestamp("2026-10-20T11:17:45.191899+00:00"), Some(z));
        assert_eq!(parse_timestamp("2026-10-20T11:17:45.191899"), Some(z));
        assert_eq!(parse_timestamp("2026-10-20T12:17:45.191899+01:00"), Some(z));
        assert_eq!(
            parse_timestamp("2026-10-20T11:17:45Z"),
            Some(z - 191_899),
            "no fraction"
        );
        assert_eq!(parse_timestamp("1970-01-01T00:00:01Z"), Some(1_000_000));
        assert_eq!(parse_timestamp("yesterday"), None);
        assert_eq!(parse_timestamp(""), None);
    }

    #[test]
    fn a_named_session_is_chosen_only_when_connected() {
        let sessions = [
            session("a", "memory", true, None),
            session("b", "memory", false, None),
        ];
        assert_eq!(
            choose_session(&sessions, Some("a"), "x")
                .unwrap()
                .session_id,
            "a"
        );
        assert!(matches!(
            choose_session(&sessions, Some("b"), "memory"),
            Err(HubError::Choice(_))
        ));
        assert!(matches!(
            choose_session(&sessions, Some("c"), "memory"),
            Err(HubError::Choice(_))
        ));
    }

    #[test]
    fn the_session_on_the_graph_wins_and_then_the_most_recent() {
        let sessions = [
            session("old", "memory", true, Some("2026-10-20T11:00:00Z")),
            session("other", "games", true, Some("2026-10-20T14:00:00Z")),
            session("new", "memory", true, Some("2026-10-20T12:00:00+00:00")),
            session("gone", "memory", false, Some("2026-10-20T15:00:00Z")),
        ];
        assert_eq!(
            choose_session(&sessions, None, "memory")
                .unwrap()
                .session_id,
            "new"
        );
        assert_eq!(
            choose_session(&sessions, None, "nowhere")
                .unwrap()
                .session_id,
            "other",
            "no session on the graph, so the most recent connected one"
        );
    }

    #[test]
    fn activity_is_compared_as_instants_not_text() {
        // As text, a's stamp sorts last because its hour reads 10; as an
        // instant it is 09:00 UTC, the earliest of the three, and c is the
        // latest.
        let sessions = [
            session("a", "memory", true, Some("2026-10-21T10:00:00+01:00")),
            session("b", "memory", true, Some("2026-10-21T09:30:00.5Z")),
            session("c", "memory", true, Some("2026-10-21T09:30:01+00:00")),
        ];
        assert_eq!(
            choose_session(&sessions, None, "memory")
                .unwrap()
                .session_id,
            "c"
        );
    }

    #[test]
    fn no_connected_session_is_an_error() {
        let sessions = [session("a", "memory", false, None)];
        assert!(matches!(
            choose_session(&sessions, None, "memory"),
            Err(HubError::Choice(_))
        ));
        assert!(matches!(
            choose_session(&[], None, "memory"),
            Err(HubError::Choice(_))
        ));
    }

    #[test]
    fn the_embeddings_parse_in_the_contract_shape() {
        let text = r#"{"schema_version": 1, "graph": "g", "node_type": "topic",
            "models": {"all-MiniLM-L6-v2": 3},
            "embeddings": [
              {"node_id": "t1", "node_type": "topic", "model_id": "all-MiniLM-L6-v2", "vector": [0.5, -1, 2e-3]},
              {"node_id": "t2", "node_type": "topic", "model_id": "all-MiniLM-L6-v2", "vector": [1, 0, 0], "norm": 1}
            ], "a_field_from_the_future": true}"#;
        let read: Embeddings = serde_json::from_str(text).unwrap();
        assert_eq!(read.models["all-MiniLM-L6-v2"], 3);
        assert_eq!(read.embeddings.len(), 2);
        assert_eq!(read.embeddings[0].vector, [0.5, -1.0, 0.002]);
        assert_eq!(read.embeddings[1].node_type, NodeType::Topic);
    }

    #[test]
    fn a_missing_route_is_told_from_a_session_the_hub_does_not_know() {
        assert!(embeddings_status(200, "{}").is_none());
        assert!(matches!(
            embeddings_status(404, "Not Found"),
            Some(Read::NoRoute)
        ));
        assert!(matches!(
            embeddings_status(404, r#"{"error": "unknown session abc"}"#),
            Some(Read::SessionGone(m)) if m == "unknown session abc"
        ));
        assert!(matches!(
            embeddings_status(502, r#"{"error": "answer too large"}"#),
            Some(Read::Failed(m)) if m == "answer too large"
        ));
        assert!(EmbeddingsError::NoRoute.to_string().contains("older than"));
    }

    #[test]
    fn the_retrieval_records_parse_in_the_contract_shape() {
        let text = r#"{"records": [
            {"record_id": "000000000002", "at": "2026-11-07T10:39:59.738344Z",
             "tool": "epimemer.segment", "query": "content_length=1679", "graph": "g",
             "retrieved": null, "response_text": "segmented", "truncated": false},
            {"record_id": "000000000014", "at": "2026-11-07T11:57:02.781461Z",
             "tool": "epimemer.store_decomposition", "query": "doc=d", "graph": "g",
             "retrieved": [], "response_text": "", "truncated": false},
            {"record_id": "000000000012", "at": "2026-11-07T11:31:36.954275+00:00",
             "tool": "epimemer.search", "query": "query=bevy", "graph": "other",
             "retrieved": [
               {"node_id": "n1", "provenance": "lexical", "score": 7.5},
               {"node_id": "n2", "provenance": "expanded", "score": null},
               {"node_id": "n3", "provenance": "a_future_provenance"}
             ],
             "response_text": "three hits", "truncated": true, "a_field_from_the_future": 1}
        ]}"#;
        let list: RetrievalList = serde_json::from_str(text).unwrap();
        let [undeclared, empty, search] = list.records.as_slice() else {
            panic!("{list:?}")
        };
        assert_eq!(undeclared.retrieved, None, "null declares nothing");
        assert_eq!(empty.retrieved, Some(Vec::new()), "an empty list is kept");
        assert_eq!(
            (
                search.tool.as_str(),
                search.graph.as_str(),
                search.truncated
            ),
            ("epimemer.search", "other", true)
        );
        let nodes = search.retrieved.as_deref().unwrap();
        assert_eq!(
            nodes[0],
            RetrievedNode {
                node_id: "n1".into(),
                provenance: "lexical".into(),
                score: Some(7.5)
            }
        );
        assert_eq!(
            (nodes[1].score, nodes[2].provenance.as_str()),
            (None, "a_future_provenance")
        );
    }

    #[test]
    fn the_graph_list_carries_the_active_graphs_reflect_counter() {
        let text = r#"{"graphs": ["epimemer-dev", "rcloud"], "active_graph": "epimemer-dev",
            "backend": "surrealdb", "reflect": {"count": 8, "threshold": 10, "suggested": false}}"#;
        let list: GraphList = serde_json::from_str(text).unwrap();
        assert_eq!(
            (list.active_graph.as_str(), list.reflect),
            (
                "epimemer-dev",
                Reflect {
                    count: 8,
                    threshold: 10,
                    suggested: false
                }
            )
        );
        let without = r#"{"graphs": [], "active_graph": "g", "backend": "memory"}"#;
        let error = serde_json::from_str::<GraphList>(without)
            .unwrap_err()
            .to_string();
        assert!(error.contains("reflect"), "{error}");
    }

    #[test]
    fn a_retrieval_record_without_its_retrieved_field_is_an_error_naming_it() {
        let text = r#"{"record_id": "000000000001", "at": "2026-11-07T10:00:00Z",
            "tool": "epimemer.search", "query": "", "graph": "g",
            "response_text": "", "truncated": false}"#;
        let error = serde_json::from_str::<RetrievalRecord>(text)
            .unwrap_err()
            .to_string();
        assert!(error.contains("retrieved"), "{error}");
    }

    #[test]
    fn error_bodies_read_the_contract_message() {
        assert_eq!(
            error_message(r#"{"error": "unknown session"}"#),
            "unknown session"
        );
        assert_eq!(error_message("Bad gateway\n"), "Bad gateway");
        assert_eq!(error_message(""), "no message");
    }
}
