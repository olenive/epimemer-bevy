//! The event stream (VIZ_CONTRACT.md §7): a WebSocket client that follows
//! one session's events from the hub's `/ws`.
//!
//! The client runs in the background (`background.rs`), so the window never
//! waits on the hub. It connects, sends the contract's `subscribe` message
//! for the session and graph followed, reads every message into a
//! [`Message`], and sends it to the app over a channel that `live.rs` drains
//! once per update. A socket that drops, or a hub that is not there, is
//! tried again after a wait that doubles from [`FIRST_RETRY`] to
//! [`MAX_RETRY`] and starts again from the first after a connection opens;
//! each opening and each loss is sent to the app as an [`Item`], so that the
//! bar can say how the stream stands and the app can read the snapshot again
//! on a reconnect. The app changes what is followed with [`resubscribe`];
//! the client sends the new subscription at once, and sends it again on
//! every reconnect. The client ends when the app drops the [`Stream`].
//!
//! That loop is written once ([`run`]), over a socket with one
//! implementation for each platform: natively a blocking tungstenite socket
//! on a thread of its own (`stream/native.rs`), in a browser the page's
//! WebSocket (`stream/web.rs`).
//!
//! Parsing is a pure function, [`parse`], so that every message shape is
//! checked on recorded JSON without a hub. The messages this client acts on
//! are read into the contract's own structs (`hub.rs` and below): the graph
//! events, the retrieval records, which `live.rs` keeps for the Retrieval
//! picker (`retrieval.rs`), the acts, the advisories and the six pipeline
//! events, which it keeps for the log (`event_log.rs`), and the reflect
//! counter, which the bar shows (`reflect_counter.rs`). An act's verb, an
//! advisory's kind and action, and the keys of a count are kept as the
//! strings they arrive as, since the contract leaves them open. Any other
//! event, such as the embedding, document and segment events and any type
//! added later, is [`Event::Other`], received and dropped. A
//! message's `seq` is kept whatever it is, since a gap in it means events
//! were lost. The hub is plain `ws://` on loopback (VIZ_CONTRACT.md §2), so
//! the native client has no TLS.
//!
//! Two hubs are read the same way. The contract's hub lists a session with
//! its `schema_version`; Epimemer 0.2.11's, which is older than the
//! contract, lists none, and [`SessionInfo`] reads its absence as `None`.
//! That hub also sends `transition_enabled`, a pipeline event the contract
//! no longer has; it is one more [`Event::Other`].

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

use bevy::prelude::Resource;
use serde::Deserialize;
use serde_json::{Map, Value};
use web_time::Instant;

use crate::background;
use crate::hub::{EdgeView, NodeView, Reflect, RetrievalRecord, SessionInfo, TimelineView};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native as transport;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web as transport;

/// The first wait before connecting again, and the longest.
pub const FIRST_RETRY: Duration = Duration::from_millis(500);
pub const MAX_RETRY: Duration = Duration::from_secs(10);

/// What the stream follows: one session, and the graphs of it whose events
/// it wants. The hub sends every graph's events when `graphs` is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    pub session: String,
    pub graphs: Vec<String>,
}

/// The contract's subscribe message. An empty list of graphs is left out,
/// since the hub would read it as a set that no graph is in.
pub fn subscribe_text(subscription: &Subscription) -> String {
    let mut body = Map::new();
    body.insert("session".into(), subscription.session.clone().into());
    if !subscription.graphs.is_empty() {
        body.insert("graphs".into(), subscription.graphs.clone().into());
    }
    serde_json::json!({ "subscribe": body }).to_string()
}

/// One message from the hub, read.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// A system message, sent to every client whatever it follows.
    System { seq: Option<u64>, system: System },
    /// An event of the session subscribed to.
    Event {
        seq: Option<u64>,
        envelope: Box<Envelope>,
    },
    /// Text that is not a JSON object, an object that is neither a system
    /// message nor an event, or a system message whose body does not read.
    Unreadable { seq: Option<u64>, error: String },
}

/// A message's `seq`, where it has one.
pub fn seq(message: &Message) -> Option<u64> {
    match message {
        Message::System { seq, .. }
        | Message::Event { seq, .. }
        | Message::Unreadable { seq, .. } => *seq,
    }
}

/// The system messages (VIZ_CONTRACT.md §7.3).
#[derive(Clone, Debug, PartialEq)]
pub enum System {
    /// A session registered, reconnected, or registered again because its
    /// active graph changed: its entry of `/api/sessions`.
    SessionConnected(SessionInfo),
    /// A session's socket closed; it stays listed for a while.
    SessionDisconnected(String),
    /// A session's grace period ended without a reconnect.
    SessionDropped(String),
    /// A system message this version does not know, by its type.
    Other(String),
}

/// An event and what the hub stamped on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub event_type: String,
    /// The session that published it, stamped by the hub.
    pub session_id: String,
    /// The graph it is about; empty on the pipeline events and a few more.
    pub graph: String,
    /// When the session made it, as the hub wrote it.
    pub timestamp: String,
    pub event: Event,
}

/// The events this client acts on, and one case for every other.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A node was created or updated.
    NodeStored(Box<NodeView>),
    /// An edge was created.
    EdgeStored(Box<EdgeView>),
    /// A node left or re-entered the active set.
    NodeStatusChanged(StatusChange),
    /// A timeline was created or changed, whole.
    TimelineStored(Box<TimelineView>),
    /// The session's active graph changed.
    GraphSwitched {
        previous_graph: String,
        new_graph: String,
    },
    /// A tool answered: what it handed the agent.
    RetrievalRecorded(Box<RetrievalRecord>),
    /// One act, at the transaction boundary that made it.
    ActRecorded(Box<Act>),
    /// A warning a tool computed, whether or not the agent was shown it.
    AdvisoryRaised(Box<Advisory>),
    /// A graph's reflect counter moved.
    ReflectCounterUpdated(Reflect),
    /// A pipeline run started, a transition fired or completed, its tokens
    /// moved, or it ended.
    Pipeline(Box<Pipeline>),
    /// An event this client receives and does not act on.
    Other,
    /// An event this client acts on whose body does not read: the shape
    /// has changed, and what the client draws may be out of date.
    Unreadable(String),
}

/// The body of `node_status_changed`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StatusChange {
    pub node_id: String,
    pub old_status: String,
    pub new_status: String,
    /// The node that replaced, followed or absorbed this one; `None` for
    /// archival.
    #[serde(default)]
    pub counterpart: Option<String>,
}

/// The body of `graph_action_recorded`: one act.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Act {
    /// Its place in the session's numbering, which the advisories share,
    /// padded to twelve digits so that text order is numeric order.
    pub action_id: String,
    /// `stored`, `corrected`, `merged` and the rest, or the kind of a
    /// timeline decision or a reopen.
    pub verb: String,
    /// The nodes it was about, or the timeline a decision was about.
    pub subjects: Vec<String>,
    /// What it swept up, such as `{"edges": 3, "nodes": 1}`.
    pub counts: BTreeMap<String, i64>,
    /// The act in words, as the session wrote it.
    pub summary: String,
    /// The judge the tool named, where it named one.
    #[serde(default)]
    pub judged_by: Option<String>,
}

/// The body of `advisory_raised`: a warning a tool computed.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Advisory {
    /// Its place in the session's numbering, which the acts share.
    pub action_id: String,
    /// The tool it came from, without the `epimemer.` prefix.
    pub tool: String,
    /// What kind of warning it is, such as `tag_joined_topic`.
    pub kind: String,
    /// The warning's own sentence, word for word.
    pub message: String,
    pub subjects: Vec<String>,
    /// More about it, in a shape the contract does not promise.
    pub detail: Map<String, Value>,
    /// What the graph's policy says about this kind, such as `proceed`.
    pub action: String,
    /// Whether the agent's response carried it.
    pub surfaced: bool,
    /// Whether the agent was asked to raise it with a person.
    pub notify_user: bool,
    /// The judge the call named, where the graph requires one.
    #[serde(default)]
    pub judged_by: Option<String>,
}

/// An edge of a pipeline's topology, between a place and a transition.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TopologyEdge {
    pub source: String,
    pub target: String,
    /// The argument an input edge feeds, where it has one.
    #[serde(default)]
    pub label: Option<String>,
}

/// The six pipeline events (VIZ_CONTRACT.md §7.4), which describe the
/// Petri nets a session runs. None carries a run id: a run is told by its
/// pipeline's name alone.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "event_type")]
pub enum Pipeline {
    /// A run started, with the whole topology of its net.
    #[serde(rename = "pipeline_started")]
    Started {
        pipeline_name: String,
        place_names: Vec<String>,
        transition_names: Vec<String>,
        edges: Vec<TopologyEdge>,
    },
    /// A transition went in flight.
    #[serde(rename = "transition_fired")]
    TransitionFired {
        pipeline_name: String,
        transition_name: String,
        input_places: Vec<String>,
    },
    /// A transition's body finished, after `duration_ms`.
    #[serde(rename = "transition_completed")]
    TransitionCompleted {
        pipeline_name: String,
        transition_name: String,
        output_places: Vec<String>,
        duration_ms: f64,
    },
    /// How many tokens each place holds now.
    #[serde(rename = "tokens_updated")]
    TokensUpdated {
        pipeline_name: String,
        place_token_counts: BTreeMap<String, i64>,
    },
    /// The run ended.
    #[serde(rename = "pipeline_completed")]
    Completed {
        pipeline_name: String,
        transitions_fired: u64,
        duration_ms: f64,
    },
    /// The run ended by raising, in `transition_name` where one was
    /// running.
    #[serde(rename = "pipeline_failed")]
    Failed {
        pipeline_name: String,
        error: String,
        #[serde(default)]
        transition_name: Option<String>,
        transitions_fired: u64,
        duration_ms: f64,
    },
}

/// The event types of [`Pipeline`].
pub const PIPELINE_EVENTS: [&str; 6] = [
    "pipeline_started",
    "transition_fired",
    "transition_completed",
    "tokens_updated",
    "pipeline_completed",
    "pipeline_failed",
];

#[derive(Deserialize)]
struct Switched {
    previous_graph: String,
    new_graph: String,
}

/// Reads one message from the hub.
pub fn parse(text: &str) -> Message {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return Message::Unreadable {
                seq: None,
                error: format!("not JSON: {error}"),
            };
        }
    };
    let Some(object) = value.as_object() else {
        return Message::Unreadable {
            seq: None,
            error: "not a JSON object".into(),
        };
    };
    let seq = object.get("seq").and_then(Value::as_u64);
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        let system = match kind {
            "session_connected" => match object.get("session").cloned() {
                Some(session) => serde_json::from_value(session).map(System::SessionConnected),
                None => {
                    return Message::Unreadable {
                        seq,
                        error: "session_connected names no session".into(),
                    };
                }
            },
            "session_disconnected" => Ok(System::SessionDisconnected(text("session_id"))),
            "session_dropped" => Ok(System::SessionDropped(text("session_id"))),
            other => Ok(System::Other(other.to_owned())),
        };
        return match system {
            Ok(system) => Message::System { seq, system },
            Err(error) => Message::Unreadable {
                seq,
                error: format!("{kind}: {error}"),
            },
        };
    }
    let Some(event_type) = object.get("event_type").and_then(Value::as_str) else {
        return Message::Unreadable {
            seq,
            error: "neither a system message nor an event".into(),
        };
    };
    let field = |key: &str| object.get(key).cloned().unwrap_or(Value::Null);
    let read = |result: Result<Event, serde_json::Error>| {
        result.unwrap_or_else(|error| Event::Unreadable(error.to_string()))
    };
    let event = match event_type {
        "node_stored" => {
            read(serde_json::from_value(field("node")).map(|n| Event::NodeStored(Box::new(n))))
        }
        "edge_stored" => {
            read(serde_json::from_value(field("edge")).map(|e| Event::EdgeStored(Box::new(e))))
        }
        "timeline_stored" => read(
            serde_json::from_value(field("timeline")).map(|t| Event::TimelineStored(Box::new(t))),
        ),
        "node_status_changed" => {
            read(serde_json::from_value(value.clone()).map(Event::NodeStatusChanged))
        }
        "retrieval_recorded" => read(
            serde_json::from_value(field("record")).map(|r| Event::RetrievalRecorded(Box::new(r))),
        ),
        "graph_action_recorded" => {
            read(serde_json::from_value(value.clone()).map(|a| Event::ActRecorded(Box::new(a))))
        }
        "advisory_raised" => {
            read(serde_json::from_value(value.clone()).map(|a| Event::AdvisoryRaised(Box::new(a))))
        }
        "reflect_counter_updated" => {
            read(serde_json::from_value(value.clone()).map(Event::ReflectCounterUpdated))
        }
        kind if PIPELINE_EVENTS.contains(&kind) => {
            read(serde_json::from_value(value.clone()).map(|p| Event::Pipeline(Box::new(p))))
        }
        "graph_switched" => {
            read(
                serde_json::from_value::<Switched>(value.clone()).map(|s| Event::GraphSwitched {
                    previous_graph: s.previous_graph,
                    new_graph: s.new_graph,
                }),
            )
        }
        _ => Event::Other,
    };
    Message::Event {
        seq,
        envelope: Box::new(Envelope {
            event_type: event_type.to_owned(),
            session_id: text("session_id"),
            graph: text("graph"),
            timestamp: text("timestamp"),
            event,
        }),
    }
}

/// What the client sends the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A socket opened and the subscription was sent on it. Connections are
    /// numbered from 1, and `seq` starts again on each.
    Connected {
        connection: u32,
    },
    /// The socket closed, or could not open; the client tries again after
    /// `retry_in`.
    Disconnected {
        error: String,
        retry_in: Duration,
    },
    Message(Message),
}

/// What the app asks the client.
#[derive(Debug)]
pub enum Command {
    /// Follow another session or graph. `ack`, where given, is answered
    /// once the subscription has been written to the socket, or at once
    /// while there is no socket, since a reconnect sends it anyway.
    Subscribe {
        subscription: Subscription,
        ack: Option<Sender<()>>,
    },
}

/// The stream as the app holds it: the items the client has sent, and the
/// way to send it commands. Dropping it ends the client.
#[derive(Resource)]
pub struct Stream {
    items: Mutex<Receiver<Item>>,
    commands: Sender<Command>,
}

/// A stream over channels made elsewhere, for a test to feed.
pub fn from_channels(items: Receiver<Item>, commands: Sender<Command>) -> Stream {
    Stream {
        items: Mutex::new(items),
        commands,
    }
}

/// Every item the client has sent since the last drain, without waiting.
pub fn drain(stream: &Stream) -> Vec<Item> {
    let Ok(items) = stream.items.lock() else {
        return Vec::new();
    };
    std::iter::from_fn(|| items.try_recv().ok()).collect()
}

/// Asks the client to follow another subscription. The answer to `ack`
/// comes once it has been written to the socket.
pub fn resubscribe(stream: &Stream, subscription: Subscription, ack: Option<Sender<()>>) {
    // A client that has ended has nothing to follow.
    let _ = stream
        .commands
        .send(Command::Subscribe { subscription, ack });
}

/// A stream just started, and the answer to its first attempt to connect:
/// the connection's number, or why it failed.
pub struct Started {
    pub stream: Stream,
    pub first: Receiver<Result<u32, String>>,
}

/// The address of the hub's event stream.
pub fn url(hub: &str) -> String {
    transport::url(hub)
}

/// Starts the client that follows `subscription` on the hub at `host:port`.
pub fn start(hub: &str, subscription: Subscription) -> Result<Started, String> {
    let (items, item_receiver) = mpsc::channel();
    let (commands, command_receiver) = mpsc::channel();
    let (first, first_receiver) = mpsc::channel();
    let hub = hub.to_owned();
    background::spawn("follow the event stream", move || async move {
        run(&hub, subscription, &items, &command_receiver, Some(first)).await;
    })?;
    Ok(Started {
        stream: from_channels(item_receiver, commands),
        first: first_receiver,
    })
}

/// Why a connection ended.
enum Ended {
    /// The app dropped its end: the client stops.
    AppGone,
    /// The socket failed or closed, for this reason.
    Lost(String),
}

/// The client: connect, subscribe, read until the socket drops, wait, and
/// again, until the app is gone.
async fn run(
    hub: &str,
    mut subscription: Subscription,
    items: &Sender<Item>,
    commands: &Receiver<Command>,
    mut first: Option<Sender<Result<u32, String>>>,
) {
    let url = url(hub);
    let mut retry = FIRST_RETRY;
    let mut connection = 0;
    loop {
        let opened = transport::connect(hub, &url).await;
        let error = match opened.and_then(|mut socket| {
            transport::send(&mut socket, subscribe_text(&subscription))?;
            Ok(socket)
        }) {
            Ok(mut socket) => {
                connection += 1;
                retry = FIRST_RETRY;
                if items.send(Item::Connected { connection }).is_err() {
                    return;
                }
                if let Some(first) = first.take() {
                    let _ = first.send(Ok(connection));
                }
                match transport::follow(&mut socket, &mut subscription, items, commands).await {
                    Ended::AppGone => return,
                    Ended::Lost(error) => error,
                }
            }
            Err(error) => error,
        };
        if let Some(first) = first.take() {
            let _ = first.send(Err(error.clone()));
        }
        let lost = Item::Disconnected {
            error,
            retry_in: retry,
        };
        if items.send(lost).is_err() || !wait(retry, &mut subscription, commands).await {
            return;
        }
        retry = (retry * 2).min(MAX_RETRY);
    }
}

/// Carries out the commands the app has sent, without waiting for more: a
/// new subscription is written to the socket at once. An error is the end
/// of the connection, and why.
fn take_commands(
    socket: &mut transport::Socket,
    subscription: &mut Subscription,
    commands: &Receiver<Command>,
) -> Result<(), Ended> {
    loop {
        match commands.try_recv() {
            Ok(Command::Subscribe {
                subscription: next,
                ack,
            }) => {
                *subscription = next;
                let sent = transport::send(socket, subscribe_text(subscription));
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
                sent.map_err(Ended::Lost)?;
            }
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => {
                transport::close(socket);
                return Err(Ended::AppGone);
            }
        }
    }
}

/// Waits `delay` before the next attempt, taking the app's commands
/// meanwhile; `false` when the app is gone.
async fn wait(
    delay: Duration,
    subscription: &mut Subscription,
    commands: &Receiver<Command>,
) -> bool {
    let until = Instant::now() + delay;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        match background::recv_timeout(commands, left).await {
            Ok(Command::Subscribe {
                subscription: next,
                ack,
            }) => {
                *subscription = next;
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
            }
            Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use tungstenite::{Message as Frame, WebSocket};

    use super::*;
    use crate::hub::NodeType;

    const SESSION: &str = "5e55e55e000040008000000000000001";

    fn fixture(name: &str) -> Message {
        let path = format!(
            "{}/tests/fixtures/events/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        parse(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
    }

    fn event(name: &str) -> (Option<u64>, Envelope) {
        match fixture(name) {
            Message::Event { seq, envelope } => (seq, *envelope),
            other => panic!("{name} read as {other:?}"),
        }
    }

    fn system(name: &str) -> (Option<u64>, System) {
        match fixture(name) {
            Message::System { seq, system } => (seq, system),
            other => panic!("{name} read as {other:?}"),
        }
    }

    #[test]
    fn every_event_this_client_acts_on_reads_in_the_contract_shape() {
        let (seq, node) = event("node_stored");
        assert_eq!(seq, Some(7));
        assert_eq!(
            (
                node.session_id.as_str(),
                node.graph.as_str(),
                node.event_type.as_str()
            ),
            (SESSION, "fixture", "node_stored")
        );
        let Event::NodeStored(view) = node.event else {
            panic!("{:?}", node.event)
        };
        assert_eq!(view.node_type, NodeType::Fact);
        assert_eq!(view.content, "Swifts sleep on the wing.");
        assert_eq!(view.confidence, None);

        let (_, edge) = event("edge_stored");
        let Event::EdgeStored(view) = edge.event else {
            panic!("{:?}", edge.event)
        };
        assert_eq!(view.edge_type, "extracted_under_topic");
        assert_eq!(view.src_id, "fac70000-0000-4000-8000-000000000003");

        let (_, status) = event("node_status_changed");
        assert_eq!(
            status.event,
            Event::NodeStatusChanged(StatusChange {
                node_id: "fac70000-0000-4000-8000-000000000002".into(),
                old_status: "active".into(),
                new_status: "corrected".into(),
                counterpart: Some("fac70000-0000-4000-8000-000000000003".into()),
            })
        );

        let (_, timeline) = event("timeline_stored");
        let Event::TimelineStored(view) = timeline.event else {
            panic!("{:?}", timeline.event)
        };
        assert_eq!(view.timepoints.len(), 2);
        assert_eq!(view.timepoints[1].kind, "interval");

        let (seq, switched) = event("graph_switched");
        assert_eq!(seq, Some(11));
        assert_eq!(
            switched.event,
            Event::GraphSwitched {
                previous_graph: "scratch".into(),
                new_graph: "fixture".into()
            }
        );
    }

    #[test]
    fn a_retrieval_record_reads_in_the_contract_shape() {
        let (seq, recorded) = event("retrieval_recorded");
        assert_eq!(seq, Some(15));
        assert_eq!(
            (recorded.session_id.as_str(), recorded.graph.as_str()),
            (SESSION, "fixture")
        );
        let Event::RetrievalRecorded(record) = recorded.event else {
            panic!("{:?}", recorded.event)
        };
        assert_eq!(
            (
                record.record_id.as_str(),
                record.tool.as_str(),
                record.graph.as_str()
            ),
            ("000000000042", "epimemer.search", "fixture")
        );
        assert_eq!(record.at, "2026-10-20T18:03:02.400000Z");
        let retrieved = record.retrieved.as_deref().unwrap();
        assert_eq!(retrieved.len(), 3);
        assert_eq!(
            (retrieved[1].provenance.as_str(), retrieved[1].score),
            ("expanded", None)
        );
        assert!(!record.truncated);

        let undeclared = r#"{"event_type": "retrieval_recorded", "graph": "g", "session_id": "s",
            "record": {"record_id": "000000000001", "at": "2026-11-07T10:00:00Z",
              "tool": "epimemer.segment", "query": "", "graph": "g", "retrieved": null,
              "response_text": "", "truncated": false}, "seq": 2}"#;
        let Message::Event { envelope, .. } = parse(undeclared) else {
            panic!()
        };
        assert!(
            matches!(&envelope.event, Event::RetrievalRecorded(r) if r.retrieved.is_none()),
            "{:?}",
            envelope.event
        );
        let broken = r#"{"event_type": "retrieval_recorded", "graph": "g", "session_id": "s",
            "record": {"record_id": "000000000001"}, "seq": 3}"#;
        let Message::Event { envelope, .. } = parse(broken) else {
            panic!()
        };
        assert!(matches!(envelope.event, Event::Unreadable(_)));
    }

    #[test]
    fn system_messages_read_from_either_hub() {
        let (seq, connected) = system("session_connected");
        assert_eq!(seq, Some(1));
        let System::SessionConnected(session) = connected else {
            panic!("{connected:?}")
        };
        assert_eq!(
            (session.session_id.as_str(), session.schema_version),
            (SESSION, Some(1))
        );
        // Epimemer 0.2.11's hub, older than the contract, lists no version.
        let (_, old) = system("session_connected_unversioned");
        let System::SessionConnected(session) = old else {
            panic!("{old:?}")
        };
        assert_eq!(session.schema_version, None);
        assert_eq!(session.last_event_at, None);
        assert_eq!(
            system("session_disconnected"),
            (Some(12), System::SessionDisconnected(SESSION.into()))
        );
        assert_eq!(
            system("session_dropped"),
            (Some(13), System::SessionDropped(SESSION.into()))
        );
        assert_eq!(
            parse(r#"{"type": "hub_restarting", "seq": 5}"#),
            Message::System {
                seq: Some(5),
                system: System::Other("hub_restarting".into())
            }
        );
    }

    #[test]
    fn an_act_and_an_advisory_read_in_the_contract_shape() {
        let (seq, act) = event("graph_action_recorded");
        assert_eq!(seq, Some(3));
        assert_eq!(
            (act.graph.as_str(), act.timestamp.as_str()),
            ("fixture", "2026-10-20T18:02:11.200000Z")
        );
        let Event::ActRecorded(act) = act.event else {
            panic!("{:?}", act.event)
        };
        assert_eq!(
            (
                act.action_id.as_str(),
                act.verb.as_str(),
                act.summary.as_str()
            ),
            ("000000000031", "stored", "stored 1 node")
        );
        assert_eq!(act.subjects, ["fac70000-0000-4000-8000-000000000003"]);
        assert_eq!(
            act.counts
                .iter()
                .map(|(k, v)| (k.as_str(), *v))
                .collect::<Vec<_>>(),
            [
                ("edges", 1),
                ("embeddings", 1),
                ("nodes", 1),
                ("timelines", 0)
            ]
        );
        assert_eq!(act.judged_by, None);

        let (seq, advisory) = event("advisory_raised");
        assert_eq!(seq, Some(5));
        let Event::AdvisoryRaised(advisory) = advisory.event else {
            panic!("{:?}", advisory.event)
        };
        assert_eq!(
            (
                advisory.action_id.as_str(),
                advisory.tool.as_str(),
                advisory.kind.as_str(),
                advisory.action.as_str()
            ),
            (
                "000000000032",
                "store_decomposition",
                "tag_joined_topic",
                "flag"
            )
        );
        assert!(advisory.message.starts_with("The tag 'swifts'"));
        assert_eq!(advisory.subjects.len(), 2);
        assert_eq!(advisory.detail["tag"], "swifts");
        assert_eq!((advisory.surfaced, advisory.notify_user), (false, true));
        assert_eq!(
            advisory.judged_by.as_deref(),
            Some("a9e00000-0000-4000-8000-000000000001")
        );

        // A verb, a kind and an action the contract does not list yet are
        // kept as they arrive.
        let future = r#"{"event_type": "advisory_raised", "graph": "g", "session_id": "s",
            "timestamp": "2026-11-07T10:00:00Z", "action_id": "000000000002", "tool": "reflect",
            "kind": "a_future_kind", "message": "m", "subjects": [], "detail": {},
            "action": "a_future_action", "surfaced": true, "notify_user": false, "seq": 2}"#;
        let Message::Event { envelope, .. } = parse(future) else {
            panic!()
        };
        assert!(
            matches!(&envelope.event, Event::AdvisoryRaised(a)
                if a.kind == "a_future_kind" && a.action == "a_future_action" && a.judged_by.is_none()),
            "{:?}",
            envelope.event
        );
        let decision = r#"{"event_type": "graph_action_recorded", "graph": "g", "session_id": "s",
            "action_id": "000000000003", "verb": "recurrence_exception", "subjects": ["tl"],
            "counts": {"a_future_count": 2}, "summary": "s", "judged_by": "j", "seq": 3}"#;
        let Message::Event { envelope, .. } = parse(decision) else {
            panic!()
        };
        assert!(
            matches!(&envelope.event, Event::ActRecorded(a)
                if a.verb == "recurrence_exception" && a.counts["a_future_count"] == 2),
            "{:?}",
            envelope.event
        );
        // An act numbered as an integer, not the contract's padded string,
        // does not read.
        let numbered = r#"{"event_type": "graph_action_recorded", "graph": "g", "session_id": "s",
            "action_id": 31, "verb": "stored", "subjects": [], "counts": {}, "summary": "s",
            "seq": 4}"#;
        let Message::Event { envelope, .. } = parse(numbered) else {
            panic!()
        };
        assert!(
            matches!(&envelope.event, Event::Unreadable(e) if e.contains("string")),
            "{:?}",
            envelope.event
        );
    }

    #[test]
    fn the_reflect_counter_reads_in_the_contract_shape() {
        let (seq, counter) = event("reflect_counter_updated");
        assert_eq!(seq, Some(6));
        assert_eq!(counter.graph, "fixture");
        assert_eq!(
            counter.event,
            Event::ReflectCounterUpdated(Reflect {
                count: 9,
                threshold: 10,
                suggested: false
            })
        );
    }

    #[test]
    fn the_six_pipeline_events_read_in_the_contract_shape() {
        let pipeline = |name: &str| {
            let (_, envelope) = event(name);
            assert_eq!(
                envelope.graph, "",
                "{name}: a pipeline event names no graph"
            );
            assert_eq!(envelope.event_type, name);
            match envelope.event {
                Event::Pipeline(p) => *p,
                other => panic!("{name} read as {other:?}"),
            }
        };
        let Pipeline::Started {
            pipeline_name,
            place_names,
            transition_names,
            edges,
        } = pipeline("pipeline_started")
        else {
            panic!()
        };
        assert_eq!(
            (pipeline_name.as_str(), place_names.len(), transition_names),
            (
                "reflect",
                3,
                vec!["topic_consolidation".to_owned(), "pending_review".into()]
            )
        );
        assert_eq!(edges.len(), 4);
        assert_eq!(
            (edges[0].label.as_deref(), edges[2].label.as_deref()),
            (None, Some("pairs"))
        );
        assert_eq!(
            pipeline("transition_fired"),
            Pipeline::TransitionFired {
                pipeline_name: "store".into(),
                transition_name: "embed".into(),
                input_places: vec!["claims".into()],
            }
        );
        assert_eq!(
            pipeline("transition_completed"),
            Pipeline::TransitionCompleted {
                pipeline_name: "store".into(),
                transition_name: "embed".into(),
                output_places: vec!["embedded".into()],
                duration_ms: 148.25,
            }
        );
        assert_eq!(
            pipeline("tokens_updated"),
            Pipeline::TokensUpdated {
                pipeline_name: "store".into(),
                place_token_counts: [("claims".to_owned(), 0), ("embedded".to_owned(), 3)]
                    .into_iter()
                    .collect(),
            }
        );
        assert_eq!(
            pipeline("pipeline_completed"),
            Pipeline::Completed {
                pipeline_name: "store".into(),
                transitions_fired: 4,
                duration_ms: 1520.5,
            }
        );
        assert_eq!(
            pipeline("pipeline_failed"),
            Pipeline::Failed {
                pipeline_name: "reflect".into(),
                error: "the embedder did not answer".into(),
                transition_name: Some("pending_review".into()),
                transitions_fired: 1,
                duration_ms: 30012.0,
            }
        );
        // A failure outside any transition names none, and a whole number
        // of milliseconds reads.
        let early = r#"{"event_type": "pipeline_failed", "category": "pipeline", "graph": "",
            "session_id": "s", "pipeline_name": "store", "error": "e", "transition_name": null,
            "transitions_fired": 0, "duration_ms": 3, "seq": 9}"#;
        let Message::Event { envelope, .. } = parse(early) else {
            panic!()
        };
        assert!(
            matches!(&envelope.event, Event::Pipeline(p) if matches!(**p,
                Pipeline::Failed { transition_name: None, duration_ms, .. } if duration_ms == 3.0)),
            "{:?}",
            envelope.event
        );
        let broken = r#"{"event_type": "transition_completed", "pipeline_name": "store",
            "transition_name": "embed", "session_id": "s", "seq": 10}"#;
        let Message::Event { envelope, .. } = parse(broken) else {
            panic!()
        };
        assert!(
            matches!(&envelope.event, Event::Unreadable(e) if e.contains("output_places")),
            "{:?}",
            envelope.event
        );
    }

    #[test]
    fn events_this_client_does_not_act_on_are_received_and_kept_only_for_their_seq() {
        let (read_seq, envelope) = event("a_future_event");
        assert_eq!(read_seq, Some(14));
        assert_eq!(envelope.event, Event::Other);
        assert_eq!(envelope.event_type, "a_future_event");
        let embedding = r#"{"event_type": "embedding_stored", "category": "graph", "graph": "",
            "item_id": "n", "model_id": "m", "dimensions": 768, "session_id": "s", "seq": 1}"#;
        let old_pipeline = r#"{"event_type": "transition_enabled", "category": "pipeline",
            "pipeline_name": "store", "transition_name": "embed", "session_id": "s", "seq": 2}"#;
        for text in [embedding, old_pipeline] {
            assert!(matches!(
                parse(text),
                Message::Event { envelope, .. } if envelope.event == Event::Other
            ));
        }
    }

    #[test]
    fn a_body_that_does_not_read_is_said_so_and_keeps_its_seq() {
        let broken = r#"{"event_type": "node_stored", "graph": "g", "session_id": "s",
            "node": {"node_id": "n"}, "seq": 9}"#;
        let Message::Event { seq, envelope } = parse(broken) else {
            panic!()
        };
        assert_eq!(seq, Some(9));
        assert!(
            matches!(&envelope.event, Event::Unreadable(e) if e.contains("node_type")),
            "{:?}",
            envelope.event
        );
        assert!(matches!(
            parse("[1, 2]"),
            Message::Unreadable { seq: None, .. }
        ));
        assert!(matches!(
            parse("not json"),
            Message::Unreadable { seq: None, .. }
        ));
        assert!(matches!(
            parse(r#"{"seq": 4, "hello": 1}"#),
            Message::Unreadable { seq: Some(4), .. }
        ));
        assert!(matches!(
            parse(r#"{"type": "session_connected", "seq": 6}"#),
            Message::Unreadable { seq: Some(6), .. }
        ));
    }

    #[test]
    fn the_subscription_names_the_session_and_its_graphs() {
        let one = Subscription {
            session: "abc".into(),
            graphs: vec!["memory".into()],
        };
        let sent: Value = serde_json::from_str(&subscribe_text(&one)).unwrap();
        assert_eq!(
            sent,
            serde_json::json!({"subscribe": {"session": "abc", "graphs": ["memory"]}})
        );
        let all = Subscription {
            session: "abc".into(),
            graphs: Vec::new(),
        };
        let sent: Value = serde_json::from_str(&subscribe_text(&all)).unwrap();
        assert_eq!(sent, serde_json::json!({"subscribe": {"session": "abc"}}));
    }

    /// Waits for the next item, failing after a few seconds.
    fn next(stream: &Stream) -> Item {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(item) = drain_one(stream) {
                return item;
            }
            assert!(Instant::now() < deadline, "no item came");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn drain_one(stream: &Stream) -> Option<Item> {
        stream.items.lock().unwrap().try_recv().ok()
    }

    fn read_text(socket: &mut WebSocket<TcpStream>) -> String {
        loop {
            match socket.read().unwrap() {
                Frame::Text(text) => return text.to_string(),
                _ => continue,
            }
        }
    }

    #[test]
    fn the_client_subscribes_follows_and_reconnects_with_its_subscription() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hub = listener.local_addr().unwrap().to_string();
        let subscription = Subscription {
            session: "first".into(),
            graphs: vec!["memory".into()],
        };
        let started = start(&hub, subscription).unwrap();
        let stream = started.stream;

        // The first connection: the subscription, then two messages, one
        // that this client acts on and one it drops.
        let (tcp, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(tcp).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&read_text(&mut socket)).unwrap(),
            serde_json::json!({"subscribe": {"session": "first", "graphs": ["memory"]}})
        );
        assert_eq!(
            started.first.recv_timeout(Duration::from_secs(5)),
            Ok(Ok(1))
        );
        assert_eq!(next(&stream), Item::Connected { connection: 1 });
        socket
            .send(Frame::text(
                r#"{"type": "session_dropped", "session_id": "x", "seq": 1}"#,
            ))
            .unwrap();
        socket
            .send(Frame::text(
                r#"{"event_type": "tokens_updated", "session_id": "first", "seq": 2}"#,
            ))
            .unwrap();
        assert_eq!(
            next(&stream),
            Item::Message(Message::System {
                seq: Some(1),
                system: System::SessionDropped("x".into())
            })
        );
        assert!(matches!(
            next(&stream),
            Item::Message(Message::Event { seq: Some(2), .. })
        ));

        // A new subscription is written at once and answered.
        let (ack, acked) = mpsc::channel();
        let second = Subscription {
            session: "second".into(),
            graphs: Vec::new(),
        };
        resubscribe(&stream, second, Some(ack));
        assert_eq!(
            serde_json::from_str::<Value>(&read_text(&mut socket)).unwrap(),
            serde_json::json!({"subscribe": {"session": "second"}})
        );
        assert_eq!(acked.recv_timeout(Duration::from_secs(5)), Ok(()));

        // The hub goes away: the client says so, waits, and connects again
        // with the subscription it follows now.
        drop(socket);
        let Item::Disconnected { retry_in, .. } = next(&stream) else {
            panic!("expected a disconnection")
        };
        assert_eq!(retry_in, FIRST_RETRY);
        let (tcp, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(tcp).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&read_text(&mut socket)).unwrap(),
            serde_json::json!({"subscribe": {"session": "second"}})
        );
        assert_eq!(next(&stream), Item::Connected { connection: 2 });

        // Dropping the stream ends the thread, which closes its socket.
        drop(stream);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match socket.read() {
                Ok(Frame::Close(_)) | Err(_) => break,
                Ok(_) => assert!(Instant::now() < deadline, "the socket stayed open"),
            }
        }
    }

    #[test]
    fn a_hub_that_is_not_there_is_tried_again_with_a_growing_wait() {
        // A port nothing listens on: bind one, note it, and let it go.
        let hub = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().to_string()
        };
        let started = start(
            &hub,
            Subscription {
                session: "s".into(),
                graphs: Vec::new(),
            },
        )
        .unwrap();
        assert!(matches!(
            started.first.recv_timeout(Duration::from_secs(5)),
            Ok(Err(_))
        ));
        let waits: Vec<Duration> = (0..3)
            .map(|_| match next(&started.stream) {
                Item::Disconnected { retry_in, .. } => retry_in,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(waits, [FIRST_RETRY, FIRST_RETRY * 2, FIRST_RETRY * 4]);
    }
}
