//! The topic map in the running scene (DESIGN.md §7): reading the topic
//! embeddings, fitting or extending the map, saving it, and moving the
//! topics, their claims following, to their places on it.
//!
//! [`MapState`] is the map drawn, where it is saved, and the read of the
//! embeddings under way or wanted. The start (`main.rs`) puts in the map
//! saved for the graph, already drawn, so the picture never jumps, and asks
//! for one read: [`Purpose::Start`]. When the answer comes, `topic_map.rs`
//! decides: with no map, or with one under another model than the hub
//! reports, a map is fitted and every topic moves to it; otherwise only
//! the topics the map does not know are put on it, by projection, and
//! nothing else moves. A map that changes is saved at once (`persist.rs`).
//!
//! A topic that arrives later, live or in a snapshot read again, waits on
//! its ring (`layout.rs`): its embedding is stored after it, so the read
//! for it is debounced, due [`topic_map::DEBOUNCE_SECONDS`] after the last
//! new topic of a burst, and then made once for all of them. A topic the
//! answer has no embedding for stays on its ring and the side panel says
//! so. A topic the map knows that stands anywhere else, one that came back
//! into the active set, say, is moved to its place on the map.
//!
//! Recalculate, the bar's button or `U` (`keys.rs`), sends
//! [`RecalculateLayout`]: the embeddings are read again and a new map is
//! fitted over the topics drawn, saved over the old, and every topic moves
//! to it. `--recalculate` asks for that at the start.
//!
//! The moves are tweened ([`LayoutTween`]) over [`TWEEN_SECONDS`]: every
//! disc and its place on the map, which the lines and the labels follow,
//! as a switch of time mode moves them along the time axis
//! (`time_mode.rs`), and the two compose, since one moves the ground and
//! the other the height. A move of every topic ends with the camera
//! fitting the view last asked for again ([`ViewRequest::Again`]). Before
//! events are applied, or a picture replaced, a move under way is finished
//! at once ([`finish_tween`]).
//!
//! If the hub has no embeddings route, an Epimemer older than the contract,
//! or the read fails, the topics stay where they are, on the circle or on
//! the saved map, and the bar says why.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;
use web_time::Instant;

use crate::apply::LiveGraph;
use crate::background::{Pending, Poll, in_background, poll};
use crate::camera::{Places, ViewRequest};
use crate::graph_index::GraphIndex;
use crate::hub::{self, EmbeddingsError, EmbeddingsRead};
use crate::keys::RECALCULATE_KEY;
use crate::layout::{self, Arrangement, Kind, Spread, follow, ring_radius, unfiled_centre};
use crate::persist::{self, MapKey, Store};
use crate::presets::ease_in_out;
use crate::relax;
use crate::reload::Catalogue;
use crate::scene::Disc;
use crate::topic_map::{
    self, Debounce, Newcomer, Plan, Purpose, TopicInput, TopicMap, note_topic, take_due,
};
use crate::ui_focus::UiFocus;

/// How long a move on the map takes: longer than a change of view, so
/// that where each topic goes can be followed.
pub const TWEEN_SECONDS: f32 = 1.5;

/// Asks for a new topic map, for this reason.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct RecalculateLayout {
    pub reason: String,
}

/// A read of the embeddings, as it comes back.
pub type Answer = Result<EmbeddingsRead, EmbeddingsError>;

/// The topic map drawn and what is being done about it.
#[derive(Resource)]
pub struct MapState {
    pub store: Store,
    pub key: MapKey,
    /// The map drawn, where there is one.
    pub map: Option<TopicMap>,
    /// The read under way, and why.
    pub reading: Option<(Purpose, Pending<Answer>)>,
    /// A read wanted, made once none is under way.
    pub wanted: Option<Purpose>,
    pub debounce: Debounce,
    /// The drawn topics the map does not know that a read is due for or
    /// was made for already, by id.
    pub noted: BTreeSet<String>,
    /// The drawn topics the last answer had no embedding for, by id.
    pub missing: BTreeSet<String>,
    /// Why there is no map to draw from, where that is known.
    pub unavailable: Option<String>,
    /// Whether the hub may have the route: once it said it has none, no
    /// read starts by itself.
    pub route: bool,
    /// The drawn topics the map knows that stand elsewhere, to be moved.
    pub misplaced: Vec<(usize, Vec2)>,
    /// The picture changed and its topics are still to be looked at.
    pub dirty: bool,
    /// The topics of the picture first drawn have been noted: the read
    /// the start wants is for them, so they need no debounced one.
    pub seeded: bool,
    /// What the bar says about the map, and whether it is an error.
    pub note: (String, bool),
}

impl MapState {
    /// The state for a graph drawn on `map`, wanting the read `wanted`.
    pub fn new(store: Store, key: MapKey, map: Option<TopicMap>, wanted: Purpose) -> Self {
        let note = match &map {
            Some(map) => (map_words(map), false),
            None => (
                "topics on the circle until the map is fitted".to_owned(),
                false,
            ),
        };
        MapState {
            store,
            key,
            map,
            reading: None,
            wanted: Some(wanted),
            debounce: Debounce::default(),
            noted: BTreeSet::new(),
            missing: BTreeSet::new(),
            unavailable: None,
            route: true,
            misplaced: Vec::new(),
            dirty: true,
            seeded: false,
            note,
        }
    }

    /// Whether nothing is waiting: no read under way, wanted or due, and
    /// no move to make.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub fn idle(&self) -> bool {
        self.reading.is_none()
            && (self.wanted.is_none() || !self.route_open())
            && self.debounce.due.is_none()
            && self.misplaced.is_empty()
            && !self.dirty
    }

    /// Where the drawn topic `id` stands, in words for the side panel.
    pub fn topic_note(&self, id: &str) -> String {
        let waiting = self.reading.is_some() || self.debounce.due.is_some();
        topic_map::topic_note(
            self.map.as_ref(),
            id,
            self.missing.contains(id),
            waiting,
            self.unavailable.as_deref(),
        )
    }
}

/// The bar's words for a map.
fn map_words(map: &TopicMap) -> String {
    let mut words = format!(
        "topic map of {} topics from {}",
        map.topics.len(),
        map.model()
    );
    if topic_map::drifted(map) {
        words.push_str(&format!(
            "; {} put on it since the fit, so Recalculate layout ({}) may read better",
            map.placed_since(),
            RECALCULATE_KEY.1
        ));
    }
    words
}

/// What the bar says about the map, and whether it is an error.
pub fn bar_note(state: &MapState) -> (String, bool) {
    match &state.reading {
        Some((Purpose::Recalculate(_), _)) => (
            "reading the topic embeddings to fit the map again...".to_owned(),
            false,
        ),
        Some(_) => ("reading the topic embeddings...".to_owned(), false),
        None => state.note.clone(),
    }
}

/// A move of nodes on the map under way: each node's place from and to,
/// and each disc's.
#[derive(Debug, Clone, PartialEq)]
pub struct Move {
    elapsed: f32,
    nodes: Vec<(usize, Vec2, Vec2)>,
    discs: Vec<(Entity, Vec2, Vec2)>,
    /// Fit the view again once it ends.
    refit: bool,
}

/// The move under way, if any.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct LayoutTween(pub Option<Move>);

/// The places with every move under way at its end.
pub fn settled_places(places: &Places, tween: Option<&LayoutTween>) -> Places {
    let mut settled = places.clone();
    if let Some(moving) = tween.and_then(|t| t.0.as_ref()) {
        for &(i, _, to) in &moving.nodes {
            if let Some(point) = settled.points.get_mut(i) {
                point.x = to.x;
                point.z = to.y;
            }
        }
    }
    settled
}

/// Follows the topic map. The app inserts the [`MapState`].
pub struct RelayoutPlugin;

impl Plugin for RelayoutPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RecalculateLayout>()
            .add_message::<ViewRequest>()
            .init_resource::<LayoutTween>()
            .init_resource::<UiFocus>()
            .add_systems(
                Update,
                (recalculate_on_key, watch_topics, drive_map, advance_tween).chain(),
            );
    }
}

/// `U` asks for a new map, unless a text field has the keyboard.
fn recalculate_on_key(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiFocus>,
    mut requests: MessageWriter<RecalculateLayout>,
) {
    if !ui.typing && keys.just_pressed(RECALCULATE_KEY.0) {
        requests.write(RecalculateLayout {
            reason: format!("asked for, by {}", RECALCULATE_KEY.1),
        });
    }
}

/// Notes the drawn topics the map does not know, and those it knows that
/// stand elsewhere, whenever the picture changes, once no move is under
/// way. A topic new to the map makes the read due again, after the
/// debounce; the topics of the picture first drawn are left to the read
/// the start asks for.
fn watch_topics(
    index: Res<GraphIndex>,
    places: Res<Places>,
    tween: Res<LayoutTween>,
    time: Res<Time>,
    state: Option<ResMut<MapState>>,
) {
    let Some(mut state) = state else {
        return;
    };
    if index.is_changed() && !state.dirty {
        state.bypass_change_detection().dirty = true;
    }
    if !state.dirty || tween.0.is_some() {
        return;
    }
    let state = state.bypass_change_detection();
    state.dirty = false;
    let mut fresh = Vec::new();
    let mut misplaced = Vec::new();
    for i in drawn_topics(&index) {
        let id = &index.ids[i];
        match state.map.as_ref().and_then(|m| m.place(id)) {
            Some(place) => {
                let at = places.points.get(i).map(|p| Vec2::new(p.x, p.z));
                if at.is_some_and(|at| at != place) {
                    misplaced.push((i, place));
                }
            }
            None if !state.noted.contains(id) => fresh.push(id.clone()),
            None => {}
        }
    }
    state.misplaced = misplaced;
    let first = !state.seeded;
    state.seeded = true;
    if !fresh.is_empty() {
        state.noted.extend(fresh);
        if !first && state.route {
            state.debounce = note_topic(time.elapsed_secs_f64());
        }
    }
}

/// The drawn topics, by index, in index order.
fn drawn_topics(index: &GraphIndex) -> Vec<usize> {
    (0..index.ids.len())
        .filter(|&i| index.kinds[i] == Kind::Topic && index.active[i])
        .collect()
}

/// Takes what was asked for and what has come: a new map asked for, a read
/// due after the debounce, a read's answer, and topics to move back to the
/// map; and starts the read wanted when none is under way.
fn drive_map(world: &mut World) {
    let Some(state) = world.get_resource::<MapState>() else {
        return;
    };
    let now = world.resource::<Time>().elapsed_secs_f64();
    let answer = match state.reading.as_ref().map(|(_, pending)| poll(pending)) {
        Some(Poll::Ready(answer)) => Some(answer),
        Some(Poll::Lost) => Some(Err(EmbeddingsError::Failed(
            "the read stopped without an answer".to_owned(),
        ))),
        Some(Poll::Waiting) | None => None,
    };
    let due = state.debounce.due.is_some_and(|due| due <= now);
    let startable = state.reading.is_none() && state.wanted.is_some() && state.route_open();
    let asked = !world.resource::<Messages<RecalculateLayout>>().is_empty();
    if !asked && answer.is_none() && state.misplaced.is_empty() && !due && !startable {
        return;
    }
    let asked: Vec<RecalculateLayout> = world
        .resource_mut::<Messages<RecalculateLayout>>()
        .drain()
        .collect();
    if let Some(answer) = answer {
        let purpose = world
            .resource_mut::<MapState>()
            .reading
            .take()
            .map_or(Purpose::Start, |(purpose, _)| purpose);
        take_answer(world, purpose, answer);
    }
    let misplaced = std::mem::take(&mut world.resource_mut::<MapState>().misplaced);
    if !misplaced.is_empty() {
        info!(
            "topic map: {} topics it knows stood elsewhere and move to it",
            misplaced.len()
        );
        move_topics(world, &misplaced.into_iter().collect(), None, false);
    }
    let start = {
        let mut state = world.resource_mut::<MapState>();
        if let Some(request) = asked.last() {
            state.wanted = Some(Purpose::Recalculate(request.reason.clone()));
        }
        let (debounce, fired) = take_due(state.debounce, now);
        if fired {
            state.debounce = debounce;
            if state.wanted.is_none() {
                state.wanted = Some(Purpose::Newcomers);
            }
        }
        if state.reading.is_none() && state.route_open() {
            state.wanted.take()
        } else {
            None
        }
    };
    if let Some(purpose) = start {
        start_read(world, purpose);
    }
}

impl MapState {
    /// Whether the read wanted may start: Recalculate always, and the
    /// rest while the hub may have the route.
    fn route_open(&self) -> bool {
        self.route || matches!(self.wanted, Some(Purpose::Recalculate(_)))
    }
}

/// Starts a read of the embeddings of the graph drawn, from the session the
/// graph was read from.
fn start_read(world: &mut World, purpose: Purpose) {
    let Some(catalogue) = world.get_resource::<Catalogue>() else {
        return;
    };
    let (hub, session, graph) = (
        catalogue.hub.clone(),
        catalogue.session.session_id.clone(),
        catalogue.graph.clone(),
    );
    info!("topic map: reading the topic embeddings of {graph:?} ({purpose:?})");
    let work = move || async move { hub::embeddings(&hub, &session, &graph).await };
    let mut state = world.resource_mut::<MapState>();
    match in_background("read the topic embeddings", work) {
        Ok(pending) => state.reading = Some((purpose, pending)),
        Err(message) => {
            warn!("topic map: {message}");
            state.note = (
                format!("could not read the topic embeddings: {message}"),
                true,
            );
        }
    }
}

/// Does what an answer asks: fits a map, puts new topics on the map, or
/// says why it cannot.
pub fn take_answer(world: &mut World, purpose: Purpose, answer: Answer) {
    let read = match answer {
        Ok(read) => read,
        Err(error) => {
            let mut state = world.resource_mut::<MapState>();
            if error == EmbeddingsError::NoRoute {
                state.route = false;
            }
            let (lead, kept) = if state.map.is_some() {
                (
                    "Topic map not updated",
                    "the saved map is kept, and new topics stay on their ring",
                )
            } else {
                ("No topic map", "the topics stay on the circle")
            };
            warn!("topic map: {error}; {kept}");
            state.unavailable = Some(error.to_string());
            state.note = (format!("{lead}: {error}; {kept}."), true);
            return;
        }
    };
    world.resource_mut::<MapState>().route = true;
    let embeddings = &read.embeddings;
    info!(
        "topic embeddings of {:?} from session {}: {} vectors under {}, {:.1} MB, fetched in {:.0} ms and parsed in {:.0} ms",
        embeddings.graph,
        read.session,
        embeddings.embeddings.len(),
        embeddings
            .models
            .iter()
            .map(|(m, d)| format!("{m} ({d})"))
            .collect::<Vec<_>>()
            .join(", "),
        read.bytes as f64 / 1e6,
        read.download.as_secs_f64() * 1e3,
        read.parse.as_secs_f64() * 1e3,
    );
    let wanted_model = world
        .resource::<MapState>()
        .map
        .as_ref()
        .map(|m| m.model().to_owned());
    let Some((model, dimension)) = topic_map::choose_model(embeddings, wanted_model.as_deref())
    else {
        let ids: Vec<String> = {
            let index = world.resource::<GraphIndex>();
            drawn_topics(index)
                .into_iter()
                .map(|i| index.ids[i].clone())
                .collect()
        };
        let mut state = world.resource_mut::<MapState>();
        state.missing = ids.iter().cloned().collect();
        state.noted.extend(ids);
        state.note = (
            "No topic map: the hub has no embedding for any topic of this graph.".to_owned(),
            true,
        );
        return;
    };
    let vectors = topic_map::topic_vectors(embeddings, &model, dimension);
    let current = world.resource::<MapState>().map.clone();
    match topic_map::plan(current.as_ref(), &purpose, &model, dimension) {
        Plan::Fit(why) => fit(world, &model, &vectors, &why),
        Plan::Extend => extend(world, &vectors),
    }
}

/// The topic map's view of the picture: each drawn topic, how many claims
/// stand beside it, and its parent by `subtopic_of`.
struct Topics {
    /// Drawn topics by index.
    topics: Vec<usize>,
    claims: HashMap<usize, usize>,
    parents: HashMap<usize, usize>,
}

fn topics_of(index: &GraphIndex) -> Topics {
    let topics = drawn_topics(index);
    let mut claims: HashMap<usize, usize> = HashMap::new();
    for (i, filing) in index.filing.iter().enumerate() {
        if let Some(t) = filing
            && index.active[i]
            && index.kinds[i] != Kind::Topic
        {
            *claims.entry(*t).or_default() += 1;
        }
    }
    let mut parents = HashMap::new();
    for &t in &topics {
        for &child in &index.subtopics[t] {
            parents.entry(child).or_insert(t);
        }
    }
    Topics {
        topics,
        claims,
        parents,
    }
}

/// Fits a new map over the drawn topics with a vector, saves it, and moves
/// every topic to it, the claims following at its spread.
fn fit(world: &mut World, model: &str, vectors: &HashMap<&str, &[f32]>, why: &str) {
    let started = Instant::now();
    let (graph, inputs_owned, missing, radius) = {
        let index = world.resource::<GraphIndex>();
        let found = topics_of(index);
        let with: Vec<usize> = found
            .topics
            .iter()
            .copied()
            .filter(|&t| vectors.contains_key(index.ids[t].as_str()))
            .collect();
        let position: HashMap<usize, usize> =
            with.iter().enumerate().map(|(k, &t)| (t, k)).collect();
        let inputs: Vec<(usize, usize, Option<usize>)> = with
            .iter()
            .map(|&t| {
                (
                    t,
                    found.claims.get(&t).copied().unwrap_or(0),
                    found.parents.get(&t).and_then(|p| position.get(p).copied()),
                )
            })
            .collect();
        let missing: Vec<String> = found
            .topics
            .iter()
            .filter(|&&t| !vectors.contains_key(index.ids[t].as_str()))
            .map(|&t| index.ids[t].clone())
            .collect();
        let graph = world.resource::<MapState>().key.graph.clone();
        (
            graph,
            inputs,
            missing,
            layout::circle_radius(found.topics.len()),
        )
    };
    if inputs_owned.is_empty() {
        let mut state = world.resource_mut::<MapState>();
        state.missing = missing.iter().cloned().collect();
        state.noted.extend(missing);
        state.note = (
            "No topic map: no drawn topic has an embedding under the hub's model.".to_owned(),
            true,
        );
        return;
    }
    let fitted = {
        let index = world.resource::<GraphIndex>();
        let inputs: Vec<TopicInput> = inputs_owned
            .iter()
            .map(|&(t, claims, parent)| TopicInput {
                id: &index.ids[t],
                vector: vectors[index.ids[t].as_str()],
                claims,
                parent,
            })
            .collect();
        topic_map::fit_map(&graph, model, &inputs, radius)
    };
    let fitted = match fitted {
        Ok(fitted) => fitted,
        Err(message) => {
            warn!("topic map: the fit failed: {message}");
            world.resource_mut::<MapState>().note =
                (format!("No topic map: the fit failed: {message}."), true);
            return;
        }
    };
    info!(
        "topic map: fitted {} topics under {model} in {:.1} ms ({why}): {} steps of relaxation{}, median distance between nearest topics {:.2}, claim spread {:.2}, outer radius {:.0}; {} topics with no embedding stay where they are",
        fitted.map.topics.len(),
        started.elapsed().as_secs_f64() * 1e3,
        fitted.iterations,
        if fitted.settled {
            ", settled"
        } else {
            ", not settled"
        },
        fitted.median_nearest.unwrap_or(0.0),
        fitted.map.spread,
        fitted.map.radius,
        missing.len(),
    );
    let moved: HashMap<usize, Vec2> = inputs_owned
        .iter()
        .zip(&fitted.points)
        .map(|(&(t, _, _), &p)| (t, p))
        .collect();
    let map = fitted.map;
    move_topics(world, &moved, Some((map.spread, map.radius)), true);
    let saved = save(world, &map);
    let mut state = world.resource_mut::<MapState>();
    state.missing = missing.iter().cloned().collect();
    state.noted = missing.into_iter().collect();
    state.unavailable = None;
    let mut words = format!("{}; fitted because {why}", map_words(&map));
    if let Err(message) = &saved {
        words.push_str(&format!("; not saved: {message}"));
    }
    state.note = (words, saved.is_err());
    state.map = Some(map);
}

/// Puts the drawn topics the map does not know on it, those with a vector,
/// saves it, and moves them there; nothing else moves.
fn extend(world: &mut World, vectors: &HashMap<&str, &[f32]>) {
    let Some(mut map) = world.resource::<MapState>().map.clone() else {
        return;
    };
    let (placed, missing) = {
        let index = world.resource::<GraphIndex>();
        let places = world.resource::<Places>();
        let found = topics_of(index);
        let (newcomers, others): (Vec<usize>, Vec<usize>) = found
            .topics
            .iter()
            .partition(|&&t| !map.topics.contains_key(&index.ids[t]));
        let room = |t: usize| relax::room(found.claims.get(&t).copied().unwrap_or(0));
        let ground = |t: usize| Vec2::new(places.points[t].x, places.points[t].z);
        let taken: Vec<(Vec2, f32)> = others.iter().map(|&t| (ground(t), room(t))).collect();
        let (with, missing): (Vec<usize>, Vec<usize>) = newcomers
            .iter()
            .partition(|&&t| vectors.contains_key(index.ids[t].as_str()));
        let arriving: Vec<Newcomer> = with
            .iter()
            .map(|&t| Newcomer {
                id: &index.ids[t],
                vector: vectors[index.ids[t].as_str()],
                claims: found.claims.get(&t).copied().unwrap_or(0),
            })
            .collect();
        let places_given = topic_map::extend_map(&mut map, &arriving, &taken);
        let placed: Vec<(usize, Vec2)> = with
            .iter()
            .zip(places_given)
            .filter_map(|(&t, place)| Some((t, place?)))
            .collect();
        let missing: Vec<String> = missing.iter().map(|&t| index.ids[t].clone()).collect();
        (placed, missing)
    };
    let saved = if placed.is_empty() {
        Ok(())
    } else {
        info!(
            "topic map: {} new topics put on the map by their embeddings; {} with none stay on their ring",
            placed.len(),
            missing.len()
        );
        move_topics(world, &placed.iter().copied().collect(), None, false);
        save(world, &map)
    };
    let mut state = world.resource_mut::<MapState>();
    state.missing = missing.iter().cloned().collect();
    state.noted = missing.into_iter().collect();
    state.unavailable = None;
    let mut words = map_words(&map);
    if let Err(message) = &saved {
        words.push_str(&format!("; not saved: {message}"));
    }
    state.note = (words, saved.is_err());
    state.map = Some(map);
}

/// Saves the map where the state keeps maps.
fn save(world: &World, map: &TopicMap) -> Result<(), String> {
    let state = world.resource::<MapState>();
    if matches!(state.store, Store::Off) {
        info!("topic map: not saved, since --no-persist was given");
        return Ok(());
    }
    let saved = persist::write_map(&state.store, &state.key, map);
    match &saved {
        Ok(()) => info!(
            "topic map: saved to {}",
            persist::describe(&state.store, &state.key)
        ),
        Err(message) => warn!("topic map: {message}"),
    }
    saved
}

/// Moves the topics in `moved` to their places on the map, the claims
/// following, as a tween. With `arrangement`, the claims spread anew at its
/// spread and the ring and the unfiled region stand outside its radius:
/// the picture is on a new map. `refit` fits the view again at the end.
pub fn move_topics(
    world: &mut World,
    moved: &HashMap<usize, Vec2>,
    arrangement: Option<(f32, f32)>,
    refit: bool,
) {
    finish_tween(world);
    let (targets, ring, spread) = {
        let index = world.resource::<GraphIndex>();
        let places = world.resource::<Places>();
        let live = world.resource::<LiveGraph>();
        let ground: Vec<Vec2> = places.points.iter().map(|p| Vec2::new(p.x, p.z)).collect();
        let from = Arrangement {
            spread: live.spread.0,
            unfiled: unfiled_centre(&live.ring),
        };
        let (ring, spread) = match arrangement {
            Some((spread, radius)) => (
                layout::TopicRing {
                    radius: ring_radius(live.ring.count, Some(radius)),
                    ..live.ring
                },
                Spread(spread),
            ),
            None => (live.ring, live.spread),
        };
        let to = Arrangement {
            spread: spread.0,
            unfiled: unfiled_centre(&ring),
        };
        let targets = follow(
            &index.ids,
            &index.kinds,
            &index.filing,
            &ground,
            moved,
            from,
            to,
        );
        (targets, ring, spread)
    };
    {
        let mut live = world.resource_mut::<LiveGraph>();
        if live.ring != ring {
            live.ring = ring;
        }
        if live.spread != spread {
            live.spread = spread;
        }
    }
    start_tween(world, targets, refit);
}

/// Starts a tween of these nodes to these places on the map.
fn start_tween(world: &mut World, targets: Vec<(usize, Vec2)>, refit: bool) {
    if targets.is_empty() {
        if refit {
            world.write_message(ViewRequest::Again);
        }
        return;
    }
    let places = world.resource::<Places>();
    let nodes: Vec<(usize, Vec2, Vec2)> = targets
        .iter()
        .filter_map(|&(i, to)| {
            let p = places.points.get(i)?;
            Some((i, Vec2::new(p.x, p.z), to))
        })
        .collect();
    let wanted: HashMap<usize, Vec2> = targets.into_iter().collect();
    let mut query = world.query::<(Entity, &Disc, &Transform)>();
    let discs: Vec<(Entity, Vec2, Vec2)> = query
        .iter(world)
        .filter_map(|(entity, disc, transform)| {
            let to = *wanted.get(&disc.index)?;
            Some((
                entity,
                Vec2::new(transform.translation.x, transform.translation.z),
                to,
            ))
        })
        .collect();
    world.resource_mut::<LayoutTween>().0 = Some(Move {
        elapsed: 0.0,
        nodes,
        discs,
        refit,
    });
}

/// Ends a move under way at once, every disc and place where it would
/// leave them, so that the picture can be changed under it.
pub fn finish_tween(world: &mut World) {
    let Some(moving) = world
        .get_resource_mut::<LayoutTween>()
        .and_then(|mut tween| tween.0.take())
    else {
        return;
    };
    for &(entity, _, to) in &moving.discs {
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            transform.translation.x = to.x;
            transform.translation.z = to.y;
        }
    }
    if let Some(mut places) = world.get_resource_mut::<Places>() {
        for &(i, _, to) in &moving.nodes {
            if let Some(point) = places.points.get_mut(i) {
                point.x = to.x;
                point.z = to.y;
            }
        }
    }
    if moving.refit {
        world.write_message(ViewRequest::Again);
    }
}

/// Moves every disc, and the places, a step along the move under way.
fn advance_tween(
    time: Res<Time>,
    mut tween: ResMut<LayoutTween>,
    mut places: ResMut<Places>,
    mut transforms: Query<&mut Transform, With<Disc>>,
    mut views: MessageWriter<ViewRequest>,
) {
    if tween.0.is_none() {
        return;
    }
    let Some(moving) = tween.0.as_mut() else {
        return;
    };
    moving.elapsed += time.delta_secs();
    let t = (moving.elapsed / TWEEN_SECONDS).min(1.0);
    let s = ease_in_out(t);
    let at = |from: Vec2, to: Vec2| if t >= 1.0 { to } else { from.lerp(to, s) };
    for &(entity, from, to) in &moving.discs {
        if let Ok(mut transform) = transforms.get_mut(entity) {
            let p = at(from, to);
            transform.translation.x = p.x;
            transform.translation.z = p.y;
        }
    }
    for &(i, from, to) in &moving.nodes {
        if let Some(point) = places.points.get_mut(i) {
            let p = at(from, to);
            point.x = p.x;
            point.z = p.y;
        }
    }
    if t >= 1.0 {
        if moving.refit {
            views.write(ViewRequest::Again);
        }
        tween.0 = None;
    }
}

/// After a switch to another graph: its saved map, drawn already, and a
/// read of its embeddings.
pub fn graph_switched(world: &mut World, hub: &str, graph: &str, map: Option<TopicMap>) {
    let Some(store) = world.get_resource::<MapState>().map(|s| s.store.clone()) else {
        return;
    };
    let key = MapKey {
        hub: hub.to_owned(),
        graph: graph.to_owned(),
    };
    world.insert_resource(MapState::new(store, key, map, Purpose::Start));
    world.insert_resource(LayoutTween::default());
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::filter::Shown;
    use crate::graph_index::tests::{edge, node};
    use crate::hub::{Embeddings, NodeType, SessionInfo, Snapshot};
    use crate::reload::{prepare, prepare_seeded};
    use crate::scene::{self, DiscMaterial, Nodes};
    use crate::topic_map::tests::{answer, grouped};

    /// A graph of `topics` topics, the first two with a claim each.
    fn snapshot(topics: usize) -> Snapshot {
        let mut nodes: Vec<_> = (0..topics)
            .map(|i| node(&format!("t{i}"), NodeType::Topic, 1))
            .collect();
        nodes.push(node("f0", NodeType::Fact, 2));
        nodes.push(node("f1", NodeType::Fact, 3));
        Snapshot {
            schema_version: Some(1),
            graph: "g".into(),
            nodes,
            edges: vec![
                edge("f0", "t0", "extracted_under_topic"),
                edge("f1", "t1", "extracted_under_topic"),
            ],
            timelines: Vec::new(),
            metacontexts: Vec::new(),
        }
    }

    fn key() -> MapKey {
        MapKey {
            hub: "127.0.0.1:1".into(),
            graph: "g".into(),
        }
    }

    fn catalogue() -> Catalogue {
        let session = SessionInfo {
            session_id: "s".into(),
            pid: 1,
            backend: "memory".into(),
            active_graph: "g".into(),
            started_at: "2026-10-21T00:00:00Z".into(),
            connected: true,
            last_event_at: None,
            schema_version: Some(1),
        };
        Catalogue {
            hub: "127.0.0.1:1".into(),
            sessions: vec![session.clone()],
            session,
            graph: "g".into(),
            browsing: "s".into(),
            graphs: None,
        }
    }

    /// An app drawing `snapshot` on `map`, with a store in memory and no
    /// read wanted.
    fn app(snapshot: &Snapshot, map: Option<TopicMap>, store: Store) -> App {
        let prepared = match &map {
            Some(map) => prepare_seeded(snapshot, None, Some(map)).unwrap(),
            None => prepare(snapshot).unwrap(),
        };
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                250,
            )))
            .init_asset::<Mesh>()
            .init_asset::<DiscMaterial>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(Shown::default())
            .insert_resource(prepared.index)
            .insert_resource(prepared.places)
            .insert_resource(prepared.live)
            .insert_resource(catalogue())
            .insert_resource(Nodes(Vec::new()))
            .add_plugins(RelayoutPlugin);
        let mut state = MapState::new(store, key(), map, Purpose::Start);
        state.wanted = None;
        app.insert_resource(state);
        scene::spawn_discs(app.world_mut(), prepared.nodes, &[], &Shown::default());
        app.update();
        app
    }

    fn ground(app: &App, id: &str) -> Vec2 {
        let index = app.world().resource::<GraphIndex>();
        let p = app.world().resource::<Places>().points[index.by_id[id]];
        Vec2::new(p.x, p.z)
    }

    fn grounds(app: &App) -> Vec<Vec2> {
        app.world()
            .resource::<Places>()
            .points
            .iter()
            .map(|p| Vec2::new(p.x, p.z))
            .collect()
    }

    fn deliver(app: &mut App, purpose: Purpose, embeddings: Embeddings) {
        let read = EmbeddingsRead {
            embeddings,
            bytes: 0,
            download: Duration::ZERO,
            parse: Duration::ZERO,
            session: "s".into(),
        };
        take_answer(app.world_mut(), purpose, Ok(read));
        for _ in 0..10 {
            app.update();
        }
    }

    #[test]
    fn with_no_map_the_topics_move_from_the_circle_to_a_fitted_map_which_is_saved() {
        let store = Store::Memory(Default::default());
        let snapshot = snapshot(30);
        let mut app = app(&snapshot, None, store.clone());
        let circle = grounds(&app);
        let vectors = grouped(30, 8);
        deliver(&mut app, Purpose::Start, answer("m", &vectors));
        let state = app.world().resource::<MapState>();
        let map = state.map.clone().expect("a map was fitted");
        assert!(!state.note.1, "{:?}", state.note);
        assert_eq!(
            persist::read_map(&store, &key()),
            Ok(Some(map.clone())),
            "and saved"
        );
        for i in 0..30 {
            let id = format!("t{i}");
            assert_eq!(
                ground(&app, &id),
                map.place(&id).unwrap(),
                "{id} on the map"
            );
        }
        assert_ne!(grounds(&app), circle);
        // The claims followed their topics, at the map's spread.
        let f0 = ground(&app, "f0").distance(ground(&app, "t0"));
        assert!((f0 - layout::CLAIM_CLEARANCE).abs() < 1e-3);
        assert_eq!(
            app.world().resource::<LiveGraph>().spread,
            Spread(map.spread)
        );
        assert!(
            app.world().resource::<LayoutTween>().0.is_none(),
            "the move ended"
        );
        // Drawing the same graph on the saved map gives the same picture.
        let again = prepare_seeded(&snapshot, None, Some(&map)).unwrap();
        let drawn = grounds(&app);
        for (i, p) in again.places.points.iter().enumerate() {
            assert_eq!(Vec2::new(p.x, p.z), drawn[i], "node {i}");
        }
    }

    #[test]
    fn a_saved_map_is_drawn_at_once_and_an_answer_under_its_model_moves_nothing() {
        let store = Store::Memory(Default::default());
        let snapshot = snapshot(30);
        let vectors = grouped(30, 8);
        let mut first = app(&snapshot, None, store.clone());
        deliver(&mut first, Purpose::Start, answer("m", &vectors));
        let map = persist::read_map(&store, &key()).unwrap().unwrap();

        let mut second = app(&snapshot, Some(map.clone()), store.clone());
        let drawn = grounds(&second);
        assert_eq!(drawn, grounds(&first), "the saved map, before any read");
        deliver(&mut second, Purpose::Start, answer("m", &vectors));
        assert_eq!(grounds(&second), drawn, "nothing moved");
        assert_eq!(second.world().resource::<MapState>().map, Some(map.clone()));
        assert_eq!(persist::read_map(&store, &key()), Ok(Some(map)));
    }

    #[test]
    fn another_model_fits_the_map_again_with_a_notice() {
        let store = Store::Memory(Default::default());
        let snapshot = snapshot(30);
        let mut app = app(&snapshot, None, store.clone());
        deliver(&mut app, Purpose::Start, answer("old", &grouped(30, 8)));
        let old = app.world().resource::<MapState>().map.clone().unwrap();
        let mut r = crate::projection::tests::noise(99);
        let other: Vec<(String, Vec<f32>)> = (0..30)
            .map(|i| (format!("t{i}"), (0..12).map(|_| r()).collect()))
            .collect();
        deliver(&mut app, Purpose::Start, answer("new", &other));
        let state = app.world().resource::<MapState>();
        let new = state.map.clone().unwrap();
        assert_eq!(new.model(), "new");
        assert_ne!(new.topics, old.topics);
        assert!(state.note.0.contains("from old to new"), "{:?}", state.note);
        assert_eq!(persist::read_map(&store, &key()), Ok(Some(new)));
    }

    #[test]
    fn a_new_topic_waits_for_the_burst_then_lands_on_the_map_and_nothing_else_moves() {
        let store = Store::Memory(Default::default());
        let snapshot = snapshot(30);
        let vectors = grouped(31, 8);
        let mut app = app(&snapshot, None, store.clone());
        deliver(&mut app, Purpose::Start, answer("m", &vectors[..30]));
        let before = grounds(&app);

        // A new topic arrives live: it takes a ring slot, and the read
        // becomes due two seconds later.
        let mut owned = crate::apply::tests::Owned {
            index: app.world_mut().remove_resource::<GraphIndex>().unwrap(),
            places: app.world_mut().remove_resource::<Places>().unwrap(),
            time: crate::time_axis::TimeModel::default(),
            live: app.world_mut().remove_resource::<LiveGraph>().unwrap(),
            mode: crate::time_axis::TimeMode::Recorded,
        };
        let model = prepare(&snapshot).unwrap();
        owned.time = model.time;
        let (_, changes) = crate::apply::tests::run(
            &mut owned,
            &[crate::apply::tests::stored(node("t30", NodeType::Topic, 4))],
        );
        app.insert_resource(owned.index)
            .insert_resource(owned.places)
            .insert_resource(owned.live)
            .insert_resource(owned.time);
        crate::apply::realize(app.world_mut(), &changes);
        app.update();
        let due = app.world().resource::<MapState>().debounce.due;
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        assert!(
            due.is_some_and(|d| (d - now - topic_map::DEBOUNCE_SECONDS).abs() < 0.3),
            "{due:?} at {now}"
        );
        assert!(
            app.world().resource::<MapState>().reading.is_none(),
            "not yet"
        );
        let on_ring = ground(&app, "t30");
        assert!(
            app.world()
                .resource::<MapState>()
                .topic_note("t30")
                .contains("until its embedding")
        );

        // Once due, the read starts; its answer puts t30 on the map.
        let mut updates = 0;
        while app.world().resource::<MapState>().debounce.due.is_some() {
            app.update();
            updates += 1;
            assert!(updates < 20, "the read never came due");
        }
        assert!(updates >= 7, "not before two seconds: {updates} updates");
        assert!(app.world().resource::<MapState>().reading.is_some());
        app.world_mut().resource_mut::<MapState>().reading = None;
        app.world_mut().resource_mut::<MapState>().wanted = None;
        deliver(&mut app, Purpose::Newcomers, answer("m", &vectors));
        let after = grounds(&app);
        assert_eq!(&after[..before.len()], &before[..], "nothing else moved");
        let landed = ground(&app, "t30");
        assert_ne!(landed, on_ring);
        let map = persist::read_map(&store, &key()).unwrap().unwrap();
        assert_eq!(map.place("t30"), Some(landed), "and it is saved");
        assert_eq!(map.placed_since(), 1);
    }

    #[test]
    fn a_topic_with_no_embedding_stays_on_its_ring_and_the_panel_says_so() {
        let store = Store::Memory(Default::default());
        let snapshot = snapshot(30);
        let mut app = app(&snapshot, None, store);
        let vectors = grouped(30, 8);
        let without_t5: Vec<_> = vectors
            .iter()
            .filter(|(id, _)| id != "t5")
            .cloned()
            .collect();
        let circle = ground(&app, "t5");
        deliver(&mut app, Purpose::Start, answer("m", &without_t5));
        assert_eq!(ground(&app, "t5"), circle);
        let state = app.world().resource::<MapState>();
        assert!(state.missing.contains("t5"));
        assert!(
            state.topic_note("t5").contains("no embedding"),
            "{}",
            state.topic_note("t5")
        );
        assert!(state.topic_note("t6").contains("placed by its embedding"));
    }

    #[test]
    fn a_topic_the_map_knows_standing_elsewhere_goes_back_with_its_claim() {
        let store = Store::Memory(Default::default());
        let snapshot = snapshot(30);
        let mut first = app(&snapshot, None, store.clone());
        deliver(&mut first, Purpose::Start, answer("m", &grouped(30, 8)));
        let map = persist::read_map(&store, &key()).unwrap().unwrap();
        let mut app = app(&snapshot, Some(map.clone()), store);
        let drawn = grounds(&app);
        // As a snapshot read again over a picture caught mid-move would:
        // t0 and its claim stand elsewhere.
        let (t0, f0) = {
            let index = app.world().resource::<GraphIndex>();
            (index.by_id["t0"], index.by_id["f0"])
        };
        {
            let mut places = app.world_mut().resource_mut::<Places>();
            for i in [t0, f0] {
                places.points[i].x += 40.0;
            }
        }
        app.world_mut().resource_mut::<GraphIndex>().set_changed();
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(grounds(&app), drawn, "back where the map puts them");
        assert_eq!(app.world().resource::<MapState>().map, Some(map));
    }

    #[test]
    fn a_switch_to_another_graph_draws_its_map_and_reads_its_embeddings() {
        let snapshot = snapshot(5);
        let mut app = app(&snapshot, None, Store::Off);
        graph_switched(app.world_mut(), "127.0.0.1:1", "other", None);
        let state = app.world().resource::<MapState>();
        assert_eq!(state.key.graph, "other");
        assert_eq!(state.wanted, Some(Purpose::Start));
        assert!(state.map.is_none() && !state.seeded);
    }

    #[test]
    fn a_hub_without_the_route_keeps_the_circle_and_says_why() {
        let snapshot = snapshot(10);
        let mut app = app(&snapshot, None, Store::Off);
        let circle = grounds(&app);
        take_answer(
            app.world_mut(),
            Purpose::Start,
            Err(EmbeddingsError::NoRoute),
        );
        app.update();
        assert_eq!(grounds(&app), circle);
        let state = app.world().resource::<MapState>();
        assert!(!state.route);
        assert!(
            state.note.1 && state.note.0.contains("no embeddings route"),
            "{:?}",
            state.note
        );
        assert!(state.idle());
    }

    #[test]
    fn recalculate_is_asked_for_by_its_key_and_reads_again() {
        let snapshot = snapshot(10);
        let mut app = app(&snapshot, None, Store::Off);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(RECALCULATE_KEY.0);
        app.update();
        let state = app.world().resource::<MapState>();
        assert!(
            matches!(&state.reading, Some((Purpose::Recalculate(why), _)) if why.contains("U")),
            "a read for a new map is under way"
        );
    }
}
