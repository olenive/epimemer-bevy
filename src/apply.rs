//! Applying the event stream to the picture drawn (DESIGN.md §11), one
//! event at a time, at the cost of what each event changes.
//!
//! The picture is four resources: the [`GraphIndex`], the camera's
//! [`Places`], the [`TimeModel`], and [`LiveGraph`], which keeps what
//! applying events needs beyond what the scene reads: each node's recorded
//! instant, how each claim was placed, the topic ring and the ranks around
//! each topic, the timelines and each claim's timelinks, the edges already
//! seen, and the edges waiting for an end that has not arrived. [`apply`]
//! changes them for one event and notes in [`Changes`] which discs to
//! spawn, despawn or reshape; [`realize`] does that to the scene once per
//! batch. Nothing here lays the graph out again or spawns every disc again.
//!
//! - `node_stored`: a new node is placed without moving any other
//!   (DESIGN.md §7). A topic takes the ring's next slot (`layout.rs`), where
//!   it waits until its embedding puts it on the topic map (`relayout.rs`); a
//!   claim is placed in the unfiled region, provisionally, since its filing
//!   edge comes as an event of its own, and moves to its topic once, when a
//!   filing edge or a tag arrives (`edge_stored` below). A node recorded
//!   after the top of the time axis grows the axis upward at its own scale
//!   (`layout::extend_axis`), so no height already drawn changes, and the
//!   axis, its ticks and "now" follow. A node already drawn is updated in
//!   place: its content, its confidence, its status, and every other field
//!   the side panel shows. A node of a kind this client does not draw, or
//!   one outside the active set, is not drawn, as the snapshot would not
//!   hold it.
//! - `edge_stored`: classified as the snapshot's edges are (`edge_index.rs`).
//!   A membership edge updates the keys the filters read (tags, sources,
//!   metacontexts, subtopics) and, for a claim placed provisionally, its
//!   place; a succession or a claim-to-claim edge joins the edge index, so
//!   that a connector appears at once and a line on the next hover; a
//!   `timelink` places its claim in claim time again. An edge whose end is
//!   not drawn yet waits for it, under that end's id. An edge already seen
//!   changes nothing.
//! - `node_status_changed`: a node that leaves the active set is no longer
//!   drawn and its disc goes; its succession edges keep the end as an id,
//!   so the connector to it becomes a stub with a marker (DESIGN.md §5). A
//!   node drawn earlier that comes back is drawn again where it was; one
//!   this client never drew cannot be, and asks for the snapshot again.
//! - `timeline_stored`: the timeline is replaced, and every claim with a
//!   `timelink` to it is placed in claim time again.
//!
//! Events about sessions, and the decision of which events reach here, are
//! `live.rs`'s.

use std::collections::{BTreeSet, HashMap, HashSet};

use bevy::prelude::*;

use crate::camera::Places;
use crate::clip::clipped_span;
use crate::edge_index::{Edge, EdgeClass, End, classify};
use crate::graph_index::GraphIndex;
use crate::hub::{EdgeView, MetacontextView, NodeView, Snapshot, TimelineView, parse_timestamp};
use crate::layout::{
    Filing, Kind, Layout, Seed, SeedPlace, Spread, TimeAxis, TopicRing, claim_offset_spread,
    extend_axis, kind_of, ring_point, time_y, unfiled_centre,
};
use crate::presets::{self, NowLayer, Region};
use crate::scene::{self, Disc, DiscAssets};
use crate::stream::{Event, StatusChange};
use crate::time_axis::{
    self, NowSource, Placement, Span, TimeMode, TimeModel, add_link, centre, claim_placement,
    disc_at, linked_order, read_validity,
};

/// The most edges kept waiting for an end that has not arrived. Past it an
/// edge that would wait is dropped; a snapshot read again restores it.
pub const MAX_WAITING: usize = 50_000;

/// What applying events needs beyond what the scene reads, in node order
/// where it is a list.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct LiveGraph {
    pub graph: String,
    /// Each node's recorded instant, in microseconds since the Unix epoch.
    pub recorded: Vec<i64>,
    /// The rule that placed each claim; a topic's is [`Filing::Filed`].
    pub filing: Vec<Filing>,
    /// Whether each node's place is final (`layout::Placed::settled`).
    pub settled: Vec<bool>,
    pub ring: TopicRing,
    /// How many claims have been placed beside each topic, by its id.
    pub ranks: HashMap<String, usize>,
    /// How many claims stand in the unfiled region.
    pub unfiled: usize,
    /// The claim spread claims are placed at: the topic map's, or the
    /// circle's (`layout.rs`).
    pub spread: Spread,
    pub timelines: Vec<TimelineView>,
    /// Each node's timelinks: the timeline and the timepoint each names.
    pub timelinks: Vec<Vec<(String, Option<String>)>>,
    pub metacontexts: Vec<MetacontextView>,
    /// Every edge seen, by id.
    pub edge_ids: HashSet<String>,
    /// Where each claim-to-claim and succession edge is in the edge index.
    pub edge_slots: HashMap<String, usize>,
    /// The edges of the edge index with an end that is not drawn, by that
    /// end's id.
    pub absent: HashMap<String, Vec<usize>>,
    /// Edges waiting for an end that is not drawn yet, by that end's id.
    pub waiting: HashMap<String, Vec<EdgeView>>,
    pub waiting_count: usize,
}

/// The [`LiveGraph`] of a snapshot as the layout placed it and the index
/// indexed it.
pub fn live_graph(snapshot: &Snapshot, layout: &Layout, index: &GraphIndex) -> LiveGraph {
    let count = layout.nodes.len();
    let mut recorded = vec![0_i64; count];
    let mut seen = vec![false; count];
    for node in &snapshot.nodes {
        if let Some(&i) = index.by_id.get(&node.node_id)
            && !seen[i]
        {
            seen[i] = true;
            recorded[i] = parse_timestamp(&node.created_at).unwrap_or_default();
        }
    }
    let mut timelinks = vec![Vec::new(); count];
    for edge in &snapshot.edges {
        if edge.edge_type == "timelink"
            && let Some(&i) = index.by_id.get(&edge.src_id)
            && index.kinds[i] != Kind::Topic
        {
            timelinks[i].push((edge.dst_id.clone(), timepoint_of(edge)));
        }
    }
    let mut absent: HashMap<String, Vec<usize>> = HashMap::new();
    for (slot, edge) in index.edges.edges.iter().enumerate() {
        for end in [&edge.src, &edge.dst] {
            if let End::Absent(id) = end {
                absent.entry(id.clone()).or_default().push(slot);
            }
        }
    }
    LiveGraph {
        graph: snapshot.graph.clone(),
        recorded,
        filing: layout.nodes.iter().map(|n| n.filing).collect(),
        settled: layout.nodes.iter().map(|n| n.settled).collect(),
        ring: layout.ring,
        ranks: layout.ranks.clone(),
        unfiled: layout.unfiled_members,
        spread: layout.spread,
        timelines: snapshot.timelines.clone(),
        timelinks,
        metacontexts: snapshot.metacontexts.clone(),
        edge_ids: snapshot.edges.iter().map(|e| e.edge_id.clone()).collect(),
        edge_slots: index
            .edges
            .edges
            .iter()
            .enumerate()
            .map(|(slot, e)| (e.id.clone(), slot))
            .collect(),
        absent,
        waiting: HashMap::new(),
        waiting_count: 0,
    }
}

/// The timepoint a `timelink` edge names.
fn timepoint_of(edge: &EdgeView) -> Option<String> {
    edge.metadata
        .get("timepoint_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// What a snapshot read again over the picture needs to leave it where it
/// is: the layout's seed and the claim axis.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefetchSeed {
    pub layout: Seed,
    pub claim_axis: TimeAxis,
}

/// The seed of the picture drawn: every node's place and recorded height,
/// the ring, the ranks and both axes.
pub fn seed(
    index: &GraphIndex,
    places: &Places,
    time: &TimeModel,
    live: &LiveGraph,
) -> RefetchSeed {
    let places_by_id = (0..index.ids.len())
        .filter_map(|i| {
            let point = places.points.get(i)?;
            let height = time
                .recorded
                .spans
                .get(i)
                .copied()
                .flatten()
                .map_or(point.y, |s| s.lo);
            Some((
                index.ids[i].clone(),
                SeedPlace {
                    position: Vec3::new(point.x, height, point.z),
                    topic: index.filing[i].map(|t| index.ids[t].clone()),
                    filing: live.filing.get(i).copied().unwrap_or_default(),
                    settled: live.settled.get(i).copied().unwrap_or(true),
                },
            ))
        })
        .collect();
    RefetchSeed {
        layout: Seed {
            places: places_by_id,
            ring: live.ring,
            ranks: live.ranks.clone(),
            time: time.recorded.axis,
            spread: live.spread,
        },
        claim_axis: time.claim.axis,
    }
}

/// The picture, as one event changes it.
pub struct Picture<'a> {
    pub index: &'a mut GraphIndex,
    pub places: &'a mut Places,
    pub time: &'a mut TimeModel,
    pub live: &'a mut LiveGraph,
    /// The time mode drawn, whose heights the index and the places hold.
    pub mode: TimeMode,
    /// The wall clock, in microseconds since the Unix epoch: "now" where no
    /// timeline states one.
    pub wall_clock: i64,
}

/// What a batch of events asks of the scene and the bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Nodes whose disc is to be spawned: new, or back in the active set.
    pub spawn: BTreeSet<usize>,
    /// Nodes whose disc is to go: they left the active set.
    pub despawn: BTreeSet<usize>,
    /// Nodes already drawn whose disc moved on the map or changed its
    /// column.
    pub reshape: BTreeSet<usize>,
    /// The pickers' lists changed: a topic, a source or a metacontext.
    pub pickers: bool,
    /// Something about time changed: the counts are worked out again.
    pub time: bool,
}

/// What one event did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It changed the picture, as described.
    Applied(String),
    /// It changed nothing, for this reason.
    Unchanged(String),
    /// The picture cannot follow it: the snapshot must be read again.
    Refetch(String),
}

/// Applies one event to the picture.
pub fn apply(picture: &mut Picture, event: &Event, changes: &mut Changes) -> Outcome {
    match event {
        Event::NodeStored(node) => node_stored(picture, node, changes),
        Event::EdgeStored(edge) => edge_stored(picture, edge, changes),
        Event::NodeStatusChanged(change) => status_changed(picture, change, changes),
        Event::TimelineStored(timeline) => timeline_stored(picture, timeline, changes),
        Event::GraphSwitched { .. }
        | Event::RetrievalRecorded(_)
        | Event::ActRecorded(_)
        | Event::AdvisoryRaised(_)
        | Event::ReflectCounterUpdated(_)
        | Event::Pipeline(_)
        | Event::Other => Outcome::Unchanged("nothing drawn depends on it".into()),
        Event::Unreadable(error) => {
            Outcome::Refetch(format!("an event that does not read: {error}"))
        }
    }
}

/// Whether node `i` is drawn: indexed and active.
fn is_drawn(index: &GraphIndex, i: usize) -> bool {
    index.active.get(i).copied().unwrap_or(false)
}

/// The drawn node with this id.
fn drawn(index: &GraphIndex, id: &str) -> Option<usize> {
    index.by_id.get(id).copied().filter(|&i| is_drawn(index, i))
}

fn confidence_of(node: &NodeView) -> Option<f32> {
    node.confidence
        .filter(|c| c.is_finite())
        .map(|c| c.clamp(0.0, 1.0) as f32)
}

fn short(id: &str) -> &str {
    crate::pickers::short_id(id)
}

fn node_stored(p: &mut Picture, node: &NodeView, changes: &mut Changes) -> Outcome {
    let Some(kind) = kind_of(node.node_type) else {
        return Outcome::Unchanged("a kind this client does not draw".into());
    };
    let active = node.status == "active";
    if let Some(&i) = p.index.by_id.get(&node.node_id) {
        let mut what = Vec::new();
        if p.index.names[i] != node.content {
            p.index.names[i].clone_from(&node.content);
            if kind == Kind::Topic {
                if let Some(region) = p.places.topics.get_mut(&i) {
                    region.name.clone_from(&node.content);
                }
                changes.pickers = true;
            }
            what.push("its content");
        }
        let confidence = confidence_of(node);
        if p.index.confidence[i] != confidence {
            p.index.confidence[i] = confidence;
            what.push("its confidence");
        }
        if active != p.index.active[i] {
            if active {
                reactivate(p, i, changes);
            } else {
                deactivate(p, i, changes);
            }
            what.push("its status");
        }
        if let Some(view) = p.index.views.get_mut(i)
            && view != node
        {
            if what.is_empty() {
                what.push("its fields");
            }
            view.clone_from(node);
        }
        return if what.is_empty() {
            Outcome::Unchanged("drawn already, as it is".into())
        } else {
            Outcome::Applied(format!("updated {}", what.join(" and ")))
        };
    }
    if !active {
        return Outcome::Unchanged(format!("{}, so not drawn", node.status));
    }
    let Some(created) = parse_timestamp(&node.created_at) else {
        return Outcome::Unchanged("its recorded time does not read".into());
    };
    let i = add_node(p, node, kind, created, changes);
    resolve_absent(p, i);
    attach_waiting(p, i, changes);
    let place = match (kind, p.index.filing[i]) {
        (Kind::Topic, _) => format!("a topic, in slot {}", p.live.ring.next - 1),
        (_, Some(topic)) => format!("beside {:?}", p.index.names[topic]),
        (_, None) => "in the unfiled region until its topic arrives".to_owned(),
    };
    Outcome::Applied(format!("placed {place}"))
}

/// Adds a node to the picture at its place, and notes its disc to spawn.
fn add_node(
    p: &mut Picture,
    node: &NodeView,
    kind: Kind,
    created: i64,
    changes: &mut Changes,
) -> usize {
    let i = p.index.ids.len();
    p.time.recorded.axis = extend_axis(&p.time.recorded.axis, created);
    let y = time_y(&p.time.recorded.axis, created);
    let (ground, filing, settled) = match kind {
        Kind::Topic => {
            let ground = ring_point(&p.live.ring, p.live.ring.next);
            p.live.ring.next += 1;
            (ground, Filing::Filed, true)
        }
        Kind::Fact | Kind::Inference => {
            let rank = p.live.unfiled;
            p.live.unfiled += 1;
            let ground = unfiled_centre(&p.live.ring)
                + claim_offset_spread(&node.node_id, rank, p.live.spread.0);
            (ground, Filing::Unfiled, false)
        }
    };
    let index = &mut *p.index;
    index.ids.push(node.node_id.clone());
    index.names.push(node.content.clone());
    if index.views.len() == i {
        index.views.push(node.clone());
    }
    index.by_id.insert(node.node_id.clone(), i);
    index.kinds.push(kind);
    index.active.push(true);
    index.heights.push(y);
    index.spans.push(Some((y, y)));
    index.metacontexts.push(Vec::new());
    index.tags.push(Vec::new());
    index.filing.push(None);
    index.subtopics.push(Vec::new());
    index.tagged_topics.push(Vec::new());
    index.sources.push(Vec::new());
    index.degrees.push(0);
    index.confidence.push(confidence_of(node));
    index.edges.claim.push(Vec::new());
    index.edges.succession.push(Vec::new());

    let keep_spans = p.places.spans.len() == p.places.points.len();
    p.places.points.push(Vec3::new(ground.x, y, ground.y));
    if keep_spans {
        p.places.spans.push(Some((y, y)));
    }
    if kind == Kind::Topic {
        let name = if node.content.trim().is_empty() {
            node.node_id.clone()
        } else {
            node.content.clone()
        };
        p.places.topics.insert(
            i,
            Region {
                id: node.node_id.clone(),
                name,
                members: vec![i],
            },
        );
        changes.pickers = true;
    }

    p.time.recorded.spans.push(Some(disc_at(y)));
    p.time.claim.spans.push(None);
    p.time.claim_places.push(None);
    p.time.linked.push(Vec::new());
    p.time.validity.push(Vec::new());
    p.live.recorded.push(created);
    p.live.filing.push(filing);
    p.live.settled.push(settled);
    p.live.timelinks.push(Vec::new());
    changes.spawn.insert(i);
    place_in_claim_time(p, i, changes);
    changes.time = true;
    i
}

/// Places node `i` in claim time again, from its linked timepoints, and
/// moves it there in the index and the places when claim time is drawn. A
/// disc at a recorded time after the claim axis's top grows the axis upward
/// at its own scale, and the nodes clipped at the old top are placed again
/// on it; a timepoint beyond the axis is drawn clipped and grows nothing.
fn place_in_claim_time(p: &mut Picture, i: usize, changes: &mut Changes) {
    let placement = claim_placement(p.index.kinds[i], p.live.recorded[i], &p.time.linked[i]);
    p.time.claim_places[i] = placement;
    if let Some(Placement::Recorded(t)) = placement {
        let grown = extend_axis(&p.time.claim.axis, t);
        if grown != p.time.claim.axis {
            p.time.claim.axis = grown;
            let clipped_above: Vec<usize> = (0..p.time.claim.spans.len())
                .filter(|&j| j != i)
                .filter(|&j| p.time.claim.spans[j].is_some_and(|s| s.clipped.above > 0))
                .collect();
            for j in clipped_above {
                respan(p, j, changes);
            }
        }
    }
    respan(p, i, changes);
}

/// Draws node `i` again on the claim axis as it now is, from its place in
/// claim time, noting its disc to reshape where its span changed.
fn respan(p: &mut Picture, i: usize, changes: &mut Changes) {
    let span = p.time.claim_places[i].map(|place| clipped_span(&place, &p.time.claim.axis));
    if p.time.claim.spans[i] != span {
        p.time.claim.spans[i] = span;
        changes.time = true;
        if p.mode == TimeMode::Claim && !changes.spawn.contains(&i) {
            changes.reshape.insert(i);
        }
    }
    place_in_mode(p, i);
}

/// Writes node `i`'s height and span in the time mode drawn into the index
/// and the places, as `time_mode::apply_mode` does for every node.
fn place_in_mode(p: &mut Picture, i: usize) {
    let times = time_axis::times(p.time, p.mode);
    let span = times.spans[i];
    let recorded = p.time.recorded.spans[i];
    let height = span.or(recorded).map_or(p.index.heights[i], |s| centre(&s));
    p.index.heights[i] = height;
    p.index.spans[i] = span.map(|s| (s.lo, s.hi));
    p.index.time = times.axis;
    p.places.points[i].y = height;
    if let Some(slot) = p.places.spans.get_mut(i) {
        *slot = p.index.spans[i];
    }
}

/// Takes node `i` out of the active set: it is no longer drawn, and the
/// edges drawn to it keep it as an id, so that a connector to it ends in a
/// marker.
fn deactivate(p: &mut Picture, i: usize, changes: &mut Changes) {
    p.index.active[i] = false;
    let id = p.index.ids[i].clone();
    let lists = [
        std::mem::take(&mut p.index.edges.claim[i]),
        std::mem::take(&mut p.index.edges.succession[i]),
    ];
    for slot in lists.into_iter().flatten() {
        p.index.degrees[i] = p.index.degrees[i].saturating_sub(1);
        let edge = &mut p.index.edges.edges[slot];
        for end in [&mut edge.src, &mut edge.dst] {
            if *end == End::Drawn(i) {
                *end = End::Absent(id.clone());
            }
        }
        let absent = p.live.absent.entry(id.clone()).or_default();
        if !absent.contains(&slot) {
            absent.push(slot);
        }
    }
    changes.spawn.remove(&i);
    changes.despawn.insert(i);
}

/// Brings node `i` back into the active set, where it was.
fn reactivate(p: &mut Picture, i: usize, changes: &mut Changes) {
    p.index.active[i] = true;
    resolve_absent(p, i);
    attach_waiting(p, i, changes);
    changes.despawn.remove(&i);
    changes.spawn.insert(i);
}

/// Joins node `i` to the edges of the index that name it as an end not
/// drawn.
fn resolve_absent(p: &mut Picture, i: usize) {
    let id = p.index.ids[i].clone();
    for slot in p.live.absent.remove(&id).unwrap_or_default() {
        resolve_end(p, slot, i);
    }
}

/// Makes the end of edge `slot` that names node `i` by id a drawn end,
/// lists the edge under `i` and counts it in `i`'s degree. An end drawn
/// already is left as it is.
fn resolve_end(p: &mut Picture, slot: usize, i: usize) {
    let id = p.index.ids[i].clone();
    let edges = &mut p.index.edges;
    let edge = &mut edges.edges[slot];
    let mut joined = false;
    for end in [&mut edge.src, &mut edge.dst] {
        if *end == End::Absent(id.clone()) {
            *end = End::Drawn(i);
            joined = true;
        }
    }
    let list = match edge.class {
        EdgeClass::Claim(_) => &mut edges.claim[i],
        _ => &mut edges.succession[i],
    };
    if joined && !list.contains(&slot) {
        list.push(slot);
        p.index.degrees[i] = p.index.degrees[i].saturating_add(1);
    }
}

/// Attaches the edges waiting for node `i`.
fn attach_waiting(p: &mut Picture, i: usize, changes: &mut Changes) {
    let id = p.index.ids[i].clone();
    let Some(waiting) = p.live.waiting.remove(&id) else {
        return;
    };
    p.live.waiting_count = p.live.waiting_count.saturating_sub(waiting.len());
    for edge in waiting {
        attach(p, &edge, Some(i), changes);
    }
}

/// Keeps an edge waiting for the node `id`.
fn wait_for(p: &mut Picture, id: &str, edge: &EdgeView) {
    if p.live.waiting_count >= MAX_WAITING {
        return;
    }
    p.live.waiting_count += 1;
    p.live
        .waiting
        .entry(id.to_owned())
        .or_default()
        .push(edge.clone());
}

fn edge_stored(p: &mut Picture, edge: &EdgeView, changes: &mut Changes) -> Outcome {
    if !p.live.edge_ids.insert(edge.edge_id.clone()) {
        return Outcome::Unchanged("seen already".into());
    }
    let src = drawn(p.index, &edge.src_id);
    let dst = drawn(p.index, &edge.dst_id).filter(|&d| Some(d) != src);
    for end in [src, dst].into_iter().flatten() {
        p.index.degrees[end] = p.index.degrees[end].saturating_add(1);
    }
    Outcome::Applied(attach(p, edge, None, changes))
}

/// Applies an edge's effects. On its arrival (`newly` is `None`) every
/// effect whose ends are drawn is applied and the edge waits for each end
/// it still needs; when a node it waited for arrives (`newly` names it),
/// the effects that end completes are applied. Says what it did.
fn attach(p: &mut Picture, edge: &EdgeView, newly: Option<usize>, changes: &mut Changes) -> String {
    let src = drawn(p.index, &edge.src_id);
    let dst = drawn(p.index, &edge.dst_id);
    let arrived = |end: Option<usize>| newly.is_none() || (end.is_some() && end == newly);
    let class = classify(&edge.edge_type);
    // The node that arrived counts the edge in its degree here, except an
    // indexed line's end, which `resolve_end` counts as it joins it.
    if let Some(i) = newly {
        let indexed = matches!(class, EdgeClass::Claim(_) | EdgeClass::Succession(_))
            && p.live.edge_slots.contains_key(&edge.edge_id);
        if !indexed {
            p.index.degrees[i] = p.index.degrees[i].saturating_add(1);
        }
    }
    match class {
        EdgeClass::Membership => {
            let needs_dst_node = matches!(
                edge.edge_type.as_str(),
                "tagged_with_topic" | "subtopic_of" | "extracted_under_topic" | "abstracts"
            );
            let needs_src_node =
                !matches!(edge.edge_type.as_str(), "contains" | "implies" | "about");
            if newly.is_none() {
                if needs_src_node && src.is_none() {
                    wait_for(p, &edge.src_id, edge);
                }
                let waits_on_src = needs_src_node && src.is_none() && edge.dst_id == edge.src_id;
                if (needs_dst_node || !needs_src_node) && dst.is_none() && !waits_on_src {
                    wait_for(p, &edge.dst_id, edge);
                }
            }
            match (src, dst) {
                (Some(s), _) if !needs_dst_node && needs_src_node && arrived(src) => {
                    membership_from(p, edge, s, changes)
                }
                (Some(s), Some(d)) if needs_dst_node && (arrived(src) || arrived(dst)) => {
                    membership_between(p, edge, s, d, changes)
                }
                _ if !needs_src_node => "a segment's edge: the node's degree".into(),
                _ => format!(
                    "waiting for {}",
                    if src.is_none() {
                        &edge.src_id
                    } else {
                        &edge.dst_id
                    }
                ),
            }
        }
        EdgeClass::Timelink => match src {
            Some(s) if arrived(src) && p.index.kinds[s] != Kind::Topic => {
                p.live.timelinks[s].push((edge.dst_id.clone(), timepoint_of(edge)));
                relink(p, s);
                place_in_claim_time(p, s, changes);
                "placed its claim in claim time".into()
            }
            Some(_) => "a topic's timelink places nothing".into(),
            None => {
                if newly.is_none() {
                    wait_for(p, &edge.src_id, edge);
                }
                format!("waiting for {}", short(&edge.src_id))
            }
        },
        class @ (EdgeClass::Claim(_) | EdgeClass::Succession(_)) => {
            if let Some(&slot) = p.live.edge_slots.get(&edge.edge_id) {
                if let Some(i) = newly {
                    resolve_end(p, slot, i);
                }
                return "joined to the node it waited for".into();
            }
            if src.is_none() && dst.is_none() {
                wait_for(p, &edge.src_id, edge);
                if edge.dst_id != edge.src_id {
                    wait_for(p, &edge.dst_id, edge);
                }
                return "waiting for either end".into();
            }
            index_line(p, edge, class, src, dst);
            match class {
                EdgeClass::Succession(_) => "a connector, drawn at once".into(),
                _ => "a line, drawn on hover".into(),
            }
        }
    }
}

/// A membership edge that needs only its source to be drawn: a metacontext
/// or a source document.
fn membership_from(p: &mut Picture, edge: &EdgeView, s: usize, changes: &mut Changes) -> String {
    match edge.edge_type.as_str() {
        "has_metacontext" => {
            if insert_sorted(&mut p.index.metacontexts[s], edge.dst_id.clone()) {
                let name = p
                    .live
                    .metacontexts
                    .iter()
                    .find(|m| m.metacontext_id == edge.dst_id)
                    .map_or_else(|| edge.dst_id.clone(), |m| m.content.clone());
                add_member(&mut p.places.metacontexts, &edge.dst_id, &name, s);
                changes.pickers = true;
            }
            "in a metacontext, for the filter".into()
        }
        "sourced_from" => {
            if insert_sorted(&mut p.index.sources[s], edge.dst_id.clone()) {
                changes.pickers = true;
            }
            p.time.validity[s].extend(
                edge.validity
                    .iter()
                    .map(|v| read_validity(&edge.edge_id, &edge.dst_id, v)),
            );
            changes.time |= !edge.validity.is_empty();
            "read from a document, for the filter".into()
        }
        _ => "nothing drawn depends on it".into(),
    }
}

/// A membership edge between two drawn nodes: a tag, a subtopic or a
/// filing edge, the last two of which may give a provisional claim its
/// place.
fn membership_between(
    p: &mut Picture,
    edge: &EdgeView,
    s: usize,
    d: usize,
    changes: &mut Changes,
) -> String {
    let (src_kind, dst_kind) = (p.index.kinds[s], p.index.kinds[d]);
    if dst_kind != Kind::Topic {
        return "names no topic, so files nothing".into();
    }
    match (edge.edge_type.as_str(), src_kind) {
        ("tagged_with_topic", _) => {
            insert_sorted(&mut p.index.tags[s], d);
            if src_kind == Kind::Topic {
                if s != d {
                    insert_sorted(&mut p.index.tagged_topics[d], s);
                }
                return "a tag between topics, for the filter".into();
            }
            if !p.live.settled[s] && p.live.filing[s] == Filing::Unfiled {
                move_claim(p, s, d, Filing::Tagged, changes);
                return format!("moved its claim beside {:?}, by its tag", p.index.names[d]);
            }
            "a tag, for the filter".into()
        }
        ("subtopic_of", Kind::Topic) if s != d => {
            insert_sorted(&mut p.index.subtopics[d], s);
            "a subtopic, for the filter".into()
        }
        ("extracted_under_topic", Kind::Fact) | ("abstracts", Kind::Inference) => {
            if !p.live.settled[s] && p.live.filing[s] < Filing::Filed {
                move_claim(p, s, d, Filing::Filed, changes);
                return format!("moved its claim beside {:?}", p.index.names[d]);
            }
            "a filing edge for a claim placed already".into()
        }
        _ => "files nothing".into(),
    }
}

/// Moves a claim placed provisionally to topic `t`, at the topic's next
/// rank. Only a claim whose place is not final moves, and a filing edge
/// makes it final.
fn move_claim(p: &mut Picture, i: usize, t: usize, filing: Filing, changes: &mut Changes) {
    let old = p.index.filing[i];
    if old == Some(t) {
        p.live.filing[i] = filing;
        p.live.settled[i] = filing == Filing::Filed;
        return;
    }
    match old {
        None => p.live.unfiled = p.live.unfiled.saturating_sub(1),
        Some(old) => {
            if let Some(region) = p.places.topics.get_mut(&old) {
                region.members.retain(|&m| m != i);
            }
        }
    }
    let rank = p.live.ranks.entry(p.index.ids[t].clone()).or_default();
    *rank += 1;
    let topic = p.places.points[t];
    let ground = Vec2::new(topic.x, topic.z)
        + claim_offset_spread(&p.index.ids[i], *rank - 1, p.live.spread.0);
    p.places.points[i].x = ground.x;
    p.places.points[i].z = ground.y;
    p.index.filing[i] = Some(t);
    p.live.filing[i] = filing;
    p.live.settled[i] = filing == Filing::Filed;
    if let Some(region) = p.places.topics.get_mut(&t) {
        region.members.push(i);
    }
    if !changes.spawn.contains(&i) {
        changes.reshape.insert(i);
    }
}

/// Puts a claim-to-claim or succession edge into the edge index, listed
/// under each drawn end, with each other end kept by id.
fn index_line(
    p: &mut Picture,
    edge: &EdgeView,
    class: EdgeClass,
    src: Option<usize>,
    dst: Option<usize>,
) {
    let slot = p.index.edges.edges.len();
    let end =
        |node: Option<usize>, id: &str| node.map_or_else(|| End::Absent(id.to_owned()), End::Drawn);
    let (src_end, dst_end) = (end(src, &edge.src_id), end(dst, &edge.dst_id));
    for absent in [&src_end, &dst_end] {
        if let End::Absent(id) = absent {
            let list = p.live.absent.entry(id.clone()).or_default();
            if !list.contains(&slot) {
                list.push(slot);
            }
        }
    }
    let lists = match class {
        EdgeClass::Claim(_) => &mut p.index.edges.claim,
        _ => &mut p.index.edges.succession,
    };
    for node in [src, dst.filter(|&d| Some(d) != src)].into_iter().flatten() {
        lists[node].push(slot);
    }
    p.index.edges.edges.push(Edge {
        id: edge.edge_id.clone(),
        edge_type: edge.edge_type.clone(),
        class,
        src: src_end,
        dst: dst_end,
        created: parse_timestamp(&edge.created_at),
    });
    p.live.edge_slots.insert(edge.edge_id.clone(), slot);
}

/// Reads node `i`'s linked timepoints again from its timelinks and the
/// timelines held.
fn relink(p: &mut Picture, i: usize) {
    let mut linked = Vec::new();
    for (timeline, timepoint) in &p.live.timelinks[i] {
        let entry = time_axis::link(&p.live.timelines, timeline, timepoint.as_deref());
        add_link(&mut linked, entry, timepoint.is_some());
    }
    linked.sort_by(linked_order);
    p.time.linked[i] = linked;
}

fn status_changed(p: &mut Picture, change: &StatusChange, changes: &mut Changes) -> Outcome {
    let active = change.new_status == "active";
    match p.index.by_id.get(&change.node_id).copied() {
        Some(i) if p.index.active[i] == active => {
            Outcome::Unchanged(format!("{} already", change.new_status))
        }
        Some(i) if active => {
            reactivate(p, i, changes);
            Outcome::Applied("back in the active set, drawn where it was".into())
        }
        Some(i) => {
            deactivate(p, i, changes);
            Outcome::Applied(format!(
                "{}: no longer drawn; its connectors end in a marker",
                change.new_status
            ))
        }
        None if active => Outcome::Refetch(format!(
            "node {} came back into the active set and was never drawn",
            short(&change.node_id)
        )),
        None => Outcome::Unchanged("not drawn".into()),
    }
}

fn timeline_stored(p: &mut Picture, timeline: &TimelineView, changes: &mut Changes) -> Outcome {
    match p
        .live
        .timelines
        .iter_mut()
        .find(|t| t.timeline_id == timeline.timeline_id)
    {
        Some(held) => *held = timeline.clone(),
        None => p.live.timelines.push(timeline.clone()),
    }
    let (now, source) = time_axis::now_of(&p.live.timelines, p.wall_clock);
    p.time.now = now;
    p.time.now_source = source;
    let linked: Vec<usize> = (0..p.index.ids.len())
        .filter(|&i| {
            p.live.timelinks[i]
                .iter()
                .any(|(t, _)| *t == timeline.timeline_id)
        })
        .collect();
    for &i in &linked {
        relink(p, i);
        place_in_claim_time(p, i, changes);
    }
    changes.time = true;
    Outcome::Applied(format!(
        "replaced timeline {:?}; {} claims placed in claim time again",
        timeline.name,
        linked.len()
    ))
}

/// Adds `value` to a sorted list once; whether it was new.
fn insert_sorted<T: Ord>(list: &mut Vec<T>, value: T) -> bool {
    match list.binary_search(&value) {
        Ok(_) => false,
        Err(at) => {
            list.insert(at, value);
            true
        }
    }
}

/// Adds node `i` to a metacontext's region, making the region where it is
/// the first, in id order.
fn add_member(regions: &mut Vec<Region>, id: &str, name: &str, i: usize) {
    match regions.binary_search_by(|r| r.id.as_str().cmp(id)) {
        Ok(at) => {
            if !regions[at].members.contains(&i) {
                regions[at].members.push(i);
            }
        }
        Err(at) => regions.insert(
            at,
            Region {
                id: id.to_owned(),
                name: name.to_owned(),
                members: vec![i],
            },
        ),
    }
}

/// What a batch ends with: the claim-time counts where time changed, "now"
/// on the wall clock where no timeline states one, and the "now" layer of
/// the time mode drawn.
pub fn finish(p: &mut Picture, changes: &Changes) {
    if changes.time {
        p.time.counts = time_axis::counts(&p.live.timelines, &p.time.linked, &p.time.claim);
    }
    if p.time.now_source == NowSource::WallClock {
        p.time.now = p.wall_clock;
    }
    let axis = time_axis::times(p.time, p.mode).axis;
    p.index.time = axis;
    let day = presets::day_height(&axis);
    p.places.now = match p.mode {
        TimeMode::Recorded => NowLayer::Latest(day),
        TimeMode::Claim => NowLayer::At {
            height: time_axis::now_height(p.time, p.mode),
            day,
        },
    };
}

/// How many nodes are drawn, and how many edges are drawn as lines or
/// connectors: the counts `--log-events` prints after each event.
pub fn counts(index: &GraphIndex) -> (usize, usize) {
    (
        index.active.iter().filter(|&&a| a).count(),
        index.edges.edges.len(),
    )
}

/// Does to the scene what a batch asks: despawns the discs of nodes that
/// left the active set, reshapes the discs that moved, and spawns the new
/// ones, all on the shared meshes and material. The picture's resources
/// are in the world again by now.
pub fn realize(world: &mut World, changes: &Changes) {
    if !changes.despawn.is_empty() || !changes.reshape.is_empty() {
        let mut discs = world.query::<(Entity, &Disc)>();
        let by_index: HashMap<usize, Entity> = discs
            .iter(world)
            .map(|(entity, disc)| (disc.index, entity))
            .collect();
        for i in &changes.despawn {
            if let Some(&entity) = by_index.get(i) {
                world.despawn(entity);
            }
        }
        let assets = scene::disc_assets(world);
        for i in changes.reshape.difference(&changes.despawn) {
            let Some(&entity) = by_index.get(i) else {
                continue;
            };
            let Some((place, span)) = disc_place(world, *i) else {
                continue;
            };
            let (y, height, profile) = scene::shape(span, place.y);
            if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                transform.translation = place.with_y(y);
                transform.scale.y = height;
            }
            let wanted = scene::profile_mesh(&assets, profile);
            if let Some(mut mesh) = world.get_mut::<Mesh3d>(entity)
                && mesh.0 != wanted
            {
                mesh.0 = wanted;
            }
        }
    }
    let new: Vec<_> = changes
        .spawn
        .iter()
        .filter_map(|&i| {
            let kind = *world.resource::<GraphIndex>().kinds.get(i)?;
            let (place, span) = disc_place(world, i)?;
            Some((i, kind, place, span))
        })
        .collect();
    if !new.is_empty() {
        let assets: DiscAssets = scene::disc_assets(world);
        scene::spawn_nodes(world, &assets, new);
    }
}

/// Where node `i`'s disc goes in the time mode drawn: its place, and its
/// span on the axis.
fn disc_place(world: &World, i: usize) -> Option<(Vec3, Option<Span>)> {
    let place = *world.resource::<Places>().points.get(i)?;
    let mode = world
        .get_resource::<TimeMode>()
        .copied()
        .unwrap_or_default();
    let span = time_axis::times(world.resource::<TimeModel>(), mode)
        .spans
        .get(i)
        .copied()
        .flatten();
    Some((place, span))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::edges::{self, Place, connector_segments};
    use crate::filter::{self, Filter};
    use crate::graph_index::tests::{edge, node};
    use crate::hub::{NodeType, TimepointView};
    use crate::layout::{CLAIM_CLEARANCE, RING_GAP, claim_offset};
    use crate::reload::{Prepared, prepare};
    use crate::time_axis::Profile;

    /// A picture's four resources, owned.
    pub(crate) struct Owned {
        pub index: GraphIndex,
        pub places: Places,
        pub time: TimeModel,
        pub live: LiveGraph,
        pub mode: TimeMode,
    }

    pub(crate) fn owned(prepared: Prepared) -> Owned {
        Owned {
            index: prepared.index,
            places: prepared.places,
            time: prepared.time,
            live: prepared.live,
            mode: TimeMode::Recorded,
        }
    }

    /// Applies events to an owned picture, as one batch.
    pub(crate) fn run(owned: &mut Owned, events: &[Event]) -> (Vec<Outcome>, Changes) {
        let mut changes = Changes::default();
        let mut picture = Picture {
            index: &mut owned.index,
            places: &mut owned.places,
            time: &mut owned.time,
            live: &mut owned.live,
            mode: owned.mode,
            wall_clock: parse_timestamp("2026-10-21T00:00:00Z").unwrap(),
        };
        let outcomes = events
            .iter()
            .map(|event| apply(&mut picture, event, &mut changes))
            .collect();
        finish(&mut picture, &changes);
        (outcomes, changes)
    }

    pub(crate) fn stored(view: NodeView) -> Event {
        Event::NodeStored(Box::new(view))
    }

    pub(crate) fn linked(view: EdgeView) -> Event {
        Event::EdgeStored(Box::new(view))
    }

    fn at(day: u32, hour: u32) -> String {
        format!("2026-09-{day:02}T{hour:02}:00:00Z")
    }

    /// Two topics and three claims, recorded from 1 to 5 September: `a`
    /// with claims `fa` and `ia`, `b` with `fb`.
    fn small() -> Snapshot {
        Snapshot {
            schema_version: Some(1),
            graph: "test".into(),
            nodes: vec![
                node("a", NodeType::Topic, 1),
                node("b", NodeType::Topic, 2),
                node("fa", NodeType::Fact, 3),
                node("ia", NodeType::Inference, 4),
                node("fb", NodeType::Fact, 5),
            ],
            edges: vec![
                edge("fa", "a", "extracted_under_topic"),
                edge("ia", "a", "abstracts"),
                edge("fb", "b", "extracted_under_topic"),
                edge("fa", "ia", "supports"),
            ],
            timelines: Vec::new(),
            metacontexts: Vec::new(),
        }
    }

    fn picture() -> Owned {
        owned(prepare(&small()).unwrap())
    }

    fn timed(id: &str, node_type: NodeType, created: &str) -> NodeView {
        NodeView {
            created_at: created.into(),
            ..node(id, node_type, 1)
        }
    }

    fn identified(src: &str, dst: &str, edge_type: &str, id: &str) -> EdgeView {
        EdgeView {
            edge_id: id.into(),
            ..edge(src, dst, edge_type)
        }
    }

    #[test]
    fn a_new_claim_and_a_new_topic_are_placed_without_moving_anything() {
        let mut picture = picture();
        let before = picture.places.points.clone();
        let heights = picture.index.heights.clone();
        let (outcomes, changes) = run(
            &mut picture,
            &[
                stored(timed("c", NodeType::Topic, &at(5, 6))),
                stored(timed("fc", NodeType::Fact, &at(5, 7))),
                linked(identified("fc", "c", "extracted_under_topic", "e-fc")),
                stored(timed("fa2", NodeType::Fact, &at(5, 8))),
                linked(identified("fa2", "a", "extracted_under_topic", "e-fa2")),
            ],
        );
        assert!(
            outcomes.iter().all(|o| matches!(o, Outcome::Applied(_))),
            "{outcomes:?}"
        );
        assert_eq!(
            &picture.places.points[..before.len()],
            &before[..],
            "nothing that was drawn moved"
        );
        assert_eq!(&picture.index.heights[..heights.len()], &heights[..]);
        let index = &picture.index;
        let (c, fc, fa2, a) = (
            index.by_id["c"],
            index.by_id["fc"],
            index.by_id["fa2"],
            index.by_id["a"],
        );
        assert_eq!(changes.spawn, [c, fc, fa2].into());
        assert!(changes.reshape.is_empty(), "moved before it was drawn");

        // The new topic continues the circle's step on the ring inside it.
        let ring = picture.live.ring;
        assert_eq!((ring.count, ring.next), (2, 3));
        let ground = |i: usize| Vec2::new(picture.places.points[i].x, picture.places.points[i].z);
        assert!((ground(c).length() - (ring.radius - RING_GAP)).abs() < 1e-3);
        assert!(ground(c).distance(ring_point(&ring, 2)) < 1e-4);
        // Its claim stands beside it, at the first rank; the new claim on
        // `a` takes the rank after `a`'s two.
        assert!((ground(fc).distance(ground(c)) - CLAIM_CLEARANCE).abs() < 1e-3);
        assert!((ground(fa2).distance(ground(a)) - claim_offset("fa2", 2).length()).abs() < 1e-3);
        assert_eq!(index.filing[fc], Some(c));
        assert!(
            picture.live.settled[fc],
            "a filing edge makes the place final"
        );
        assert_eq!(picture.places.topics[&c].members, [c, fc]);
        assert!(changes.pickers, "a new topic for the topic picker");
    }

    #[test]
    fn a_node_stored_again_changes_in_place_every_field_the_panel_shows() {
        let mut picture = picture();
        let fa = picture.index.by_id["fa"];
        let before = picture.places.points.clone();
        let mut again = picture.index.views[fa].clone();
        again.description = "A note added later.".into();
        again.retrieved_at = Some("2026-10-20T09:00:00Z".into());
        again
            .metadata
            .insert("confidence_basis".into(), "read twice".into());
        let (outcomes, changes) = run(&mut picture, &[stored(again.clone())]);
        assert_eq!(outcomes, [Outcome::Applied("updated its fields".into())]);
        assert_eq!(picture.index.views[fa], again);
        assert_eq!(picture.places.points, before, "nothing moved");
        assert!(changes.spawn.is_empty() && changes.reshape.is_empty());
        let (outcomes, _) = run(&mut picture, &[stored(again)]);
        assert_eq!(
            outcomes,
            [Outcome::Unchanged("drawn already, as it is".into())]
        );

        let (_, _) = run(
            &mut picture,
            &[stored(timed("late", NodeType::Fact, &at(5, 9)))],
        );
        let late = picture.index.by_id["late"];
        assert_eq!(
            picture.index.views[late].node_id, "late",
            "a new node's view"
        );
        assert_eq!(picture.index.views.len(), picture.index.ids.len());
    }

    #[test]
    fn a_node_recorded_after_the_top_grows_the_axis_at_its_scale() {
        let mut picture = picture();
        let axis = picture.time.recorded.axis;
        let scale = axis.height as f64 / axis.span as f64;
        let latest = picture.index.by_id["fb"];
        assert_eq!(picture.index.heights[latest], axis.height);
        run(
            &mut picture,
            &[stored(timed("late", NodeType::Topic, &at(9, 12)))],
        );
        let grown = picture.time.recorded.axis;
        assert_eq!(grown.start, axis.start);
        assert!(grown.height > axis.height);
        assert!(((grown.height as f64 / grown.span as f64) - scale).abs() / scale < 1e-6);
        assert_eq!(picture.index.time, grown, "the axis drawn follows");
        let late = picture.index.by_id["late"];
        assert!((picture.index.heights[late] - grown.height).abs() < 1e-3);
        assert_eq!(
            picture.index.heights[latest], axis.height,
            "the latest node before stays at its height"
        );
        assert_eq!(
            presets::day_height(&grown),
            presets::day_height(&axis),
            "a day is as high as it was"
        );
        assert_eq!(
            picture.time.now,
            parse_timestamp("2026-10-21T00:00:00Z").unwrap(),
            "now follows the wall clock"
        );
        // One recorded earlier changes no axis.
        run(
            &mut picture,
            &[stored(timed("early", NodeType::Topic, &at(2, 12)))],
        );
        assert_eq!(picture.time.recorded.axis, grown);
    }

    #[test]
    fn a_claim_whose_filing_edge_comes_later_moves_once_to_its_topic() {
        let mut picture = picture();
        let (_, first) = run(
            &mut picture,
            &[stored(timed("late", NodeType::Fact, &at(5, 9)))],
        );
        let late = picture.index.by_id["late"];
        assert_eq!(first.spawn, [late].into());
        let unfiled = unfiled_centre(&picture.live.ring);
        let ground = |o: &Owned| Vec2::new(o.places.points[late].x, o.places.points[late].z);
        assert!(ground(&picture).distance(unfiled) < CLAIM_CLEARANCE + 1e-3 + 1.5);
        assert_eq!(picture.index.filing[late], None);
        assert!(!picture.live.settled[late]);

        // The tag comes first: it moves to the tag's topic, provisionally.
        let (outcomes, changes) = run(
            &mut picture,
            &[linked(identified(
                "late",
                "b",
                "tagged_with_topic",
                "e-tag",
            ))],
        );
        assert!(matches!(&outcomes[0], Outcome::Applied(w) if w.contains("by its tag")));
        assert_eq!(changes.reshape, [late].into(), "drawn already, so it moves");
        let b = picture.index.by_id["b"];
        assert_eq!(picture.index.filing[late], Some(b));
        assert_eq!(picture.index.tags[late], [b]);

        // Then the filing edge: to its filing topic, for good.
        let a = picture.index.by_id["a"];
        run(
            &mut picture,
            &[linked(identified(
                "late",
                "a",
                "extracted_under_topic",
                "e-file",
            ))],
        );
        assert_eq!(picture.index.filing[late], Some(a));
        assert!(picture.live.settled[late]);
        let at_a = Vec2::new(picture.places.points[a].x, picture.places.points[a].z);
        assert!((ground(&picture).distance(at_a) - claim_offset("late", 2).length()).abs() < 1e-3);
        assert!(!picture.places.topics[&b].members.contains(&late));
        assert!(picture.places.topics[&a].members.contains(&late));

        // A second filing edge moves it no more, and nor does a tag.
        let settled = ground(&picture);
        let (_, changes) = run(
            &mut picture,
            &[
                linked(identified("late", "b", "extracted_under_topic", "e-again")),
                linked(identified("late", "b", "tagged_with_topic", "e-tag2")),
            ],
        );
        assert!(changes.reshape.is_empty());
        assert_eq!(ground(&picture), settled);
        // A claim of the snapshot never moves.
        let fb = picture.index.by_id["fb"];
        let before = picture.places.points[fb];
        run(
            &mut picture,
            &[linked(identified(
                "fb",
                "a",
                "extracted_under_topic",
                "e-fb",
            ))],
        );
        assert_eq!(picture.places.points[fb], before);
    }

    #[test]
    fn an_edge_that_comes_before_its_node_waits_for_it() {
        let mut picture = picture();
        let (outcomes, _) = run(
            &mut picture,
            &[
                linked(identified("early", "a", "extracted_under_topic", "e1")),
                linked(identified("early", "the-real", "has_metacontext", "e2")),
                linked(identified("early", "fa", "supports", "e3")),
            ],
        );
        assert!(outcomes.iter().all(|o| matches!(o, Outcome::Applied(w) if w.contains("waiting") || w.contains("drawn on hover"))), "{outcomes:?}");
        let fa = picture.index.by_id["fa"];
        let slot = picture.live.edge_slots["e3"];
        assert_eq!(
            picture.index.edges.edges[slot].src,
            End::Absent("early".into())
        );
        assert_eq!(picture.live.waiting["early"].len(), 2);

        run(
            &mut picture,
            &[stored(timed("early", NodeType::Fact, &at(5, 9)))],
        );
        let early = picture.index.by_id["early"];
        let a = picture.index.by_id["a"];
        assert_eq!(picture.index.filing[early], Some(a), "filed at once");
        assert_eq!(picture.index.metacontexts[early], ["the-real"]);
        assert_eq!(picture.index.edges.edges[slot].src, End::Drawn(early));
        assert_eq!(picture.index.edges.claim[early], [slot]);
        assert!(picture.index.edges.claim[fa].contains(&slot));
        assert_eq!(picture.index.degrees[early], 3);
        assert!(picture.live.waiting.is_empty() && picture.live.waiting_count == 0);
        assert_eq!(
            picture
                .places
                .metacontexts
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["the-real"]
        );
    }

    #[test]
    fn membership_edges_update_what_the_filters_read() {
        let mut picture = picture();
        let fb = picture.index.by_id["fb"];
        let a = picture.index.by_id["a"];
        let (_, changes) = run(
            &mut picture,
            &[
                linked(identified("fb", "a", "tagged_with_topic", "t1")),
                linked(identified("fb", "doc", "sourced_from", "s1")),
                linked(identified("fb", "the-real", "has_metacontext", "m1")),
                linked(identified("b", "a", "subtopic_of", "st1")),
            ],
        );
        assert!(changes.pickers);
        assert_eq!(picture.index.tags[fb], [a]);
        assert_eq!(picture.index.sources[fb], ["doc"]);
        assert_eq!(picture.index.metacontexts[fb], ["the-real"]);
        let b = picture.index.by_id["b"];
        assert_eq!(picture.index.subtopics[a], [b]);
        let shown = filter::shown(
            &Filter {
                source: Some("doc".into()),
                ..Filter::default()
            },
            &picture.index,
        );
        assert_eq!(
            shown.passes.iter().filter(|&&p| p).count(),
            1,
            "the source filter finds the node read from it"
        );
        // The same edge again changes nothing.
        let (outcomes, _) = run(
            &mut picture,
            &[linked(identified("fb", "doc", "sourced_from", "s1"))],
        );
        assert_eq!(outcomes, [Outcome::Unchanged("seen already".into())]);
        assert_eq!(picture.index.sources[fb], ["doc"]);
    }

    #[test]
    fn a_node_that_leaves_the_active_set_goes_and_leaves_a_stub() {
        let mut picture = picture();
        let (fa, ia) = (picture.index.by_id["fa"], picture.index.by_id["ia"]);
        // A correction: fa gives way to a new fact, joined by superseded_by.
        let (_, changes) = run(
            &mut picture,
            &[
                Event::NodeStatusChanged(StatusChange {
                    node_id: "fa".into(),
                    old_status: "active".into(),
                    new_status: "corrected".into(),
                    counterpart: Some("fa-new".into()),
                }),
                stored(timed("fa-new", NodeType::Fact, &at(5, 9))),
                linked(identified("fa", "fa-new", "superseded_by", "succ")),
                linked(identified("fa-new", "a", "extracted_under_topic", "file")),
            ],
        );
        let new = picture.index.by_id["fa-new"];
        assert_eq!(changes.despawn, [fa].into());
        assert!(changes.spawn.contains(&new));
        assert!(!picture.index.active[fa]);
        let shown = filter::shown(&Filter::default(), &picture.index);
        assert!(!shown.visible[fa], "not drawn, whatever the filter");
        assert!(shown.visible[new]);
        // Its supports edge keeps it by id now.
        let supports = picture.live.edge_slots
            [&crate::graph_index::tests::edge("fa", "ia", "supports").edge_id];
        assert_eq!(
            picture.index.edges.edges[supports].src,
            End::Absent("fa".into())
        );
        assert!(picture.index.edges.claim[fa].is_empty());
        assert!(picture.index.edges.claim[ia].contains(&supports));
        // The connector to the new node is a stub with a marker.
        let succession = &picture.index.edges.edges[picture.live.edge_slots["succ"]];
        assert_eq!(succession.src, End::Absent("fa".into()));
        let alphas = edges::mesh_alphas(
            &picture.index.edges,
            &shown,
            &picture.places.points,
            crate::focus::Focus(None),
        );
        let pieces = connector_segments(
            &picture.index.edges,
            &picture.places.points,
            &picture.places.spans,
            &alphas,
        );
        let expected = edges::connector(Place::Absent, Place::Shown(picture.places.points[new]))
            .unwrap()
            .0;
        assert_eq!(pieces.len(), expected.len(), "a stub and its cross");
        // Back in the active set, it is drawn where it was.
        let before = picture.places.points[fa];
        let (_, changes) = run(
            &mut picture,
            &[Event::NodeStatusChanged(StatusChange {
                node_id: "fa".into(),
                old_status: "corrected".into(),
                new_status: "active".into(),
                counterpart: None,
            })],
        );
        assert_eq!(changes.spawn, [fa].into());
        assert_eq!(picture.places.points[fa], before);
        assert_eq!(picture.index.edges.edges[supports].src, End::Drawn(fa));
        // A node never drawn cannot come back without the snapshot.
        let (outcomes, _) = run(
            &mut picture,
            &[Event::NodeStatusChanged(StatusChange {
                node_id: "stranger".into(),
                old_status: "archived".into(),
                new_status: "active".into(),
                counterpart: None,
            })],
        );
        assert!(matches!(outcomes[0], Outcome::Refetch(_)));
    }

    fn timeline(points: Vec<TimepointView>) -> TimelineView {
        TimelineView {
            timeline_id: "line".into(),
            name: "The world".into(),
            description: String::new(),
            timepoints: points,
            reference_time: None,
            created_at: "2026-09-01T00:00:00Z".into(),
            graph: "test".into(),
        }
    }

    fn interval(id: &str, start: &str, end: &str) -> TimepointView {
        TimepointView {
            timepoint_id: id.into(),
            start: Some(start.into()),
            end: Some(end.into()),
            label: None,
            kind: "interval".into(),
            earliest: None,
            latest: None,
            contested: false,
            temporal_contradiction_id: None,
            metadata: Default::default(),
        }
    }

    #[test]
    fn a_timeline_replaced_places_the_claims_linked_to_it_again() {
        let mut picture = picture();
        picture.mode = TimeMode::Claim;
        let mut timelink = identified("fa", "line", "timelink", "tl");
        timelink.metadata.insert("timepoint_id".into(), "p".into());
        let fa = picture.index.by_id["fa"];
        // The timelink first, to a timepoint not yet on any timeline.
        let (_, changes) = run(&mut picture, &[linked(timelink)]);
        assert_eq!(picture.time.claim.spans[fa], None, "no position yet");
        assert!(changes.reshape.contains(&fa));
        assert_eq!(picture.index.spans[fa], None);

        // The timeline arrives with the point as an interval, inside the
        // claim axis, which is the recorded one: the graph had no timepoint.
        let first = timeline(vec![interval(
            "p",
            "2026-09-02T00:00:00Z",
            "2026-09-03T00:00:00Z",
        )]);
        let (outcomes, changes) = run(&mut picture, &[Event::TimelineStored(Box::new(first))]);
        assert!(matches!(&outcomes[0], Outcome::Applied(w) if w.contains("1 claims")));
        assert!(changes.reshape.contains(&fa));
        let span = picture.time.claim.spans[fa].unwrap();
        assert_eq!(span.profile, Profile::Solid);
        let axis = picture.time.claim.axis;
        assert_eq!(
            span.lo,
            time_y(&axis, parse_timestamp("2026-09-02T00:00:00Z").unwrap())
        );
        assert_eq!(
            picture.index.spans[fa],
            Some((span.lo, span.hi)),
            "drawn in claim time"
        );
        assert_eq!(picture.places.points[fa].y, centre(&span));
        assert_eq!(picture.time.counts.interval_points_drawn, 1);

        // Replaced, the point is longer, and the column follows.
        let second = timeline(vec![interval(
            "p",
            "2026-09-02T00:00:00Z",
            "2026-09-04T00:00:00Z",
        )]);
        run(&mut picture, &[Event::TimelineStored(Box::new(second))]);
        let longer = picture.time.claim.spans[fa].unwrap();
        assert_eq!(longer.lo, span.lo);
        assert!(longer.hi > span.hi);
        assert_eq!(picture.live.timelines.len(), 1, "replaced, not added");

        // Replaced again, the point runs on past the axis's top: the axis
        // does not grow for a timepoint, and the column is drawn to its top.
        let third = timeline(vec![interval(
            "p",
            "2026-09-02T00:00:00Z",
            "2027-03-01T00:00:00Z",
        )]);
        let (_, changes) = run(&mut picture, &[Event::TimelineStored(Box::new(third))]);
        assert_eq!(picture.time.claim.axis, axis, "nothing drawn moves");
        let clipped = picture.time.claim.spans[fa].unwrap();
        assert_eq!((clipped.lo, clipped.hi), (span.lo, axis.height));
        assert_eq!(clipped.clipped.above, 1);
        assert!(changes.reshape.contains(&fa));
        assert_eq!(picture.time.counts.clipped_nodes, 1);
        assert_eq!(
            crate::tray::tray_for(&picture.time).beyond[0].node,
            fa,
            "listed beyond the axis"
        );

        // A node recorded after the top grows the axis, and the column
        // clipped there reaches up to the new top.
        let (_, changes) = run(
            &mut picture,
            &[stored(timed(
                "late",
                NodeType::Fact,
                "2026-09-20T12:00:00Z",
            ))],
        );
        let grown = picture.time.claim.axis;
        assert_eq!(
            (grown.start, grown.height > axis.height),
            (axis.start, true)
        );
        let reached = picture.time.claim.spans[fa].unwrap();
        assert_eq!(reached.lo, span.lo, "the scale is kept");
        assert_eq!(reached.hi, grown.height);
        assert!(changes.reshape.contains(&fa));
    }

    #[test]
    fn a_snapshot_read_again_over_the_picture_moves_nothing() {
        let mut picture = picture();
        run(
            &mut picture,
            &[
                stored(timed("c", NodeType::Topic, &at(9, 6))),
                stored(timed("fc", NodeType::Fact, &at(9, 7))),
                linked(identified("fc", "c", "extracted_under_topic", "e-fc")),
                stored(timed("loose", NodeType::Fact, &at(9, 8))),
            ],
        );
        let seed = seed(
            &picture.index,
            &picture.places,
            &picture.time,
            &picture.live,
        );
        // The snapshot read again holds the same nodes and one more of each.
        let mut snapshot = small();
        for (id, kind, hour) in [
            ("c", NodeType::Topic, 6),
            ("fc", NodeType::Fact, 7),
            ("loose", NodeType::Fact, 8),
            ("d", NodeType::Topic, 10),
            ("fd", NodeType::Fact, 11),
        ] {
            snapshot.nodes.push(timed(id, kind, &at(9, hour)));
        }
        snapshot
            .edges
            .push(identified("fc", "c", "extracted_under_topic", "e-fc"));
        snapshot
            .edges
            .push(identified("fd", "d", "extracted_under_topic", "e-fd"));
        let again = crate::reload::prepare_seeded(&snapshot, Some(&seed), None).unwrap();
        for (i, id) in picture.index.ids.iter().enumerate() {
            let j = again.index.by_id[id];
            assert_eq!(again.places.points[j], picture.places.points[i], "{id}");
        }
        assert_eq!(
            again.time.recorded.axis.start,
            picture.time.recorded.axis.start
        );
        let d = again.index.by_id["d"];
        let ring = again.live.ring;
        assert_eq!(ring.next, 4, "the slot after the live topic's");
        let ground = Vec2::new(again.places.points[d].x, again.places.points[d].z);
        assert!(ground.distance(ring_point(&ring, 3)) < 1e-4);
        assert!(
            !again.live.settled[again.index.by_id["loose"]],
            "still provisional"
        );
    }
}
