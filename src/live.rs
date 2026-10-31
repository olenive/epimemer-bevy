//! Live updates (DESIGN.md §11): the event stream (`stream.rs`) drained
//! once per update and applied to the picture (`apply.rs`), and the
//! snapshot read again whenever the stream cannot be trusted to have said
//! everything.
//!
//! [`drain_stream`] takes every item the stream's thread has sent since the
//! last update. A connection opening resets the expected `seq`, and unless
//! its subscription was sent before the snapshot drawn was asked for, the
//! snapshot is read again, because events sent while the client was away
//! were lost. A gap in `seq` asks for the same, and so does a
//! `session_connected` for the session followed after it was seen
//! disconnected, since a session that cannot reach the hub drops its events
//! (VIZ_CONTRACT.md §7.5). The read goes through `reload.rs` as a
//! [`Refetch`], over the picture drawn, and while it, or a switch to another
//! graph, is under way, the events of the graph being read are held and
//! applied once it is drawn; applying an event the snapshot already holds
//! changes nothing. A read that fails is tried again after
//! [`REFETCH_RETRY`] while the stream is connected.
//!
//! System messages keep the bar's session list up to date, and
//! `graph_switched` a session's active graph there; neither changes what is
//! drawn. An event for another session or another graph is dropped, and so
//! are the events this client does not act on.
//!
//! [`LiveState`] says how the stream stands, for the bar: connected, lost
//! and trying again with the last error, or reading the graph again; and
//! with `--log-events` every event applied is printed to stderr in one line
//! with what it did and the counts after it.

use std::time::Duration;

use bevy::prelude::*;
use web_time::Instant;

use crate::apply::{self, Changes, LiveGraph, Outcome, Picture};
use crate::camera::Places;
use crate::focus::{Focus, Hovered};
use crate::graph_index::GraphIndex;
use crate::hub::SessionInfo;
use crate::pickers::{self, Pickers, short_id};
use crate::relayout;
use crate::reload::{self, Catalogue, LoadStatus, Loading, Refetch, receive_loads};
use crate::stream::{self, Envelope, Event, Item, Message, Stream, Subscription, System};
use crate::time_axis::{TimeMode, TimeModel};
use crate::time_mode;

/// How long after a read of the graph failed it is tried again.
pub const REFETCH_RETRY: Duration = Duration::from_secs(5);
/// The most events held while a graph is read.
pub const MAX_HELD: usize = 20_000;
/// A batch at least this long is logged with the time it took.
const BURST: usize = 20;

/// How the stream's connection stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    /// There is no stream.
    #[default]
    Off,
    Connecting,
    /// Connected, on the connection numbered.
    Live {
        connection: u32,
    },
    /// Lost, or never made; the thread tries again after `retry_in`.
    Lost {
        error: String,
        retry_in: Duration,
    },
}

/// An event held, with its `seq`.
pub type Held = (Option<u64>, Box<Envelope>);

/// Where the stream stands and what it has done.
#[derive(Resource, Clone, Debug, Default)]
pub struct LiveState {
    pub connection: Connection,
    /// The connection whose subscription was sent before the snapshot
    /// drawn was asked for: it needs no second read.
    pub subscribed_before_load: Option<u32>,
    /// The session and graph the stream was last asked to follow by a
    /// switch, where one did.
    pub following: Option<(String, String)>,
    /// The `seq` of the last message on this connection.
    pub last_seq: Option<u64>,
    /// Events held while a graph is read.
    pub held: Vec<Held>,
    /// A read of the graph drawn wanted, and why.
    pub wanted: Option<String>,
    /// When to try again a read that failed.
    pub retry_at: Option<Instant>,
    /// Whether the load under way is a read this module asked for.
    pub refetching: bool,
    /// Whether a load was under way at the last update.
    pub was_loading: bool,
    /// Messages received, events applied, and reads asked for.
    pub received: usize,
    pub applied: usize,
    pub refetches: usize,
    /// Print every event applied to stderr.
    pub log_events: bool,
}

/// Drains the stream once per update and applies what it brings.
pub struct LivePlugin {
    pub log_events: bool,
}

impl Plugin for LivePlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<LiveState>() {
            app.insert_resource(LiveState::default());
        }
        app.world_mut().resource_mut::<LiveState>().log_events = self.log_events;
        app.add_message::<Refetch>()
            .add_systems(PreUpdate, drain_stream.after(receive_loads));
    }
}

/// Asks for the graph drawn to be read again, once however many reasons
/// come.
fn want(state: &mut LiveState, reason: String) {
    if state.wanted.is_none() {
        if state.log_events {
            eprintln!("stream: the graph will be read again: {reason}");
        }
        info!("live: the graph will be read again: {reason}");
        state.wanted = Some(reason);
    }
}

/// The session and graph whose events are wanted: those being read while a
/// load is under way, else those drawn.
fn target(world: &World, loading: bool) -> (String, String) {
    if loading
        && let Some(LoadStatus::Loading { graph, session }) = world.get_resource::<LoadStatus>()
    {
        return (session.clone(), graph.clone());
    }
    world
        .get_resource::<Catalogue>()
        .map_or_else(Default::default, |c| {
            (c.session.session_id.clone(), c.graph.clone())
        })
}

/// Takes what the stream has sent since the last update, and applies the
/// events of the graph drawn, or holds them while a graph is read.
pub fn drain_stream(world: &mut World) {
    let items = world
        .get_resource::<Stream>()
        .map(stream::drain)
        .unwrap_or_default();
    let loading = world
        .get_resource::<Loading>()
        .is_some_and(|l| l.0.is_some());
    let Some(state) = world.get_resource::<LiveState>() else {
        return;
    };
    let retry_due = state.retry_at.is_some_and(|at| Instant::now() >= at);
    let idle = items.is_empty()
        && loading == state.was_loading
        && (loading || (state.held.is_empty() && state.wanted.is_none() && !retry_due));
    if idle {
        return;
    }
    let mut state = world.remove_resource::<LiveState>().unwrap_or_default();
    let mut to_apply: Vec<Held> = Vec::new();
    for item in items {
        take(world, &mut state, item, loading, &mut to_apply);
    }
    if state.was_loading && !loading {
        load_ended(world, &mut state);
    }
    state.was_loading = loading;
    if !loading {
        if retry_due && matches!(state.connection, Connection::Live { .. }) {
            state.retry_at = None;
            want(&mut state, "trying again a read that failed".into());
        }
        if let Some(reason) = state.wanted.take() {
            world.write_message(Refetch { reason });
            state.refetching = true;
            state.refetches += 1;
        } else if !state.held.is_empty() {
            let (session, graph) = target(world, false);
            let mut held: Vec<Held> = std::mem::take(&mut state.held)
                .into_iter()
                .filter(|(_, e)| {
                    e.session_id == session && (e.graph.is_empty() || e.graph == graph)
                })
                .collect();
            held.append(&mut to_apply);
            to_apply = held;
        }
    }
    if !to_apply.is_empty() {
        apply_batch(world, &mut state, to_apply);
    }
    world.insert_resource(state);
}

/// Takes one item from the stream.
fn take(
    world: &mut World,
    state: &mut LiveState,
    item: Item,
    loading: bool,
    to_apply: &mut Vec<Held>,
) {
    match item {
        Item::Connected { connection } => {
            state.connection = Connection::Live { connection };
            state.last_seq = None;
            info!("live: the event stream is connected (connection {connection})");
            if state.log_events {
                eprintln!("stream: connected (connection {connection})");
            }
            if state.subscribed_before_load == Some(connection) {
                state.subscribed_before_load = None;
            } else {
                want(
                    state,
                    format!(
                        "the event stream connected again (connection {connection}), so events may have been missed"
                    ),
                );
            }
        }
        Item::Disconnected { error, retry_in } => {
            warn!(
                "live: the event stream is lost ({error}); trying again in {:.1} s",
                retry_in.as_secs_f32()
            );
            if state.log_events {
                eprintln!(
                    "stream: lost ({error}); trying again in {:.1} s",
                    retry_in.as_secs_f32()
                );
            }
            state.connection = Connection::Lost { error, retry_in };
        }
        Item::Message(message) => {
            state.received += 1;
            if let Some(seq) = stream::seq(&message) {
                if let Some(last) = state.last_seq
                    && seq != last + 1
                {
                    want(
                        state,
                        format!("a gap in the event stream, seq {seq} after {last}"),
                    );
                }
                state.last_seq = Some(seq);
            }
            match message {
                Message::System { system, .. } => system_message(world, state, system),
                Message::Event { seq, envelope } => {
                    route(world, state, seq, envelope, loading, to_apply)
                }
                Message::Unreadable { error, .. } => {
                    warn!("live: a message this client cannot read: {error}");
                }
            }
        }
    }
}

/// Sends an event on to be applied, or holds it while a graph is read, or
/// drops it where it is about another session or graph or nothing drawn.
fn route(
    world: &mut World,
    state: &mut LiveState,
    seq: Option<u64>,
    envelope: Box<Envelope>,
    loading: bool,
    to_apply: &mut Vec<Held>,
) {
    if let Event::GraphSwitched { new_graph, .. } = &envelope.event {
        set_active_graph(world, &envelope.session_id, new_graph);
    }
    let (session, graph) = target(world, loading);
    let elsewhere =
        envelope.session_id != session || (!envelope.graph.is_empty() && envelope.graph != graph);
    if elsewhere || matches!(envelope.event, Event::Other | Event::GraphSwitched { .. }) {
        if state.log_events {
            let why = if elsewhere {
                "about another session or graph"
            } else {
                "not acted on"
            };
            let seq = seq.map_or_else(|| "-".to_owned(), |s| s.to_string());
            eprintln!("event {seq} {}: received, {why}", envelope.event_type);
        }
        return;
    }
    if loading || state.wanted.is_some() {
        if state.held.len() < MAX_HELD {
            state.held.push((seq, envelope));
        } else {
            want(
                state,
                "more events came while the graph was read than can be held".into(),
            );
        }
        return;
    }
    to_apply.push((seq, envelope));
}

/// Keeps the bar's session list up to date with a system message.
fn system_message(world: &mut World, state: &mut LiveState, system: System) {
    if state.log_events {
        eprintln!("stream: {system:?}");
    }
    let Some(mut catalogue) = world.get_resource_mut::<Catalogue>() else {
        return;
    };
    match system {
        System::SessionConnected(info) => {
            let followed = info.session_id == catalogue.session.session_id;
            let was_connected = catalogue.session.connected;
            upsert(&mut catalogue, info.clone());
            if followed {
                if catalogue.session != info {
                    catalogue.session = info;
                }
                if !was_connected {
                    want(
                        state,
                        "the session followed is connected to the hub again, and the events it sent meanwhile were dropped".into(),
                    );
                }
            }
        }
        System::SessionDisconnected(id) => {
            if let Some(session) = catalogue.sessions.iter_mut().find(|s| s.session_id == id)
                && session.connected
            {
                session.connected = false;
            }
            if catalogue.session.session_id == id {
                catalogue.session.connected = false;
                warn!(
                    "live: the session followed, {}, is disconnected from the hub",
                    short_id(&id)
                );
            }
        }
        System::SessionDropped(id) => {
            if catalogue.sessions.iter().any(|s| s.session_id == id) {
                catalogue.sessions.retain(|s| s.session_id != id);
            }
            if catalogue.session.session_id == id {
                warn!(
                    "live: the session followed, {}, has gone; the graph stays drawn as it was",
                    short_id(&id)
                );
            }
        }
        System::Other(kind) => debug!("live: a system message this client does not know: {kind}"),
    }
}

/// Puts a session's entry into the list, replacing the old one, writing
/// only when it changed.
fn upsert(catalogue: &mut Catalogue, info: SessionInfo) {
    match catalogue
        .sessions
        .iter_mut()
        .find(|s| s.session_id == info.session_id)
    {
        Some(old) if *old == info => {}
        Some(old) => *old = info,
        None => catalogue.sessions.push(info),
    }
}

/// A session's active graph changed.
fn set_active_graph(world: &mut World, session: &str, graph: &str) {
    let Some(mut catalogue) = world.get_resource_mut::<Catalogue>() else {
        return;
    };
    if let Some(entry) = catalogue
        .sessions
        .iter_mut()
        .find(|s| s.session_id == session && s.active_graph != graph)
    {
        entry.active_graph = graph.to_owned();
    }
    if catalogue.session.session_id == session && catalogue.session.active_graph != graph {
        catalogue.session.active_graph = graph.to_owned();
    }
}

/// A load has ended. A switch that failed leaves the graph drawn as it was,
/// so the stream follows it again and it is read again, since its events
/// went unheard meanwhile; a read of it that failed is tried again later.
fn load_ended(world: &mut World, state: &mut LiveState) {
    let failed = matches!(
        world.get_resource::<LoadStatus>(),
        Some(LoadStatus::Failed(_))
    );
    let drawn = target(world, false);
    if failed {
        if state.following.as_ref().is_some_and(|f| *f != drawn) {
            if let Some(stream) = world.get_resource::<Stream>() {
                let subscription = Subscription {
                    session: drawn.0.clone(),
                    graphs: vec![drawn.1.clone()],
                };
                stream::resubscribe(stream, subscription, None);
            }
            state.following = Some(drawn);
            want(
                state,
                "a switch failed, and the graph drawn went unheard meanwhile".into(),
            );
        } else if state.refetching {
            state.retry_at = Some(Instant::now() + REFETCH_RETRY);
        }
    } else {
        state.following = Some(drawn);
    }
    state.refetching = false;
}

/// One line for `--log-events`: the event, what it did, and the counts
/// after it.
pub fn event_line(
    seq: Option<u64>,
    envelope: &Envelope,
    outcome: &Outcome,
    counts: (usize, usize),
    applied: usize,
) -> String {
    let subject = match &envelope.event {
        Event::NodeStored(node) => format!(
            "{:?} {} {:?}",
            node.node_type,
            short_id(&node.node_id),
            crate::labels::label_text(&node.content, &node.node_id)
        ),
        Event::EdgeStored(edge) => format!(
            "{} {} to {}",
            edge.edge_type,
            short_id(&edge.src_id),
            short_id(&edge.dst_id)
        ),
        Event::NodeStatusChanged(change) => format!(
            "{} {} to {}",
            short_id(&change.node_id),
            change.old_status,
            change.new_status
        ),
        Event::TimelineStored(timeline) => format!(
            "{:?} with {} timepoints",
            timeline.name,
            timeline.timepoints.len()
        ),
        Event::GraphSwitched { new_graph, .. } => format!("to {new_graph:?}"),
        Event::Other => String::new(),
        Event::Unreadable(error) => format!("unreadable: {error}"),
    };
    let did = match outcome {
        Outcome::Applied(what) => what.clone(),
        Outcome::Unchanged(why) => format!("unchanged, {why}"),
        Outcome::Refetch(why) => format!("the graph will be read again: {why}"),
    };
    let seq = seq.map_or_else(|| "-".to_owned(), |s| s.to_string());
    format!(
        "event {seq} {} {subject}: {did}; {} nodes drawn, {} edges as lines, {applied} events applied",
        envelope.event_type, counts.0, counts.1
    )
}

/// Applies a batch of events to the picture, in order, and then to the
/// scene once.
fn apply_batch(world: &mut World, state: &mut LiveState, events: Vec<Held>) {
    let started = Instant::now();
    time_mode::finish_switch(world);
    relayout::finish_tween(world);
    let mode = world
        .get_resource::<TimeMode>()
        .copied()
        .unwrap_or_default();
    let taken = (
        world.remove_resource::<GraphIndex>(),
        world.remove_resource::<Places>(),
        world.remove_resource::<TimeModel>(),
        world.remove_resource::<LiveGraph>(),
    );
    let (Some(mut index), Some(mut places), Some(mut time), Some(mut live)) = taken else {
        // Not a picture to apply to: put back what there was.
        let (index, places, time, live) = taken;
        if let Some(index) = index {
            world.insert_resource(index);
        }
        if let Some(places) = places {
            world.insert_resource(places);
        }
        if let Some(time) = time {
            world.insert_resource(time);
        }
        if let Some(live) = live {
            world.insert_resource(live);
        }
        return;
    };
    let mut changes = Changes::default();
    let mut picture = Picture {
        index: &mut index,
        places: &mut places,
        time: &mut time,
        live: &mut live,
        mode,
        wall_clock: reload::wall_clock(),
    };
    let count = events.len();
    for (seq, envelope) in &events {
        let outcome = apply::apply(&mut picture, &envelope.event, &mut changes);
        state.applied += 1;
        if let Outcome::Refetch(reason) = &outcome {
            want(state, reason.clone());
        }
        if state.log_events {
            let line = event_line(
                *seq,
                envelope,
                &outcome,
                apply::counts(picture.index),
                state.applied,
            );
            eprintln!("{line}");
        }
    }
    apply::finish(&mut picture, &changes);
    let lists = changes
        .pickers
        .then(|| pickers::build(picture.index, &picture.live.metacontexts));
    let (nodes, _) = apply::counts(&index);
    world.insert_resource(index);
    world.insert_resource(places);
    world.insert_resource(time);
    world.insert_resource(live);
    if let (Some(lists), Some(mut pickers)) = (lists, world.get_resource_mut::<Pickers>()) {
        pickers.set_if_neq(lists);
    }
    apply::realize(world, &changes);
    let gone = |node: Option<usize>| node.filter(|i| !changes.despawn.contains(i));
    if let Some(mut focus) = world.get_resource_mut::<Focus>() {
        let kept = gone(focus.0);
        if kept != focus.0 {
            focus.0 = kept;
        }
    }
    if let Some(mut hovered) = world.get_resource_mut::<Hovered>() {
        let kept = gone(hovered.0);
        if kept != hovered.0 {
            hovered.0 = kept;
        }
    }
    if let Some(mut status) = world.get_resource_mut::<LoadStatus>()
        && let LoadStatus::Loaded { nodes: shown, .. } = &mut *status
        && *shown != nodes
    {
        *shown = nodes;
    }
    let took = started.elapsed().as_secs_f64() * 1e3;
    let summary = format!(
        "live: applied {count} events in {took:.2} ms: {} discs spawned, {} gone, {} moved",
        changes.spawn.len(),
        changes.despawn.len(),
        changes.reshape.len()
    );
    // A burst, a decomposition's worth, is worth a line; a trickle is not.
    if count >= BURST {
        info!("{summary}");
    } else {
        debug!("{summary}");
    }
}

/// How the stream stands, in a few words for the bar, and whether that is
/// a problem.
pub fn status_text(state: &LiveState, followed_connected: bool) -> (String, bool) {
    match &state.connection {
        Connection::Off => ("off".into(), false),
        Connection::Connecting => ("connecting".into(), false),
        Connection::Lost { error, .. } => (format!("reconnecting: {error}"), true),
        Connection::Live { .. } if state.refetching => {
            ("connected, reading the graph again".into(), false)
        }
        Connection::Live { .. } if !followed_connected => {
            ("connected; the session is disconnected".into(), true)
        }
        Connection::Live { .. } => (
            format!(
                "connected: {} messages, {} events applied",
                state.received, state.applied
            ),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};

    use super::*;
    use crate::apply::tests::{linked, stored};
    use crate::filter::{Filter, FilterPlugin};
    use crate::graph_index::tests::{edge, node};
    use crate::hub::{EdgeView, GraphList, NodeType, NodeView, Snapshot};
    use crate::reload::{Arrival, Purpose, catalogue, prepare};
    use crate::scene::{Disc, DiscMaterial};
    use crate::stream::Command;

    const SESSION: &str = "followed";

    fn session(id: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            pid: 7,
            backend: "surrealdb".into(),
            active_graph: "test".into(),
            started_at: "2026-10-20T10:00:00Z".into(),
            connected: true,
            last_event_at: None,
            schema_version: Some(1),
        }
    }

    /// An app drawing `snapshot`, with a stream fed by the sender returned.
    fn app(snapshot: &Snapshot) -> (App, Sender<Item>, mpsc::Receiver<Command>) {
        let prepared = prepare(snapshot).unwrap();
        let arrival = Arrival {
            prepared: prepared.clone(),
            session: session(SESSION),
            sessions: vec![session(SESSION)],
            graphs: GraphList {
                graphs: vec!["test".into()],
                active_graph: "test".into(),
                backend: "surrealdb".into(),
            },
            purpose: Purpose::Switch,
            map: None,
        };
        let (items, item_receiver) = mpsc::channel();
        let (commands, command_receiver) = mpsc::channel();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<DiscMaterial>()
            .add_message::<crate::camera::ViewRequest>()
            .insert_resource(catalogue("127.0.0.1:1", "test", &arrival))
            .insert_resource(Filter::default())
            .insert_resource(prepared.index.clone())
            .insert_resource(LiveState {
                subscribed_before_load: Some(1),
                ..LiveState::default()
            })
            .insert_resource(stream::from_channels(item_receiver, commands))
            // No reload plugin: a read asked for is seen as the message,
            // and no load starts, so nothing waits on a hub.
            .init_resource::<Loading>()
            .init_resource::<LoadStatus>()
            .add_plugins((FilterPlugin, LivePlugin { log_events: false }));
        reload::apply(app.world_mut(), prepared);
        app.insert_resource(LoadStatus::Loaded {
            graph: "test".into(),
            nodes: 0,
        });
        app.update();
        (app, items, command_receiver)
    }

    fn small() -> Snapshot {
        Snapshot {
            schema_version: Some(1),
            graph: "test".into(),
            nodes: vec![node("a", NodeType::Topic, 1), node("fa", NodeType::Fact, 2)],
            edges: vec![edge("fa", "a", "extracted_under_topic")],
            timelines: Vec::new(),
            metacontexts: Vec::new(),
        }
    }

    fn message(seq: u64, event: Event, event_type: &str) -> Item {
        Item::Message(Message::Event {
            seq: Some(seq),
            envelope: Box::new(Envelope {
                event_type: event_type.into(),
                session_id: SESSION.into(),
                graph: "test".into(),
                event,
            }),
        })
    }

    fn refetches(app: &mut App) -> Vec<String> {
        app.world_mut()
            .resource_mut::<Messages<Refetch>>()
            .drain()
            .map(|r| r.reason)
            .collect()
    }

    fn discs(app: &mut App) -> usize {
        let mut query = app.world_mut().query::<&Disc>();
        query.iter(app.world()).count()
    }

    fn fact(id: &str, day: u32) -> NodeView {
        node(id, NodeType::Fact, day)
    }

    #[test]
    fn events_are_applied_in_the_update_they_arrive_and_a_gap_asks_for_the_graph_again() {
        let (mut app, items, _commands) = app(&small());
        items.send(Item::Connected { connection: 1 }).unwrap();
        items
            .send(message(1, stored(fact("new", 3)), "node_stored"))
            .unwrap();
        items
            .send(message(
                2,
                linked(edge("new", "a", "extracted_under_topic")),
                "edge_stored",
            ))
            .unwrap();
        app.update();
        assert_eq!(discs(&mut app), 3, "drawn in the update it came");
        assert!(
            refetches(&mut app).is_empty(),
            "connection 1 was subscribed before the load"
        );
        let state = app.world().resource::<LiveState>();
        assert_eq!(state.connection, Connection::Live { connection: 1 });
        assert_eq!((state.received, state.applied), (2, 2));
        assert_eq!(
            *app.world().resource::<LoadStatus>(),
            LoadStatus::Loaded {
                graph: "test".into(),
                nodes: 3
            }
        );

        // seq 3 never comes.
        items
            .send(message(4, stored(fact("after-gap", 4)), "node_stored"))
            .unwrap();
        app.update();
        let reasons = refetches(&mut app);
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("seq 4 after 2"), "{reasons:?}");
        assert_eq!(
            app.world().resource::<LiveState>().held.len(),
            1,
            "held until the graph is read again"
        );
        assert_eq!(discs(&mut app), 3);
    }

    #[test]
    fn a_reconnect_and_a_session_back_from_a_disconnection_ask_for_the_graph_again() {
        let (mut app, items, _commands) = app(&small());
        items.send(Item::Connected { connection: 1 }).unwrap();
        app.update();
        assert!(refetches(&mut app).is_empty());
        items
            .send(Item::Disconnected {
                error: "Connection refused".into(),
                retry_in: Duration::from_millis(500),
            })
            .unwrap();
        app.update();
        let state = app.world().resource::<LiveState>();
        assert_eq!(
            status_text(state, true),
            ("reconnecting: Connection refused".into(), true)
        );
        items.send(Item::Connected { connection: 2 }).unwrap();
        app.update();
        assert_eq!(refetches(&mut app).len(), 1, "the client's own reconnect");
        app.world_mut().resource_mut::<LiveState>().refetching = false;

        // The session disconnects and connects again.
        let system = |seq, system| {
            Item::Message(Message::System {
                seq: Some(seq),
                system,
            })
        };
        items
            .send(system(1, System::SessionDisconnected(SESSION.into())))
            .unwrap();
        app.update();
        assert!(!app.world().resource::<Catalogue>().session.connected);
        assert!(refetches(&mut app).is_empty());
        items
            .send(system(2, System::SessionConnected(session(SESSION))))
            .unwrap();
        app.update();
        assert_eq!(refetches(&mut app).len(), 1);
        assert!(app.world().resource::<Catalogue>().session.connected);

        // A change of active graph, with no disconnection, reads nothing
        // again, and only the picker changes.
        app.world_mut().resource_mut::<LiveState>().refetching = false;
        let mut moved = session(SESSION);
        moved.active_graph = "elsewhere".into();
        items
            .send(system(3, System::SessionConnected(moved)))
            .unwrap();
        items
            .send(system(4, System::SessionDropped("other".into())))
            .unwrap();
        app.update();
        assert!(refetches(&mut app).is_empty());
        assert_eq!(
            app.world().resource::<Catalogue>().session.active_graph,
            "elsewhere"
        );
    }

    #[test]
    fn events_of_another_session_or_graph_and_events_not_acted_on_are_dropped() {
        let (mut app, items, _commands) = app(&small());
        items.send(Item::Connected { connection: 1 }).unwrap();
        let mut other_session = message(1, stored(fact("x", 3)), "node_stored");
        if let Item::Message(Message::Event { envelope, .. }) = &mut other_session {
            envelope.session_id = "someone-else".into();
        }
        let mut other_graph = message(2, stored(fact("y", 3)), "node_stored");
        if let Item::Message(Message::Event { envelope, .. }) = &mut other_graph {
            envelope.graph = "games".into();
        }
        items.send(other_session).unwrap();
        items.send(other_graph).unwrap();
        items
            .send(message(3, Event::Other, "transition_fired"))
            .unwrap();
        items
            .send(message(
                4,
                Event::GraphSwitched {
                    previous_graph: "test".into(),
                    new_graph: "games".into(),
                },
                "graph_switched",
            ))
            .unwrap();
        app.update();
        assert_eq!(discs(&mut app), 2);
        let state = app.world().resource::<LiveState>();
        assert_eq!((state.received, state.applied), (4, 0));
        assert_eq!(
            app.world().resource::<Catalogue>().sessions[0].active_graph,
            "games",
            "the picker follows the switch"
        );
        assert!(refetches(&mut app).is_empty());
    }

    /// A graph the size of the development graph's: 420 topics and 1,600
    /// claims, each filed and tagged, in one metacontext, read from a
    /// handful of documents, with supports edges between claims.
    fn large() -> Snapshot {
        let mut nodes = Vec::new();
        let mut edges: Vec<EdgeView> = Vec::new();
        for t in 0..420 {
            nodes.push(node(&format!("t{t}"), NodeType::Topic, 1 + t % 28));
        }
        for c in 0..1600 {
            let id = format!("c{c}");
            let kind = if c % 4 == 0 {
                NodeType::Inference
            } else {
                NodeType::Fact
            };
            nodes.push(node(&id, kind, 1 + c % 28));
            let topic = format!("t{}", c % 420);
            let filing = if kind == NodeType::Fact {
                "extracted_under_topic"
            } else {
                "abstracts"
            };
            edges.push(edge(&id, &topic, filing));
            edges.push(edge(
                &id,
                &format!("t{}", (c * 7) % 420),
                "tagged_with_topic",
            ));
            edges.push(edge(&id, "the-real", "has_metacontext"));
            edges.push(edge(&id, &format!("doc{}", c % 40), "sourced_from"));
            if c % 4 == 0 && c > 0 {
                edges.push(edge(&format!("c{}", c - 1), &id, "supports"));
            }
        }
        Snapshot {
            schema_version: Some(1),
            graph: "test".into(),
            nodes,
            edges,
            timelines: Vec::new(),
            metacontexts: Vec::new(),
        }
    }

    #[test]
    fn a_burst_of_a_decomposition_is_applied_in_one_update() {
        let (mut app, items, _commands) = app(&large());
        items.send(Item::Connected { connection: 1 }).unwrap();
        app.update();
        let before = discs(&mut app);
        assert_eq!(before, 2020);

        // One store_decomposition: a new topic, 40 claims and their edges,
        // nodes first and then edges, as the session sends them: 41 nodes
        // and 400 edges.
        let mut burst: Vec<(Event, &str)> = vec![(
            stored(node("new-topic", NodeType::Topic, 28)),
            "node_stored",
        )];
        for c in 0..40 {
            let kind = if c % 4 == 0 {
                NodeType::Inference
            } else {
                NodeType::Fact
            };
            burst.push((stored(node(&format!("n{c}"), kind, 28)), "node_stored"));
        }
        let mut edge_count = 0;
        for c in 0..40 {
            let id = format!("n{c}");
            let filing = if c % 4 == 0 {
                "abstracts"
            } else {
                "extracted_under_topic"
            };
            let mut list = vec![
                edge(&id, "new-topic", filing),
                edge(&id, "new-topic", "tagged_with_topic"),
                edge(&id, &format!("t{}", c * 3), "tagged_with_topic"),
                edge(&id, &format!("t{}", c * 5 + 1), "tagged_with_topic"),
                edge(&id, "the-real", "has_metacontext"),
                edge(&id, "doc-new", "sourced_from"),
                edge(
                    &format!("seg{c}"),
                    &id,
                    if c % 4 == 0 { "implies" } else { "contains" },
                ),
                edge(&id, &format!("n{}", (c + 1) % 40), "supports"),
                edge(&id, &format!("c{}", c * 11), "supports"),
                edge(&id, &format!("c{}", c * 13 + 1), "related"),
            ];
            edge_count += list.len();
            burst.extend(list.drain(..).map(|e| (linked(e), "edge_stored")));
        }
        assert_eq!(edge_count, 400);
        for (seq, (event, kind)) in burst.into_iter().enumerate() {
            items.send(message(seq as u64 + 1, event, kind)).unwrap();
        }
        let started = Instant::now();
        app.update();
        let took = started.elapsed();
        eprintln!(
            "a burst of 441 events applied in one update of {:.1} ms",
            took.as_secs_f64() * 1e3
        );
        assert_eq!(
            discs(&mut app),
            before + 41,
            "every new node drawn in that update"
        );
        let state = app.world().resource::<LiveState>();
        assert_eq!(state.applied, 441);
        assert!(state.held.is_empty() && state.wanted.is_none());
        let index = app.world().resource::<GraphIndex>();
        let topic = index.by_id["new-topic"];
        assert!((0..40).all(|c| index.filing[index.by_id[&format!("n{c}")]] == Some(topic)));
        assert!(
            took < Duration::from_millis(250),
            "{took:?} is more than a few frames"
        );
    }
}
