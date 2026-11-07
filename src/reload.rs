//! Loading a graph: the snapshot read from the hub, placed, indexed, and
//! swapped into the running scene.
//!
//! The first graph is read before the window opens, in `main.rs`, through
//! [`prepare_seeded`], on its saved topic map. Every later one is read in
//! the background, so the window keeps drawing while the hub answers: a
//! [`LoadGraph`] message, sent by the bar's graph picker, starts
//! [`load_and_prepare`] with
//! [`in_background`](crate::background::in_background), on a thread of its
//! own natively and as a task on the page's event loop in a browser, and the
//! answer comes back over a channel that [`receive_loads`] polls once per
//! update. A newer request replaces an older one still waiting; the older
//! answer is dropped when it comes.
//!
//! [`apply`] swaps the new graph in, in the time mode drawn: the old discs
//! go, the new index, places, time model and picker lists replace the
//! old, a switch of time mode under way stops, the filter forgets its topic,
//! time window, source document and retrieval record (which name things in
//! the old graph) and keeps its kinds, metacontexts and mode, the focus and the hover are
//! cleared, since an index names a node of one graph, and the camera fits
//! the new graph from the 3D preset. A load that fails leaves the old graph drawn
//! and says why in [`LoadStatus`], which the bar shows.
//!
//! A switch to another session also moves the event stream to it
//! (`stream.rs`), before the snapshot is read, so that no event between the
//! two is missed; events that arrive meanwhile are held until the snapshot
//! is drawn (`live.rs`).
//!
//! A [`Refetch`], which `live.rs` sends after a gap in the event stream, a
//! reconnect, or a session that came back, reads the graph drawn again on
//! the same path, over the picture drawn ([`RefetchSeed`]): every node
//! keeps its place and the axes their scale, and the focus, the hover, the
//! filter, the time mode and the camera are kept wherever the ids they name
//! are still in the graph.
//!
//! Every graph is placed on its saved topic map where there is one
//! (`relayout.rs`): a switch reads the map of the graph it switches to in
//! the same background work, and a read again places a topic new to the
//! picture where the map drawn puts it.

use std::sync::mpsc;
use std::time::Duration;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use web_time::{Instant, SystemTime, UNIX_EPOCH};

use crate::apply::{self, LiveGraph, RefetchSeed};
use crate::background::{self, Pending, Poll, in_background, poll};
use crate::camera::{self, Places};
use crate::edge_index;
use crate::filter::{self, Filter};
use crate::focus::{Focus, Hovered};
use crate::graph_index::{self, GraphIndex};
use crate::hub::{self, GraphList, Loaded, SessionInfo, Snapshot};
use crate::layout::{self, Kind, Placed};
use crate::live::LiveState;
use crate::persist::{self, MapKey};
use crate::pickers::{self, Pickers, short_id};
use crate::presets;
use crate::relayout::{self, LayoutTween, MapState};
use crate::scene;
use crate::stream::{self, Stream, Subscription};
use crate::time_axis::{self, TimeMode, TimeModel};
use crate::time_mode::{self, ModeTween};
use crate::topic_map::{self, TopicMap};

/// How long a switch to another session waits for the event stream to
/// follow it before reading the snapshot.
const SUBSCRIBE_WAIT: Duration = Duration::from_secs(1);

/// A snapshot as the scene draws it: placed, indexed, and listed for the
/// pickers.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub graph: String,
    pub nodes: Vec<Placed>,
    /// Indexed and placed in recorded time; [`apply`] and `main.rs` apply
    /// the time mode drawn.
    pub index: GraphIndex,
    pub places: Places,
    pub pickers: Pickers,
    pub time: TimeModel,
    /// What applying the event stream needs (`apply.rs`).
    pub live: LiveGraph,
}

/// [`prepare_seeded`] with no seed and no map: the circle.
#[cfg(test)]
pub fn prepare(snapshot: &Snapshot) -> Result<Prepared, String> {
    prepare_seeded(snapshot, None, None)
}

/// Places a snapshot's nodes, over the picture `seed` describes where there
/// is one, so that the nodes already drawn keep their places and the axes
/// their scale, and on the topic map `map` where there is one; and builds
/// everything the scene, the filter, the camera and the bar read from them.
/// A graph with no node to draw is an error, since there would be nothing
/// to look at.
pub fn prepare_seeded(
    snapshot: &Snapshot,
    seed: Option<&RefetchSeed>,
    map: Option<&TopicMap>,
) -> Result<Prepared, String> {
    let started = Instant::now();
    let layout = layout::place_seeded(
        &snapshot.nodes,
        &snapshot.edges,
        seed.map(|s| &s.layout),
        map.map(topic_map::places),
    );
    let count = |kind: Kind| layout.nodes.iter().filter(|n| n.kind == kind).count();
    info!(
        "placed {} topics, {} facts and {} inferences in {:.1} ms; claims by filing edge {}, by tag {}, unfiled {}; {} nodes left out",
        count(Kind::Topic),
        count(Kind::Fact),
        count(Kind::Inference),
        started.elapsed().as_secs_f64() * 1e3,
        layout.filed,
        layout.tagged,
        layout.unfiled,
        layout.left_out,
    );
    if layout.nodes.is_empty() {
        return Err(format!("graph {:?} has no node to draw", snapshot.graph));
    }
    let index = graph_index::build(&snapshot.nodes, &snapshot.edges, &layout);
    let edges = edge_index::counts(&index.edges);
    info!(
        "edges drawn as lines: {} claim-to-claim between drawn nodes and {} with an end not drawn; {} successions and merges between drawn nodes, {} whose old end is not drawn and {} whose new end is not; {} of either with neither end drawn",
        edges.claim_drawn,
        edges.claim_absent,
        edges.succession_drawn,
        edges.succession_old_absent,
        edges.succession_new_absent,
        edges.undrawn,
    );
    let places = Places {
        points: layout.nodes.iter().map(|n| n.position).collect(),
        topics: presets::topic_regions(&layout.nodes, &index_names(&index)),
        metacontexts: presets::metacontext_regions(
            &layout.nodes,
            &snapshot.edges,
            &snapshot.metacontexts,
        ),
        spans: Vec::new(),
        now: presets::NowLayer::Latest(presets::day_height(&layout.time)),
    };
    let listed: Vec<String> = places
        .metacontexts
        .iter()
        .map(|m| format!("{:?} ({} nodes)", m.name, m.members.len()))
        .collect();
    info!(
        "{} metacontexts with a drawn node: {}; one day is {:.2} high on the recorded time axis",
        listed.len(),
        listed.join(", "),
        presets::day_height(&layout.time)
    );
    let pickers = pickers::build(&index, &snapshot.metacontexts);
    let time = match seed {
        None => time_axis::build(snapshot, &layout, wall_clock()),
        Some(seed) => {
            time_axis::build_seeded(snapshot, &layout, wall_clock(), Some(seed.claim_axis))
        }
    };
    log_time(&time);
    let live = apply::live_graph(snapshot, &layout, &index);
    Ok(Prepared {
        graph: snapshot.graph.clone(),
        nodes: layout.nodes,
        index,
        places,
        pickers,
        time,
        live,
    })
}

/// The wall clock, in microseconds since the Unix epoch: "now" where no
/// timeline states one.
pub fn wall_clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros().min(i64::MAX as u128) as i64)
}

/// Logs what claim time makes of the graph.
fn log_time(time: &TimeModel) {
    let c = &time.counts;
    info!(
        "timepoints: {} interval, {} of them drawn as a column; {} vague, {} with a bound and {} of them drawn as a smear; {} instant, {} drawn; {} of another kind",
        c.interval_points,
        c.interval_points_drawn,
        c.vague_points,
        c.vague_points_with_bounds,
        c.vague_points_drawn,
        c.instant_points,
        c.instant_points_drawn,
        c.other_points,
    );
    info!(
        "claim time draws {} solid columns, {} smears, {} instants and {} discs at their recorded time, and lists {} claims with no place in the tray; {} claims are linked to more than one timepoint; now is {}",
        c.columns,
        c.smears,
        c.instants,
        c.discs,
        c.unplaced,
        c.several,
        match &time.now_source {
            time_axis::NowSource::Reference(name) =>
                format!("the reference time of timeline {name:?}"),
            time_axis::NowSource::WallClock => "the wall clock".to_owned(),
        }
    );
}

/// Each drawn node's content by its id.
fn index_names(index: &GraphIndex) -> std::collections::HashMap<&str, &str> {
    index
        .ids
        .iter()
        .zip(&index.names)
        .map(|(id, name)| (id.as_str(), name.as_str()))
        .collect()
}

/// Logs what a load read: the graph, its size, and how long it took.
pub fn log_loaded(loaded: &Loaded) {
    info!(
        "snapshot of {:?}: {} nodes, {} edges, {:.1} MB, fetched in {:.0} ms and parsed in {:.0} ms",
        loaded.snapshot.graph,
        loaded.snapshot.nodes.len(),
        loaded.snapshot.edges.len(),
        loaded.snapshot_bytes as f64 / 1e6,
        loaded.download.as_secs_f64() * 1e3,
        loaded.parse.as_secs_f64() * 1e3,
    );
}

/// What a load in the background sends back: the prepared graph, and what
/// the hub listed on the way.
#[derive(Debug, Clone)]
pub struct Arrival {
    pub prepared: Prepared,
    pub session: SessionInfo,
    pub sessions: Vec<SessionInfo>,
    pub graphs: GraphList,
    pub purpose: Purpose,
    /// The topic map the graph was placed on.
    pub map: Option<TopicMap>,
}

/// Why a graph is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Purpose {
    /// Another graph, or another session's, in place of the one drawn.
    #[default]
    Switch,
    /// The graph drawn, again, over the picture drawn.
    Refetch,
}

/// Where a load finds its topic map: the map drawn, for a read again, or
/// the map saved for the graph, for a switch.
pub enum MapSource {
    Drawn(Option<TopicMap>),
    Saved(persist::Store, MapKey),
}

/// The map saved under `key`, or none, saying why where it cannot be read.
pub fn saved_map(store: &persist::Store, key: &MapKey) -> Option<TopicMap> {
    if matches!(store, persist::Store::Off) {
        info!(
            "topic map of {:?}: none read, since --no-persist was given",
            key.graph
        );
        return None;
    }
    match persist::read_map(store, key) {
        Ok(Some(map)) => {
            info!(
                "topic map of {:?}: {} topics from {}, read from {}",
                key.graph,
                map.topics.len(),
                map.model(),
                persist::describe(store, key)
            );
            Some(map)
        }
        Ok(None) => {
            info!(
                "topic map of {:?}: none saved at {}",
                key.graph,
                persist::describe(store, key)
            );
            None
        }
        Err(message) => {
            warn!(
                "topic map of {:?}: {message}; a new one is fitted",
                key.graph
            );
            None
        }
    }
}

/// Reads a graph from the hub and prepares it, over `seed` where it is
/// read again and on its topic map, as one piece of background work. The
/// snapshot is dropped here once prepared, so only what the scene draws
/// crosses back.
pub async fn load_and_prepare(
    request: &hub::Request,
    seed: Option<&RefetchSeed>,
    purpose: Purpose,
    map: MapSource,
) -> Result<Arrival, String> {
    let loaded = hub::load(request).await.map_err(|e| e.to_string())?;
    log_loaded(&loaded);
    let map = match map {
        MapSource::Drawn(map) => map,
        MapSource::Saved(store, key) => saved_map(&store, &key),
    };
    let prepared = prepare_seeded(&loaded.snapshot, seed, map.as_ref())?;
    Ok(Arrival {
        prepared,
        session: loaded.session,
        sessions: loaded.sessions,
        graphs: loaded.graphs,
        purpose,
        map,
    })
}

/// What the hub offers and what is drawn from it: the sessions it lists,
/// the session and graph drawn, and the session whose graphs the graph
/// picker lists.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct Catalogue {
    /// The hub, as `host:port`.
    pub hub: String,
    pub sessions: Vec<SessionInfo>,
    /// The session the drawn graph was read from.
    pub session: SessionInfo,
    /// The graph drawn.
    pub graph: String,
    /// The session the graph picker lists the graphs of: the drawn one
    /// until another is chosen.
    pub browsing: String,
    /// That session's graphs, once read.
    pub graphs: Option<GraphList>,
}

/// The catalogue after a load from this hub.
pub fn catalogue(hub: &str, graph: &str, loaded: &Arrival) -> Catalogue {
    Catalogue {
        hub: hub.to_owned(),
        sessions: loaded.sessions.clone(),
        session: loaded.session.clone(),
        graph: graph.to_owned(),
        browsing: loaded.session.session_id.clone(),
        graphs: Some(loaded.graphs.clone()),
    }
}

/// Where loading stands, for the bar.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub enum LoadStatus {
    #[default]
    Idle,
    Loading {
        graph: String,
        session: String,
    },
    /// The graph drawn, and how many nodes it has.
    Loaded {
        graph: String,
        nodes: usize,
    },
    Failed(String),
}

/// Asks for a graph to be read from a session and drawn.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct LoadGraph {
    pub session: String,
    pub graph: String,
}

/// Asks for the graph drawn to be read again, for this reason.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct Refetch {
    pub reason: String,
}

/// The load under way, if any.
#[derive(Resource, Default)]
pub struct Loading(pub Option<Pending<Result<Arrival, String>>>);

/// Starts loads and swaps in what they bring.
pub struct ReloadPlugin;

impl Plugin for ReloadPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LoadGraph>()
            .add_message::<Refetch>()
            .init_resource::<Loading>()
            .init_resource::<LoadStatus>()
            .add_systems(PreUpdate, receive_loads)
            .add_systems(Update, start_loads);
    }
}

/// What a refetch reads the picture drawn from, and the event stream a
/// switch moves.
#[derive(bevy::ecs::system::SystemParam)]
struct Drawn<'w> {
    index: Res<'w, GraphIndex>,
    places: Res<'w, Places>,
    time: Res<'w, TimeModel>,
    live: Option<Res<'w, LiveGraph>>,
    stream: Option<Res<'w, Stream>>,
    live_state: Option<ResMut<'w, LiveState>>,
    map: Option<Res<'w, MapState>>,
    tween: Option<Res<'w, LayoutTween>>,
}

/// Starts background work for the latest [`LoadGraph`], replacing any load
/// still waiting, and moves the event stream to its session first; or, with no
/// switch asked for and no load under way, for a [`Refetch`] of the graph
/// drawn, over the picture drawn.
fn start_loads(
    mut requests: MessageReader<LoadGraph>,
    mut refetches: MessageReader<Refetch>,
    catalogue: Res<Catalogue>,
    mut drawn: Drawn,
    mut loading: ResMut<Loading>,
    mut status: ResMut<LoadStatus>,
) {
    let switch = requests.read().last().cloned();
    let refetch = refetches.read().last().cloned();
    let (request, seed, purpose, acked, map) = match (switch, refetch) {
        (Some(request), _) => {
            let acked = drawn.stream.as_deref().map(|stream| {
                let (ack, acked) = mpsc::channel();
                let subscription = Subscription {
                    session: request.session.clone(),
                    graphs: vec![request.graph.clone()],
                };
                stream::resubscribe(stream, subscription, Some(ack));
                acked
            });
            if let Some(state) = drawn.live_state.as_deref_mut() {
                state.following = Some((request.session.clone(), request.graph.clone()));
            }
            let store = drawn
                .map
                .as_deref()
                .map_or(persist::Store::Off, |m| m.store.clone());
            let key = MapKey {
                hub: catalogue.hub.clone(),
                graph: request.graph.clone(),
            };
            (
                request,
                None,
                Purpose::Switch,
                acked,
                MapSource::Saved(store, key),
            )
        }
        (None, Some(refetch)) if loading.0.is_none() => {
            let Some(live) = drawn.live.as_deref() else {
                return;
            };
            info!(
                "reading graph {:?} again: {}",
                catalogue.graph, refetch.reason
            );
            // A move on the map under way is seeded where it ends.
            let places = relayout::settled_places(&drawn.places, drawn.tween.as_deref());
            let seed = apply::seed(&drawn.index, &places, &drawn.time, live);
            let request = LoadGraph {
                session: catalogue.session.session_id.clone(),
                graph: catalogue.graph.clone(),
            };
            let map = drawn.map.as_deref().and_then(|m| m.map.clone());
            (
                request,
                Some(seed),
                Purpose::Refetch,
                None,
                MapSource::Drawn(map),
            )
        }
        _ => return,
    };
    let hub_request = hub::Request {
        hub: catalogue.hub.clone(),
        session: Some(request.session.clone()),
        graph: request.graph.clone(),
    };
    info!(
        "loading graph {:?} from session {}",
        request.graph, request.session
    );
    let work = move || async move {
        if let Some(acked) = acked {
            let _ = background::recv_timeout(&acked, SUBSCRIBE_WAIT).await;
        }
        load_and_prepare(&hub_request, seed.as_ref(), purpose, map).await
    };
    match in_background("load a snapshot", work) {
        Ok(pending) => {
            loading.0 = Some(pending);
            *status = LoadStatus::Loading {
                graph: request.graph,
                session: request.session,
            };
        }
        Err(message) => *status = LoadStatus::Failed(message),
    }
}

/// Takes the answer of the load under way when it has come, and applies it
/// or reports why it failed.
pub fn receive_loads(world: &mut World) {
    let answer = match world.resource::<Loading>().0.as_ref().map(poll) {
        None | Some(Poll::Waiting) => return,
        Some(Poll::Ready(answer)) => answer,
        Some(Poll::Lost) => Err("the load stopped without an answer".to_owned()),
    };
    world.resource_mut::<Loading>().0 = None;
    match answer {
        Ok(arrival) => {
            let nodes = arrival.prepared.nodes.len();
            let graph = arrival.prepared.graph.clone();
            if let Some(mut catalogue) = world.get_resource_mut::<Catalogue>() {
                let hub = catalogue.hub.clone();
                *catalogue = self::catalogue(&hub, &graph, &arrival);
            }
            info!(
                "drawing graph {graph:?} from session {}",
                short_id(&arrival.session.session_id)
            );
            let hub = world
                .get_resource::<Catalogue>()
                .map(|c| c.hub.clone())
                .unwrap_or_default();
            match arrival.purpose {
                Purpose::Switch => {
                    apply(world, arrival.prepared);
                    relayout::graph_switched(world, &hub, &graph, arrival.map);
                }
                Purpose::Refetch => apply_again(world, arrival.prepared),
            }
            *world.resource_mut::<LoadStatus>() = LoadStatus::Loaded { graph, nodes };
        }
        Err(message) => {
            error!("{message}");
            *world.resource_mut::<LoadStatus>() = LoadStatus::Failed(message);
        }
    }
}

/// The filter for a new graph: the criteria that name things in the old
/// one (a topic, a stretch of its time axis, a document, a retrieval
/// record) are dropped, and the kinds, the metacontexts and the mode are
/// kept.
pub fn filter_for_new_graph(filter: &Filter) -> Filter {
    Filter {
        topic: None,
        window: None,
        source: None,
        retrieval: None,
        ..filter.clone()
    }
}

/// The filter for the graph drawn, read again: every criterion is kept,
/// the retrieval record among them, except a topic that is no longer a
/// drawn topic and a document no drawn node is read from any more.
pub fn filter_for_refetch(filter: &Filter, index: &GraphIndex) -> Filter {
    let topic = filter.topic.clone().filter(|id| {
        index
            .by_id
            .get(id)
            .is_some_and(|&i| index.kinds[i] == Kind::Topic)
    });
    let source = filter
        .source
        .clone()
        .filter(|id| index.sources.iter().any(|s| s.contains(id)));
    Filter {
        topic,
        source,
        ..filter.clone()
    }
}

/// Swaps the graph drawn, read again, into the world over the picture it
/// was seeded from: the time mode, the filter, the focus, the hover and the
/// camera stay wherever the ids they name are still drawn.
pub fn apply_again(world: &mut World, prepared: Prepared) {
    let ids = |node: Option<usize>, world: &World| {
        let index = world.get_resource::<GraphIndex>()?;
        node.and_then(|i| index.ids.get(i).cloned())
    };
    let focused = ids(world.get_resource::<Focus>().and_then(|f| f.0), world);
    let hovered = ids(world.get_resource::<Hovered>().and_then(|h| h.0), world);
    relayout::finish_tween(world);
    let Prepared {
        nodes,
        mut index,
        mut places,
        pickers,
        time,
        live,
        ..
    } = prepared;
    scene::despawn_discs(world);
    let mode = world
        .get_resource::<TimeMode>()
        .copied()
        .unwrap_or_default();
    time_mode::apply_mode(mode, &time, &mut index, &mut places);
    if let (Some(old), Some(old_places)) = (
        world.get_resource::<GraphIndex>(),
        world.get_resource::<Places>(),
    ) {
        let (kept, moved) = old
            .ids
            .iter()
            .zip(&old_places.points)
            .filter_map(|(id, &before)| {
                let i = *index.by_id.get(id)?;
                Some(places.points.get(i)? == &before)
            })
            .fold((0, 0), |(kept, moved), same| {
                if same {
                    (kept + 1, moved)
                } else {
                    (kept, moved + 1)
                }
            });
        info!(
            "read again: {} nodes, {kept} of them drawn before and where they were, {moved} moved",
            index.ids.len()
        );
    }
    let filter = filter_for_refetch(
        &world.get_resource::<Filter>().cloned().unwrap_or_default(),
        &index,
    );
    let shown = filter::shown(&filter, &index);
    let find = |id: Option<String>| {
        id.and_then(|id| index.by_id.get(&id).copied())
            .filter(|&i| index.active[i])
    };
    let (focus, hover) = (find(focused), find(hovered));
    world.insert_resource(Focus(focus));
    world.insert_resource(Hovered(hover));
    let spans = time_axis::times(&time, mode).spans.clone();
    world.insert_resource(time);
    world.insert_resource(ModeTween::default());
    world.insert_resource(filter);
    world.insert_resource(index);
    world.insert_resource(places);
    world.insert_resource(pickers);
    world.insert_resource(live);
    world.insert_resource(shown.clone());
    scene::spawn_discs(world, nodes, &spans, &shown);
}

/// Swaps a prepared graph into the world in place of the one drawn.
pub fn apply(world: &mut World, prepared: Prepared) {
    let Prepared {
        graph,
        nodes,
        mut index,
        mut places,
        pickers,
        time,
        live,
    } = prepared;
    scene::despawn_discs(world);
    world.insert_resource(LayoutTween::default());
    let mode = world
        .get_resource::<TimeMode>()
        .copied()
        .unwrap_or_default();
    time_mode::apply_mode(mode, &time, &mut index, &mut places);
    world.insert_resource(time);
    world.insert_resource(ModeTween::default());
    let filter = filter_for_new_graph(&world.get_resource::<Filter>().cloned().unwrap_or_default());
    let shown = filter::shown(&filter, &index);
    filter::log_shown(&filter, &index, &shown);
    world.insert_resource(filter);
    world.insert_resource(index);
    world.insert_resource(places);
    world.insert_resource(pickers);
    world.insert_resource(live);
    world.insert_resource(shown.clone());
    world.insert_resource(Focus(None));
    world.insert_resource(Hovered(None));
    let spans = scene::current_spans(world);
    scene::spawn_discs(world, nodes, &spans, &shown);
    camera::refit_new_graph(world);
    let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
    for mut window in windows.iter_mut(world) {
        window.title = format!("epimemer-bevy: {graph}");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::camera::ViewRequest;
    use crate::filter::{Kinds, Metacontexts, Mode, RetrievalFocus, Shown, TimeWindow};
    use crate::graph_index::tests::node;
    use crate::hub::{NodeType, parse_snapshot};
    use crate::presets::Preset;
    use crate::scene::{Disc, DiscMaterial};

    fn fixture() -> Snapshot {
        parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    fn session(id: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            pid: 7,
            backend: "surrealdb".into(),
            active_graph: "fixture".into(),
            started_at: "2026-10-20T10:00:00Z".into(),
            connected: true,
            last_event_at: None,
            schema_version: Some(1),
        }
    }

    fn arrival(prepared: Prepared) -> Arrival {
        Arrival {
            prepared,
            session: session("new-session"),
            sessions: vec![session("old-session"), session("new-session")],
            graphs: GraphList {
                graphs: vec!["fixture".into(), "other".into()],
                active_graph: "fixture".into(),
                backend: "surrealdb".into(),
                reflect: crate::hub::Reflect {
                    count: 3,
                    threshold: 10,
                    suggested: false,
                },
            },
            purpose: Purpose::Switch,
            map: None,
        }
    }

    /// An app drawing a small graph of four topics and a fact, with a
    /// filter that sets every criterion.
    fn app() -> App {
        let nodes: Vec<_> = (1..=4)
            .map(|day| node(&format!("old-topic-{day}"), NodeType::Topic, day))
            .chain([node("old-fact", NodeType::Fact, 5)])
            .collect();
        let snapshot = Snapshot {
            schema_version: Some(1),
            graph: "old".into(),
            nodes,
            edges: Vec::new(),
            timelines: Vec::new(),
            metacontexts: Vec::new(),
        };
        let old = prepare(&snapshot).unwrap();
        let filter = Filter {
            kinds: Kinds {
                topic: false,
                ..Kinds::default()
            },
            metacontexts: Metacontexts::Only(["the-real".to_owned()].into()),
            topic: Some("old-topic-1".into()),
            window: Some(TimeWindow {
                from: 10.0,
                to: 20.0,
            }),
            source: Some("doc".into()),
            retrieval: Some(RetrievalFocus {
                record_id: "000000000003".into(),
                retrieved: Some(["old-fact".to_owned()].into()),
            }),
            mode: Mode::Hide,
            ..Filter::default()
        };
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<DiscMaterial>()
            .add_message::<ViewRequest>()
            .add_plugins(ReloadPlugin)
            .insert_resource(catalogue("127.0.0.1:1", "old", &arrival(old.clone())));
        let shown = filter::shown(&filter, &old.index);
        apply(app.world_mut(), old);
        app.insert_resource(filter).insert_resource(shown);
        app.world_mut()
            .resource_mut::<Messages<ViewRequest>>()
            .clear();
        app
    }

    fn discs(app: &mut App) -> Vec<(usize, Kind)> {
        let mut query = app.world_mut().query::<&Disc>();
        let mut discs: Vec<(usize, Kind)> =
            query.iter(app.world()).map(|d| (d.index, d.kind)).collect();
        discs.sort_by_key(|&(index, _)| index);
        discs
    }

    #[test]
    fn a_graph_switch_replaces_the_index_and_discs_and_resets_what_named_the_old_graph() {
        let mut app = app();
        assert_eq!(discs(&mut app).len(), 5);
        assert!(
            app.world()
                .resource::<GraphIndex>()
                .by_id
                .contains_key("old-fact")
        );

        // The load runs in the background, as from the bar, and its answer
        // is taken by the next update after it has come.
        let prepared = prepare(&fixture()).unwrap();
        let expected_index = prepared.index.clone();
        let expected_pickers = prepared.pickers.clone();
        app.insert_resource(Focus(Some(4)))
            .insert_resource(Hovered(Some(3)));
        app.world_mut().resource_mut::<Loading>().0 =
            Some(in_background("test load", move || async move { Ok(arrival(prepared)) }).unwrap());
        for _ in 0..200 {
            app.update();
            if app.world().resource::<Loading>().0.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            *app.world().resource::<LoadStatus>(),
            LoadStatus::Loaded {
                graph: "fixture".into(),
                nodes: 5
            }
        );

        assert_eq!(*app.world().resource::<GraphIndex>(), expected_index);
        assert_eq!(*app.world().resource::<Pickers>(), expected_pickers);
        let kinds: Vec<Kind> = discs(&mut app).into_iter().map(|(_, k)| k).collect();
        assert_eq!(
            kinds,
            [
                Kind::Topic,
                Kind::Topic,
                Kind::Fact,
                Kind::Fact,
                Kind::Inference
            ],
            "the fixture's five drawn nodes, the old five gone"
        );
        let places = app.world().resource::<Places>();
        assert_eq!(places.points.len(), 5);
        assert_eq!(places.topics.len(), 2);

        let filter = app.world().resource::<Filter>();
        assert_eq!(
            (
                &filter.topic,
                &filter.window,
                &filter.source,
                &filter.retrieval
            ),
            (&None, &None, &None, &None),
            "a topic, a window, a document and a retrieval record of the old graph mean nothing in the new"
        );
        assert!(!filter.kinds.topic && filter.kinds.fact, "kinds kept");
        assert_eq!(
            filter.metacontexts,
            Metacontexts::Only(["the-real".to_owned()].into())
        );
        assert_eq!(filter.mode, Mode::Hide);
        let shown = app.world().resource::<Shown>();
        assert_eq!(*shown, filter::shown(filter, &expected_index));
        assert_eq!(shown.passes.len(), 5);
        assert_eq!(
            (
                *app.world().resource::<Focus>(),
                *app.world().resource::<Hovered>()
            ),
            (Focus(None), Hovered(None)),
            "an index of the old graph names nothing in the new"
        );

        let catalogue = app.world().resource::<Catalogue>();
        assert_eq!(catalogue.graph, "fixture");
        assert_eq!(catalogue.session.session_id, "new-session");
        assert_eq!(catalogue.browsing, "new-session");
        assert_eq!(catalogue.hub, "127.0.0.1:1", "the hub stays");

        let requests: Vec<ViewRequest> = app
            .world_mut()
            .resource_mut::<Messages<ViewRequest>>()
            .drain()
            .collect();
        assert!(requests.contains(&ViewRequest::Preset(Preset::Free)));
    }

    #[test]
    fn a_failed_load_keeps_the_graph_drawn_and_says_why() {
        let mut app = app();
        let before = app.world().resource::<GraphIndex>().clone();
        app.world_mut().resource_mut::<Loading>().0 = Some(
            in_background("test load", || async { Err("the hub said no".to_owned()) }).unwrap(),
        );
        for _ in 0..200 {
            app.update();
            if app.world().resource::<Loading>().0.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            *app.world().resource::<LoadStatus>(),
            LoadStatus::Failed("the hub said no".into())
        );
        assert_eq!(*app.world().resource::<GraphIndex>(), before);
        assert_eq!(discs(&mut app).len(), 5);
        assert_eq!(
            app.world().resource::<Filter>().topic.as_deref(),
            Some("old-topic-1")
        );
    }

    #[test]
    fn a_read_again_keeps_the_retrieval_record_chosen() {
        let prepared = prepare(&fixture()).unwrap();
        let filter = Filter {
            retrieval: Some(RetrievalFocus {
                record_id: "000000000003".into(),
                retrieved: Some(["no-longer-drawn".to_owned()].into()),
            }),
            ..Filter::default()
        };
        assert_eq!(
            filter_for_refetch(&filter, &prepared.index),
            filter,
            "its ids are the record's, whatever the graph holds now"
        );
    }

    #[test]
    fn an_empty_graph_is_refused() {
        let snapshot = Snapshot {
            schema_version: Some(1),
            graph: "empty".into(),
            nodes: Vec::new(),
            edges: Vec::new(),
            timelines: Vec::new(),
            metacontexts: Vec::new(),
        };
        assert!(prepare(&snapshot).unwrap_err().contains("no node to draw"));
    }
}
