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
//! are read into the contract's own structs (`hub.rs`); every other event,
//! the pipeline events, the acts, the advisories, the retrieval records and
//! any type added later, is [`Event::Other`], received and dropped. A
//! message's `seq` is kept whatever it is, since a gap in it means events
//! were lost. The hub is plain `ws://` on loopback (VIZ_CONTRACT.md §2), so
//! the native client has no TLS.
//!
//! Two hubs are read the same way. The contract's hub lists a session with
//! its `schema_version`; Epimemer 0.2.11's, which is older than the
//! contract, lists none, and [`SessionInfo`] reads its absence as `None`.
//! That hub also sends `transition_enabled`, a pipeline event the contract
//! no longer has; it is one more [`Event::Other`].

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

use bevy::prelude::Resource;
use serde::Deserialize;
use serde_json::{Map, Value};
use web_time::Instant;

use crate::background;
use crate::hub::{EdgeView, NodeView, SessionInfo, TimelineView};

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
    /// The graph it is about; empty on a few events.
    pub graph: String,
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
    fn events_this_client_does_not_act_on_are_received_and_kept_only_for_their_seq() {
        for (name, event_type, seq) in [
            ("graph_action_recorded", "graph_action_recorded", 3),
            ("transition_fired", "transition_fired", 4),
            ("a_future_event", "a_future_event", 14),
        ] {
            let (read_seq, envelope) = event(name);
            assert_eq!(read_seq, Some(seq), "{name}");
            assert_eq!(envelope.event, Event::Other, "{name}");
            assert_eq!(envelope.event_type, event_type);
        }
        let old_pipeline = r#"{"event_type": "transition_enabled", "category": "pipeline",
            "pipeline_name": "store", "transition_name": "embed", "session_id": "s", "seq": 2}"#;
        assert!(matches!(
            parse(old_pipeline),
            Message::Event { envelope, .. } if envelope.event == Event::Other
        ));
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
