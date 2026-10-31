//! Where every node sits.
//!
//! The scene's ground plane is the topic map (Bevy's x and z) and time is up
//! (Bevy's y), as DESIGN.md §6 sets out. Three rules place a node:
//!
//! - **Time.** Recorded time, `created_at`, maps linearly onto y: the
//!   earliest node at 0 and the latest at [`TIME_HEIGHT`], so the graph grows
//!   upward.
//! - **Topics** stand on the topic map where there is one ([`MapPlaces`]):
//!   the map is fitted from the topics' embeddings and saved
//!   (`topic_map.rs`), and every topic it names stands where it says. A
//!   topic the map does not name, and every topic while there is no map,
//!   takes a slot on a circle in creation order, spaced for the topics of
//!   the first snapshot read, and outside the map where there is one. A
//!   topic that arrives later, on the event stream or in a snapshot read
//!   again over the same picture, takes the next slot ([`ring_point`]): the
//!   circle's own angular step, on a ring just inside the circle, because
//!   the step past the last topic comes back round to the first. So a new
//!   topic moves no other, and it waits on its ring until its embedding
//!   puts it on the map (`relayout.rs`).
//! - **Claims** sit beside their filing topic (DESIGN.md §3): a fact's
//!   `extracted_under_topic` edge, an inference's `abstracts` edge, else its
//!   first `tagged_with_topic` edge, else a region for unfiled claims outside
//!   the circle. Where a claim has several such edges, the earliest recorded
//!   one counts, so the choice does not depend on the order the hub lists
//!   edges in. The claim's angle around the topic comes from a fixed hash of
//!   its node id; its distance grows with its rank among that topic's
//!   claims in recorded order, at the map's claim spread ([`Spread`]). A
//!   topic with more claims therefore spreads wider, and a new claim takes
//!   the next rank and moves no other.
//!
//! A first snapshot is placed by [`place_seeded`] with no seed, on its
//! saved map where there is one, a pure function of the two, so the same
//! snapshot and map always give the same picture. A snapshot read again over a picture
//! already drawn, after a gap in the event stream, is placed by
//! [`place_seeded`] with that picture as its [`Seed`]: every node it names
//! keeps its place, the time axis keeps its start and its scale and grows
//! upward to reach a later node ([`extend_axis`]), and only the nodes new to
//! it are placed by the rules above, so that reading the graph again moves
//! nothing. A claim whose place was provisional, one placed live before any
//! filing edge or tag for it had arrived, moves to its topic when one does.
//!
//! When the topics move, because a map was fitted or a topic was put on
//! it, [`follow`] says where every claim goes: to the same angle and rank
//! around its topic's new place, at the new spread.

use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;

use bevy::math::{Vec2, Vec3};

use crate::hub::{EdgeView, NodeType, NodeView, parse_timestamp};

/// The height of the time axis, from the earliest node to the latest.
pub const TIME_HEIGHT: f32 = 300.0;
/// The arc between neighbouring topics on the circle.
pub const TOPIC_SPACING: f32 = 3.0;
/// The smallest circle, so that a graph with a handful of topics is not a
/// knot at the origin.
pub const MIN_CIRCLE_RADIUS: f32 = 40.0;
/// How far a topic's first claim sits from the topic, clear of its disc.
pub const CLAIM_CLEARANCE: f32 = 3.2;
/// How fast the claims around a topic spread with rank: the k-th sits at
/// `CLAIM_CLEARANCE + CLAIM_SPACING * sqrt(k)`, which keeps the density
/// even as the count grows.
pub const CLAIM_SPACING: f32 = 1.5;
/// How far outside the circle the unfiled region's centre is.
pub const UNFILED_GAP: f32 = 30.0;
/// How far apart the rings that take topics arriving after the first
/// snapshot are, and how far inside the circle the first of them is: wide
/// enough that the claims of a ring and of its neighbour, which spread a
/// few units around their topics, keep apart on the map.
pub const RING_GAP: f32 = 16.0;

/// The share of [`CLAIM_SPACING`] the claims of a topic spread at: 1 around
/// the circle, and the map's own on a map (`relax.rs`). Only the spacing
/// between ranks scales; the first claim keeps [`CLAIM_CLEARANCE`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread(pub f32);

impl Default for Spread {
    fn default() -> Self {
        Spread(1.0)
    }
}

/// The topic map's places, as the layout reads them: each topic's place by
/// id, the claim spread, and the map's outer radius, beyond which the ring
/// of topics it does not name and the unfiled region stand.
#[derive(Clone, Copy, Debug)]
pub struct MapPlaces<'a> {
    pub topics: &'a std::collections::BTreeMap<String, [f32; 2]>,
    pub spread: f32,
    pub radius: f32,
}

/// The kinds of node the scene draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Topic,
    Fact,
    Inference,
}

/// A node and its place in the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub node_id: String,
    pub kind: Kind,
    pub position: Vec3,
    /// For a claim, the topic it was placed beside, by its filing edge or
    /// its tag; `None` for a topic and for an unfiled claim.
    pub topic: Option<String>,
    /// For a claim, which rule placed it; a topic's is [`Filing::Filed`].
    pub filing: Filing,
    /// Whether the place is final. Every node a snapshot places is; a
    /// claim placed live before its filing edge arrived is not, and moves
    /// to its topic once when that edge comes (`apply.rs`).
    pub settled: bool,
}

/// Which rule placed a claim, from the least to the most: none, a tag, a
/// filing edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Filing {
    /// The unfiled region.
    #[default]
    Unfiled,
    /// Its earliest tag.
    Tagged,
    /// Its filing edge.
    Filed,
}

/// The topic circle, and the slot the next topic takes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TopicRing {
    pub radius: f32,
    /// How many topics the circle was spaced for: the first snapshot's.
    pub count: usize,
    /// The slot the next topic takes. The slots below `count` are on the
    /// circle; the rest are on rings inside it ([`ring_point`]).
    pub next: usize,
}

/// Where the picture drawn so far put each node, for placing a snapshot
/// read again over it with [`place_seeded`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seed {
    /// Each node drawn so far, by id: its place on the map and its height
    /// in recorded time, the topic it stands beside, the rule that placed
    /// it, and whether that place is final.
    pub places: HashMap<String, SeedPlace>,
    pub ring: TopicRing,
    /// How many claims have been placed beside each topic, by the topic's
    /// id: the next claim's rank.
    pub ranks: HashMap<String, usize>,
    /// The recorded time axis.
    pub time: TimeAxis,
    /// The claim spread the picture was drawn at.
    pub spread: Spread,
}

/// One node's place in a [`Seed`].
#[derive(Clone, Debug, PartialEq)]
pub struct SeedPlace {
    pub position: Vec3,
    pub topic: Option<String>,
    pub filing: Filing,
    pub settled: bool,
}

/// How a claim's place was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor<'a> {
    /// By its filing edge, to this topic.
    Filed(&'a str),
    /// By its earliest tag, to this topic.
    Tagged(&'a str),
    /// By nothing: the unfiled region.
    Unfiled,
}

impl<'a> Anchor<'a> {
    fn topic(self) -> Option<&'a str> {
        match self {
            Anchor::Filed(topic) | Anchor::Tagged(topic) => Some(topic),
            Anchor::Unfiled => None,
        }
    }
}

/// The linear map from recorded time to height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeAxis {
    /// The earliest instant, in microseconds since the Unix epoch.
    pub start: i64,
    /// The latest instant minus the earliest; zero when there is one instant.
    pub span: i64,
    pub height: f32,
}

/// The layout of a whole snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub nodes: Vec<Placed>,
    pub time: TimeAxis,
    pub circle_radius: f32,
    /// The circle and the next topic's slot.
    pub ring: TopicRing,
    /// How many claims stand beside each topic, by its id, and how many in
    /// the unfiled region: the ranks the next claims take.
    pub ranks: HashMap<String, usize>,
    pub unfiled_members: usize,
    /// The claim spread the claims were placed at.
    pub spread: Spread,
    /// How many claims were placed by a filing edge, by a tag, and in the
    /// unfiled region.
    pub filed: usize,
    pub tagged: usize,
    pub unfiled: usize,
    /// Nodes not drawn: a kind this client does not know, a `created_at`
    /// that does not parse, or an id already drawn.
    pub left_out: usize,
}

/// The time axis spanning these instants.
pub fn time_axis(instants: impl IntoIterator<Item = i64>, height: f32) -> TimeAxis {
    let (min, max) = instants
        .into_iter()
        .fold((i64::MAX, i64::MIN), |(lo, hi), t| (lo.min(t), hi.max(t)));
    if min > max {
        return TimeAxis {
            start: 0,
            span: 0,
            height,
        };
    }
    TimeAxis {
        start: min,
        span: max - min,
        height,
    }
}

/// The axis grown upward, at its own scale, to reach `instant`: the start
/// and the height of every instant already on it stay as they were, and
/// only the top moves. An instant at or below the top changes nothing; one
/// below the start keeps a height below 0. An axis with no span cannot keep
/// a scale it does not have, and takes `instant` as a top at its own height,
/// or [`TIME_HEIGHT`] where it has none.
pub fn extend_axis(axis: &TimeAxis, instant: i64) -> TimeAxis {
    let top = axis.start.saturating_add(axis.span);
    if instant <= top {
        return *axis;
    }
    let span = instant - axis.start;
    let height = if axis.span <= 0 || axis.height <= 0.0 {
        if axis.height > 0.0 {
            axis.height
        } else {
            TIME_HEIGHT
        }
    } else {
        (axis.height as f64 * span as f64 / axis.span as f64) as f32
    };
    TimeAxis {
        start: axis.start,
        span,
        height,
    }
}

/// The height of an instant on the axis. With a single instant every node
/// is at 0.
pub fn time_y(axis: &TimeAxis, instant: i64) -> f32 {
    if axis.span == 0 {
        return 0.0;
    }
    let fraction = (instant - axis.start) as f64 / axis.span as f64;
    (fraction * axis.height as f64) as f32
}

/// The instant at a height on the axis, the inverse of [`time_y`] to the
/// nearest microsecond. A height outside the axis gives an instant outside
/// its span. With a single instant every height is that instant.
pub fn instant_at(axis: &TimeAxis, y: f32) -> i64 {
    if axis.span == 0 || axis.height == 0.0 {
        return axis.start;
    }
    axis.start + (y as f64 / axis.height as f64 * axis.span as f64).round() as i64
}

/// The radius of the topic circle for this many topics.
pub fn circle_radius(topics: usize) -> f32 {
    (topics as f32 * TOPIC_SPACING / TAU).max(MIN_CIRCLE_RADIUS)
}

/// The radius of the ring of topic slots: the circle for this many topics,
/// or outside the map where there is one, so that the slots and the
/// unfiled region stand clear of it.
pub fn ring_radius(topics: usize, map_radius: Option<f32>) -> f32 {
    circle_radius(topics).max(map_radius.unwrap_or(0.0))
}

/// The place of the i-th of n topics on the circle.
pub fn circle_point(index: usize, count: usize, radius: f32) -> Vec2 {
    Vec2::from_angle(TAU * index as f32 / count.max(1) as f32) * radius
}

/// The place of a topic slot. The first `count` slots stand on the circle;
/// each later one keeps the circle's angular step, so slot `count + k`
/// stands at the angle of slot `k`, on a ring [`RING_GAP`] inside the
/// circle, then on a ring further in for each further round, and once no
/// ring fits inside, on rings outside the unfiled region. No slot moves
/// when a later one is taken.
pub fn ring_point(ring: &TopicRing, slot: usize) -> Vec2 {
    let count = ring.count.max(1);
    if slot < count {
        return circle_point(slot, count, ring.radius);
    }
    let extra = slot - count;
    let round = 1 + extra / count;
    let inner = ((ring.radius - RING_GAP) / RING_GAP).floor().max(0.0) as usize;
    let radius = if round <= inner {
        ring.radius - round as f32 * RING_GAP
    } else {
        ring.radius + 2.0 * UNFILED_GAP + (round - inner) as f32 * RING_GAP
    };
    circle_point(extra % count, count, radius)
}

/// The centre of the unfiled region, outside the circle.
pub fn unfiled_centre(ring: &TopicRing) -> Vec2 {
    Vec2::new(ring.radius + UNFILED_GAP, 0.0)
}

/// FNV-1a, 64 bits: a fixed hash, the same on every run and every machine,
/// unlike the standard library's randomly seeded one.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A claim's offset from its topic: an angle from the hash of its id and a
/// distance from its rank among the topic's claims, at the full spacing.
#[cfg(test)]
pub fn claim_offset(node_id: &str, rank: usize) -> Vec2 {
    claim_offset_spread(node_id, rank, 1.0)
}

/// A claim's offset from its topic at a claim spread: the angle of
/// [`claim_offset`], and a distance whose spacing between ranks is
/// `spread` of [`CLAIM_SPACING`].
pub fn claim_offset_spread(node_id: &str, rank: usize, spread: f32) -> Vec2 {
    let fraction = (fnv1a(node_id.as_bytes()) >> 11) as f64 / (1u64 << 53) as f64;
    let angle = (fraction * std::f64::consts::TAU) as f32;
    let distance = CLAIM_CLEARANCE + CLAIM_SPACING * spread * (rank as f32).sqrt();
    Vec2::from_angle(angle) * distance
}

/// The rank a claim `distance` from its topic was placed at, at a claim
/// spread: the inverse of [`claim_offset_spread`]'s distance, to the
/// nearest whole rank.
pub fn rank_of(distance: f32, spread: f32) -> usize {
    let step = CLAIM_SPACING * spread;
    if step <= 0.0 || distance <= CLAIM_CLEARANCE {
        return 0;
    }
    let root = ((distance - CLAIM_CLEARANCE) / step) as f64;
    (root * root).round() as usize
}

/// The anchor of every claim among `kinds`, by the rules in the module
/// documentation. Only edges between nodes in `kinds` count, so an edge to
/// a topic that is not drawn falls through to the next rule.
pub fn anchors<'a>(
    kinds: &HashMap<&'a str, Kind>,
    edges: &'a [EdgeView],
) -> HashMap<&'a str, Anchor<'a>> {
    // For each claim and each rule (0 the filing edge, 1 a tag), the
    // earliest edge: its time, its id to break ties, and its topic.
    let mut earliest: HashMap<(&str, u8), (i64, &str, &str)> = HashMap::new();
    for edge in edges {
        let (Some(&src), Some(Kind::Topic)) = (
            kinds.get(edge.src_id.as_str()),
            kinds.get(edge.dst_id.as_str()),
        ) else {
            continue;
        };
        let rule = match (src, edge.edge_type.as_str()) {
            (Kind::Fact, "extracted_under_topic") | (Kind::Inference, "abstracts") => 0,
            (Kind::Fact | Kind::Inference, "tagged_with_topic") => 1,
            _ => continue,
        };
        let candidate = (
            parse_timestamp(&edge.created_at).unwrap_or(i64::MAX),
            edge.edge_id.as_str(),
            edge.dst_id.as_str(),
        );
        earliest
            .entry((edge.src_id.as_str(), rule))
            .and_modify(|best| {
                if (candidate.0, candidate.1) < (best.0, best.1) {
                    *best = candidate;
                }
            })
            .or_insert(candidate);
    }
    kinds
        .iter()
        .filter(|&(_, &kind)| kind != Kind::Topic)
        .map(|(&id, _)| {
            let anchor = match (earliest.get(&(id, 0)), earliest.get(&(id, 1))) {
                (Some(&(_, _, topic)), _) => Anchor::Filed(topic),
                (None, Some(&(_, _, topic))) => Anchor::Tagged(topic),
                (None, None) => Anchor::Unfiled,
            };
            (id, anchor)
        })
        .collect()
}

/// The rule an anchor names.
pub fn filing_of(anchor: Anchor) -> Filing {
    match anchor {
        Anchor::Filed(_) => Filing::Filed,
        Anchor::Tagged(_) => Filing::Tagged,
        Anchor::Unfiled => Filing::Unfiled,
    }
}

/// The drawn kind of a node type, `None` for one this client does not
/// draw.
pub fn kind_of(node_type: NodeType) -> Option<Kind> {
    match node_type {
        NodeType::Topic => Some(Kind::Topic),
        NodeType::Fact => Some(Kind::Fact),
        NodeType::Inference => Some(Kind::Inference),
        NodeType::Unknown => None,
    }
}

/// Places every node of a snapshot. A node id the snapshot repeats is
/// drawn once, as it first appears.
#[cfg(test)]
pub fn place(nodes: &[NodeView], edges: &[EdgeView]) -> Layout {
    place_seeded(nodes, edges, None, None)
}

/// Places every node of a snapshot, over the picture `seed` describes
/// where there is one, on the topic map `map` where there is one: a node
/// the seed names keeps its place, unless it is a claim whose place was
/// provisional and the snapshot gives it a better rule; the time axis keeps
/// the seed's start and scale and grows upward to reach the latest node; a
/// topic new to the seed stands where the map puts it, or takes the ring's
/// next slot in creation order where the map does not name it; and claims
/// new to the seed take the next ranks of their topics, in recorded order,
/// at the seed's claim spread, or the map's without a seed. Without either
/// this is [`place`].
pub fn place_seeded(
    nodes: &[NodeView],
    edges: &[EdgeView],
    seed: Option<&Seed>,
    map: Option<MapPlaces>,
) -> Layout {
    let mut seen = HashSet::new();
    let drawable: Vec<(&NodeView, Kind, i64)> = nodes
        .iter()
        .filter_map(|node| {
            let kind = kind_of(node.node_type)?;
            let created = parse_timestamp(&node.created_at)?;
            seen.insert(node.node_id.as_str())
                .then_some((node, kind, created))
        })
        .collect();
    let left_out = nodes.len() - drawable.len();
    let instants = drawable.iter().map(|&(_, _, t)| t);
    let time = match seed {
        None => time_axis(instants, TIME_HEIGHT),
        Some(seed) => instants.fold(seed.time, |axis, t| extend_axis(&axis, t)),
    };
    let kinds: HashMap<&str, Kind> = drawable
        .iter()
        .map(|&(node, kind, _)| (node.node_id.as_str(), kind))
        .collect();
    let seeded = |id: &str| seed.and_then(|s| s.places.get(id));

    // Topics in creation order, ties broken by id; a topic the seed names
    // keeps its place, and every other takes the next slot.
    let mut topics: Vec<(i64, &str)> = drawable
        .iter()
        .filter(|&&(_, kind, _)| kind == Kind::Topic)
        .map(|&(node, _, t)| (t, node.node_id.as_str()))
        .collect();
    topics.sort_unstable();
    let mut ring = seed.map_or(
        TopicRing {
            radius: ring_radius(topics.len(), map.map(|m| m.radius)),
            count: topics.len(),
            next: 0,
        },
        |s| s.ring,
    );
    let spread = match (seed, map) {
        (Some(seed), _) => seed.spread,
        (None, Some(map)) => Spread(map.spread),
        (None, None) => Spread::default(),
    };
    let on_map = |id: &str| {
        map.and_then(|m| m.topics.get(id))
            .map(|&[x, z]| Vec2::new(x, z))
    };
    let mut topic_places: HashMap<&str, Vec2> = HashMap::new();
    for &(_, id) in &topics {
        let place = match (seeded(id), on_map(id)) {
            (Some(kept), _) => Vec2::new(kept.position.x, kept.position.z),
            (None, Some(place)) => place,
            (None, None) => {
                let place = ring_point(&ring, ring.next);
                ring.next += 1;
                place
            }
        };
        topic_places.insert(id, place);
    }
    let unfiled_centre = unfiled_centre(&ring);

    // Claims in recorded order, each taking the next rank of its topic.
    let anchors = anchors(&kinds, edges);
    let mut claims: Vec<(i64, &str)> = drawable
        .iter()
        .filter(|&&(_, kind, _)| kind != Kind::Topic)
        .map(|&(node, _, t)| (t, node.node_id.as_str()))
        .collect();
    claims.sort_unstable();
    let mut ranks: HashMap<String, usize> = seed.map(|s| s.ranks.clone()).unwrap_or_default();
    let mut unfiled_members = 0;
    let mut claim_places: HashMap<&str, (Vec2, Option<String>, Filing, bool)> = HashMap::new();
    for &(_, id) in &claims {
        let anchor = anchors[id];
        let rule = filing_of(anchor);
        let kept = seeded(id).filter(|kept| kept.settled || rule <= kept.filing);
        let (ground, topic, filing, settled) = match kept {
            Some(kept) => (
                Vec2::new(kept.position.x, kept.position.z),
                kept.topic.clone(),
                kept.filing,
                kept.settled,
            ),
            None => {
                let (centre, rank) = match anchor.topic() {
                    Some(topic) => {
                        let rank = ranks.entry(topic.to_owned()).or_default();
                        *rank += 1;
                        (topic_places[topic], *rank - 1)
                    }
                    None => (unfiled_centre, unfiled_members),
                };
                let settled = seed.is_none() || seeded(id).is_none() || rule == Filing::Filed;
                (
                    centre + claim_offset_spread(id, rank, spread.0),
                    anchor.topic().map(str::to_owned),
                    rule,
                    settled,
                )
            }
        };
        if topic.is_none() {
            unfiled_members += 1;
        }
        claim_places.insert(id, (ground, topic, filing, settled));
    }

    let count = |wanted: fn(&Anchor) -> bool| anchors.values().filter(|a| wanted(a)).count();
    let placed = drawable
        .iter()
        .map(|&(node, kind, t)| {
            let id = node.node_id.as_str();
            let height = seeded(id).map_or_else(|| time_y(&time, t), |kept| kept.position.y);
            let (ground, topic, filing, settled) = match kind {
                Kind::Topic => (topic_places[id], None, Filing::Filed, true),
                _ => claim_places[id].clone(),
            };
            Placed {
                node_id: node.node_id.clone(),
                kind,
                position: Vec3::new(ground.x, height, ground.y),
                topic,
                filing,
                settled,
            }
        })
        .collect();
    Layout {
        nodes: placed,
        time,
        circle_radius: ring.radius,
        ring,
        ranks,
        unfiled_members,
        spread,
        filed: count(|a| matches!(a, Anchor::Filed(_))),
        tagged: count(|a| matches!(a, Anchor::Tagged(_))),
        unfiled: count(|a| matches!(a, Anchor::Unfiled)),
        left_out,
    }
}

/// How the claims stand around their topics: the claim spread and the
/// centre of the unfiled region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrangement {
    pub spread: f32,
    pub unfiled: Vec2,
}

/// Where the nodes go when the topics in `moved` move to the places it
/// gives and the claims' arrangement changes from `from` to `to`: each
/// moved topic to its place, each claim beside a topic to the same angle
/// and rank around that topic's place, and each unfiled claim the same
/// around the unfiled region's centre, at the new spread. `ground` is each
/// node's place on the map now, and every claim stands at its rank around
/// its topic there, as the layout put it. Only the nodes whose place
/// changes are listed, in node order.
pub fn follow(
    ids: &[String],
    kinds: &[Kind],
    filing: &[Option<usize>],
    ground: &[Vec2],
    moved: &HashMap<usize, Vec2>,
    from: Arrangement,
    to: Arrangement,
) -> Vec<(usize, Vec2)> {
    let respread = from.spread != to.spread;
    (0..ground.len().min(kinds.len()))
        .filter_map(|i| {
            let target = match kinds[i] {
                Kind::Topic => *moved.get(&i)?,
                Kind::Fact | Kind::Inference => {
                    let (old, new) = match filing.get(i).copied().flatten() {
                        Some(t) => {
                            let old = *ground.get(t)?;
                            (old, moved.get(&t).copied().unwrap_or(old))
                        }
                        None => (from.unfiled, to.unfiled),
                    };
                    if old == new && !respread {
                        return None;
                    }
                    let rank = rank_of(ground[i].distance(old), from.spread);
                    new + claim_offset_spread(ids.get(i)?, rank, to.spread)
                }
            };
            (target != ground[i]).then_some((i, target))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::parse_snapshot;

    fn node(id: &str, node_type: NodeType, created_at: &str) -> NodeView {
        NodeView {
            node_id: id.into(),
            node_type,
            content: String::new(),
            description: String::new(),
            status: "active".into(),
            source_id: None,
            extraction_method: "unspecified".into(),
            confidence: None,
            retrieved_at: None,
            created_at: created_at.into(),
            graph: "test".into(),
            metadata: Default::default(),
        }
    }

    fn edge(id: &str, src: &str, dst: &str, edge_type: &str, created_at: &str) -> EdgeView {
        EdgeView {
            edge_id: id.into(),
            src_id: src.into(),
            dst_id: dst.into(),
            edge_type: edge_type.into(),
            weight: 1.0,
            validity: Vec::new(),
            created_at: created_at.into(),
            graph: "test".into(),
            metadata: Default::default(),
        }
    }

    fn day(d: u32) -> String {
        format!("2026-09-{d:02}T12:00:00Z")
    }

    /// Two topics and four claims: one filed, one filed twice, one tagged
    /// only, one with nothing.
    fn small_graph() -> (Vec<NodeView>, Vec<EdgeView>) {
        let nodes = vec![
            node("t1", NodeType::Topic, &day(1)),
            node("t2", NodeType::Topic, &day(2)),
            node("f1", NodeType::Fact, &day(3)),
            node("i1", NodeType::Inference, &day(4)),
            node("f2", NodeType::Fact, &day(5)),
            node("f3", NodeType::Fact, &day(6)),
        ];
        let edges = vec![
            edge("e1", "f1", "t1", "extracted_under_topic", &day(3)),
            // f1 is also tagged t2; the filing edge wins.
            edge("e2", "f1", "t2", "tagged_with_topic", &day(3)),
            // i1 abstracts both topics; the earlier edge wins.
            edge("e3", "i1", "t2", "abstracts", &day(8)),
            edge("e4", "i1", "t1", "abstracts", &day(4)),
            // f2 has only tags, and one of them points at no node.
            edge("e5", "f2", "t2", "tagged_with_topic", &day(5)),
            edge("e6", "f2", "gone", "tagged_with_topic", &day(1)),
            // f3's filing edge points at a topic not in the snapshot.
            edge("e7", "f3", "gone", "extracted_under_topic", &day(6)),
            // A fact never files by `abstracts`, nor by a segment edge.
            edge("e8", "seg", "f3", "contains", &day(6)),
        ];
        (nodes, edges)
    }

    fn position(layout: &Layout, id: &str) -> Vec3 {
        layout
            .nodes
            .iter()
            .find(|n| n.node_id == id)
            .unwrap_or_else(|| panic!("{id} is placed"))
            .position
    }

    fn ground(v: Vec3) -> Vec2 {
        Vec2::new(v.x, v.z)
    }

    #[test]
    fn fnv1a_matches_the_published_test_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn a_claims_offset_is_the_same_on_every_run() {
        // Worked out by hand from FNV-1a and the spacing constants, so that
        // swapping in a randomly seeded hasher, or changing the rule or the
        // spacing, fails here.
        let id = "fac70000-0000-4000-8000-000000000001";
        let near = claim_offset(id, 0);
        assert!(
            near.distance(Vec2::new(2.813_36, -1.524_796)) < 1e-4,
            "{near}"
        );
        let fourth = claim_offset(id, 4);
        assert!(
            fourth.distance(Vec2::new(5.450_886, -2.954_293)) < 1e-4,
            "{fourth}"
        );
        let (nodes, edges) = small_graph();
        assert_eq!(place(&nodes, &edges), place(&nodes, &edges));
    }

    #[test]
    fn distance_from_the_topic_grows_with_rank() {
        let distances: Vec<f32> = (0..50).map(|k| claim_offset("x", k).length()).collect();
        assert!(distances.windows(2).all(|w| w[1] > w[0]));
        assert!((distances[0] - CLAIM_CLEARANCE).abs() < 1e-5);
    }

    #[test]
    fn a_busier_topic_spreads_wider() {
        let mut nodes = vec![
            node("busy", NodeType::Topic, &day(1)),
            node("quiet", NodeType::Topic, &day(1)),
        ];
        let mut edges = Vec::new();
        for i in 0..20 {
            let topic = if i < 3 { "quiet" } else { "busy" };
            let id = format!("c{i}");
            nodes.push(node(&id, NodeType::Fact, &day(2 + i)));
            edges.push(edge(
                &format!("e{i}"),
                &id,
                topic,
                "extracted_under_topic",
                &day(2),
            ));
        }
        let layout = place(&nodes, &edges);
        let spread = |topic: &str, claims: std::ops::Range<u32>| {
            let centre = ground(position(&layout, topic));
            claims
                .map(|i| ground(position(&layout, &format!("c{i}"))).distance(centre))
                .fold(0.0_f32, f32::max)
        };
        assert!(spread("busy", 3..20) > spread("quiet", 0..3));
    }

    #[test]
    fn a_new_claim_moves_no_other_node() {
        let (nodes, edges) = small_graph();
        let before = place(&nodes, &edges);
        let with = |created: &str| {
            let (mut nodes, mut edges) = (nodes.clone(), edges.clone());
            nodes.push(node("f4", NodeType::Fact, created));
            edges.push(edge("e9", "f4", "t1", "extracted_under_topic", created));
            place(&nodes, &edges)
        };
        // Recorded in the same instant as the latest node, so the time axis
        // is unchanged and nothing may move at all.
        let same_instant = with(&day(6));
        assert_eq!(same_instant.nodes.len(), before.nodes.len() + 1);
        for old in &before.nodes {
            assert_eq!(position(&same_instant, &old.node_id), old.position);
        }
        // Recorded later, so the axis rescales to the new span; still
        // nothing moves on the ground.
        let later = with(&day(9));
        for old in &before.nodes {
            assert_eq!(ground(position(&later, &old.node_id)), ground(old.position));
        }
        let t1 = ground(position(&later, "t1"));
        let f4 = ground(position(&later, "f4"));
        assert!((f4.distance(t1) - claim_offset("f4", 2).length()).abs() < 1e-4);
    }

    #[test]
    fn claims_file_by_edge_then_tag_then_nowhere() {
        let (nodes, edges) = small_graph();
        let kinds: HashMap<&str, Kind> = [
            ("t1", Kind::Topic),
            ("t2", Kind::Topic),
            ("f1", Kind::Fact),
            ("i1", Kind::Inference),
            ("f2", Kind::Fact),
            ("f3", Kind::Fact),
        ]
        .into_iter()
        .collect();
        let found = anchors(&kinds, &edges);
        assert_eq!(found["f1"], Anchor::Filed("t1"));
        assert_eq!(found["i1"], Anchor::Filed("t1"), "the earlier abstracts");
        assert_eq!(found["f2"], Anchor::Tagged("t2"));
        assert_eq!(found["f3"], Anchor::Unfiled, "its topic is not drawn");
        assert_eq!(found.len(), 4, "topics have no anchor");

        let layout = place(&nodes, &edges);
        assert_eq!(
            (layout.filed, layout.tagged, layout.unfiled),
            (2, 1, 1),
            "counted by rule"
        );
        let t1 = ground(position(&layout, "t1"));
        let f1 = ground(position(&layout, "f1"));
        assert!(
            (f1.distance(t1) - CLAIM_CLEARANCE).abs() < 1e-4,
            "t1's first claim"
        );
        let f3 = ground(position(&layout, "f3"));
        assert!(f3.length() > layout.circle_radius + UNFILED_GAP / 2.0);

        let topic_of = |id: &str| {
            let placed = layout.nodes.iter().find(|n| n.node_id == id).unwrap();
            placed.topic.clone()
        };
        assert_eq!(topic_of("f1").as_deref(), Some("t1"));
        assert_eq!(topic_of("f2").as_deref(), Some("t2"), "by its tag");
        assert_eq!(topic_of("f3"), None, "unfiled");
        assert_eq!(topic_of("t1"), None, "a topic is beside no topic");
    }

    #[test]
    fn the_filing_edge_does_not_depend_on_edge_order() {
        let (nodes, mut edges) = small_graph();
        let forward = place(&nodes, &edges);
        edges.reverse();
        assert_eq!(place(&nodes, &edges), forward);
    }

    #[test]
    fn topics_stand_evenly_on_the_circle_in_creation_order() {
        let nodes: Vec<NodeView> = (0..8)
            .map(|i| node(&format!("t{}", 7 - i), NodeType::Topic, &day(1 + 7 - i)))
            .collect();
        let layout = place(&nodes, &[]);
        let radius = layout.circle_radius;
        assert_eq!(radius, MIN_CIRCLE_RADIUS);
        for i in 0..8 {
            let at = ground(position(&layout, &format!("t{i}")));
            assert!((at.length() - radius).abs() < 1e-3);
            assert!(at.distance(circle_point(i, 8, radius)) < 1e-3);
        }
        assert!((circle_radius(427) - 427.0 * TOPIC_SPACING / TAU).abs() < 1e-3);
    }

    #[test]
    fn a_later_topic_keeps_the_circles_step_on_a_ring_inside_it() {
        let ring = TopicRing {
            radius: 205.0,
            count: 430,
            next: 430,
        };
        for slot in [0, 7, 429] {
            assert!(ring_point(&ring, slot).distance(circle_point(slot, 430, 205.0)) < 1e-4);
        }
        // Slot 430 + k stands at slot k's angle, a ring inside.
        for k in [0, 1, 215] {
            let at = ring_point(&ring, 430 + k);
            let on_circle = circle_point(k, 430, 205.0);
            assert!((at.length() - (205.0 - RING_GAP)).abs() < 1e-3, "{at}");
            assert!(at.normalize().distance(on_circle.normalize()) < 1e-4);
        }
        assert!((ring_point(&ring, 860).length() - (205.0 - 2.0 * RING_GAP)).abs() < 1e-3);
        // A small circle has room for one ring inside, then rings go
        // outside the unfiled region.
        let small = TopicRing {
            radius: MIN_CIRCLE_RADIUS,
            count: 2,
            next: 2,
        };
        assert!((ring_point(&small, 2).length() - (MIN_CIRCLE_RADIUS - RING_GAP)).abs() < 1e-3);
        let outside = ring_point(&small, 4).length();
        assert!(outside > unfiled_centre(&small).length(), "{outside}");
    }

    #[test]
    fn an_axis_grows_upward_at_its_own_scale() {
        let axis = time_axis([100, 300], 50.0);
        assert_eq!(extend_axis(&axis, 250), axis, "inside: unchanged");
        assert_eq!(extend_axis(&axis, 50), axis, "below: unchanged");
        let grown = extend_axis(&axis, 500);
        assert_eq!((grown.start, grown.span, grown.height), (100, 400, 100.0));
        for instant in [100, 200, 300] {
            assert!((time_y(&grown, instant) - time_y(&axis, instant)).abs() < 1e-4);
        }
        assert_eq!(time_y(&grown, 500), 100.0);
        assert!(time_y(&grown, 50) < 0.0, "earlier stands below 0");
        let single = time_axis([7], 50.0);
        let from_one = extend_axis(&single, 17);
        assert_eq!(
            (from_one.start, from_one.span, from_one.height),
            (7, 10, 50.0)
        );
    }

    #[test]
    fn a_seeded_layout_keeps_every_place_the_seed_names() {
        let (nodes, edges) = small_graph();
        let first = place(&nodes, &edges);
        let seed = Seed {
            places: first
                .nodes
                .iter()
                .map(|n| {
                    (
                        n.node_id.clone(),
                        SeedPlace {
                            position: n.position,
                            topic: n.topic.clone(),
                            filing: n.filing,
                            settled: n.settled,
                        },
                    )
                })
                .collect(),
            ring: TopicRing {
                next: first.ring.count,
                ..first.ring
            },
            ranks: first.ranks.clone(),
            time: first.time,
            spread: first.spread,
        };
        let (mut more_nodes, mut more_edges) = (nodes.clone(), edges.clone());
        more_nodes.push(node("t3", NodeType::Topic, &day(9)));
        more_nodes.push(node("f9", NodeType::Fact, &day(10)));
        more_edges.push(edge("e9", "f9", "t1", "extracted_under_topic", &day(10)));
        let again = place_seeded(&more_nodes, &more_edges, Some(&seed), None);
        for old in &first.nodes {
            assert_eq!(
                position(&again, &old.node_id),
                old.position,
                "{}",
                old.node_id
            );
        }
        assert_eq!(again.time.start, first.time.start);
        let t3 = ground(position(&again, "t3"));
        assert!(t3.distance(ring_point(&seed.ring, seed.ring.next)) < 1e-4);
        let f9 = ground(position(&again, "f9"));
        let t1 = ground(position(&again, "t1"));
        assert!((f9.distance(t1) - claim_offset("f9", 2).length()).abs() < 1e-3);
    }

    #[test]
    fn time_maps_linearly_with_the_earliest_at_zero() {
        let axis = time_axis([300, 100, 200], 50.0);
        assert_eq!(
            axis,
            TimeAxis {
                start: 100,
                span: 200,
                height: 50.0
            }
        );
        assert_eq!(time_y(&axis, 100), 0.0);
        assert_eq!(time_y(&axis, 200), 25.0);
        assert_eq!(time_y(&axis, 300), 50.0);
        let single = time_axis([7], 50.0);
        assert_eq!(time_y(&single, 7), 0.0);
        assert_eq!(time_axis([], 50.0).span, 0);
    }

    #[test]
    fn a_height_reads_back_as_its_instant() {
        let axis = time_axis([1_000, 1_000 + 86_400_000_000], TIME_HEIGHT);
        for instant in [
            1_000,
            43_201_000_000,
            1_000 + 86_400_000_000,
            -5_000_000_000,
        ] {
            // A height is an f32, good to about one part in ten million
            // of the axis.
            let back = instant_at(&axis, time_y(&axis, instant));
            assert!(
                (back - instant).abs() <= axis.span / 1_000_000,
                "{instant} came back as {back}"
            );
        }
        assert_eq!(instant_at(&axis, TIME_HEIGHT / 2.0), 43_200_001_000);
        assert_eq!(instant_at(&time_axis([7], 50.0), 30.0), 7);
    }

    #[test]
    fn the_graph_grows_upward() {
        let (nodes, edges) = small_graph();
        let layout = place(&nodes, &edges);
        let heights: Vec<f32> = ["t1", "t2", "f1", "i1", "f2", "f3"]
            .iter()
            .map(|id| position(&layout, id).y)
            .collect();
        assert_eq!(heights[0], 0.0);
        assert_eq!(heights[5], TIME_HEIGHT);
        assert!(heights.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn unknown_kinds_and_bad_times_are_left_out() {
        let nodes = vec![
            node("t1", NodeType::Topic, &day(1)),
            node("odd", NodeType::Unknown, &day(2)),
            node("f1", NodeType::Fact, "not a time"),
        ];
        let layout = place(&nodes, &[]);
        assert_eq!(layout.left_out, 2);
        assert_eq!(layout.nodes.len(), 1);
    }

    #[test]
    fn a_repeated_id_is_drawn_once() {
        let nodes = vec![
            node("t1", NodeType::Topic, &day(1)),
            node("x", NodeType::Fact, &day(2)),
            node("x", NodeType::Topic, &day(3)),
        ];
        let edges = [edge("e1", "x", "t1", "extracted_under_topic", &day(2))];
        let layout = place(&nodes, &edges);
        assert_eq!(layout.left_out, 1);
        let kinds: Vec<Kind> = layout.nodes.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [Kind::Topic, Kind::Fact]);
        assert_eq!(layout.filed, 1);
    }

    #[test]
    fn the_fixture_places_every_known_node() {
        let snapshot = parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        let layout = place(&snapshot.nodes, &snapshot.edges);
        assert_eq!(layout.nodes.len(), 5);
        assert_eq!(layout.left_out, 1, "the node of unknown kind");
        assert_eq!((layout.filed, layout.tagged, layout.unfiled), (2, 1, 0));
        let heights = layout.nodes.iter().map(|n| n.position.y);
        let (lo, hi) = heights.fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
        assert_eq!(lo, 0.0);
        assert_eq!(
            hi, TIME_HEIGHT,
            "the axis spans the drawn nodes, not the later node left out"
        );
    }
    #[test]
    fn a_rank_reads_back_from_a_claims_distance_at_any_spread() {
        let centre = Vec2::new(301.25, -248.5);
        for spread in [1.0, 0.51, 0.25] {
            for rank in [0, 1, 2, 7, 50, 111, 400] {
                for id in ["a", "fac70000-0000-4000-8000-000000000001", "x9"] {
                    let placed = centre + claim_offset_spread(id, rank, spread);
                    assert_eq!(
                        rank_of(placed.distance(centre), spread),
                        rank,
                        "{id} {rank} {spread}"
                    );
                }
            }
        }
        assert_eq!(claim_offset("a", 9), claim_offset_spread("a", 9, 1.0));
        let near = claim_offset_spread("a", 9, 0.5).length();
        assert!((near - (CLAIM_CLEARANCE + CLAIM_SPACING * 0.5 * 3.0)).abs() < 1e-4);
    }

    /// The small graph's topics on a map: t1 named, t2 not.
    fn small_map() -> std::collections::BTreeMap<String, [f32; 2]> {
        [("t1".to_owned(), [12.0, -30.0])].into_iter().collect()
    }

    #[test]
    fn on_a_map_its_topics_stand_where_it_says_and_the_rest_on_a_ring_outside_it() {
        let (nodes, edges) = small_graph();
        let topics = small_map();
        let map = MapPlaces {
            topics: &topics,
            spread: 0.5,
            radius: 300.0,
        };
        let layout = place_seeded(&nodes, &edges, None, Some(map));
        assert_eq!(ground(position(&layout, "t1")), Vec2::new(12.0, -30.0));
        let t2 = ground(position(&layout, "t2"));
        assert!((t2.length() - 300.0).abs() < 1e-3, "{t2}");
        assert_eq!(layout.ring.next, 1, "only t2 took a slot");
        assert_eq!(layout.spread, Spread(0.5));
        let f1 = ground(position(&layout, "f1"));
        assert!((f1.distance(Vec2::new(12.0, -30.0)) - CLAIM_CLEARANCE).abs() < 1e-4);
        let i1 = ground(position(&layout, "i1"));
        assert!(
            (i1.distance(Vec2::new(12.0, -30.0)) - claim_offset_spread("i1", 1, 0.5).length())
                .abs()
                < 1e-4
        );
        let f3 = ground(position(&layout, "f3"));
        assert!(f3.x > 300.0, "the unfiled region is outside the map: {f3}");
        assert_eq!(
            place_seeded(&nodes, &edges, None, Some(map)),
            layout,
            "the same snapshot on the same map, the same picture"
        );
    }

    #[test]
    fn claims_follow_their_topic_to_the_map_where_placing_on_the_map_puts_them() {
        let (nodes, edges) = small_graph();
        let circle = place(&nodes, &edges);
        let topics = small_map();
        let on_map = place_seeded(
            &nodes,
            &edges,
            None,
            Some(MapPlaces {
                topics: &topics,
                spread: 0.5,
                radius: circle.ring.radius,
            }),
        );
        let ids: Vec<String> = circle.nodes.iter().map(|n| n.node_id.clone()).collect();
        let kinds: Vec<Kind> = circle.nodes.iter().map(|n| n.kind).collect();
        let by_id: HashMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let filing: Vec<Option<usize>> = circle
            .nodes
            .iter()
            .map(|n| n.topic.as_deref().map(|t| by_id[t]))
            .collect();
        let grounds: Vec<Vec2> = circle.nodes.iter().map(|n| ground(n.position)).collect();
        let moved: HashMap<usize, Vec2> = [(by_id["t1"], Vec2::new(12.0, -30.0))].into();
        let unfiled = unfiled_centre(&circle.ring);
        let moves = follow(
            &ids,
            &kinds,
            &filing,
            &grounds,
            &moved,
            Arrangement {
                spread: 1.0,
                unfiled,
            },
            Arrangement {
                spread: 0.5,
                unfiled,
            },
        );
        let mut after = grounds.clone();
        for &(i, place) in &moves {
            after[i] = place;
        }
        for id in ["t1", "f1", "i1", "f3"] {
            assert_eq!(after[by_id[id]], ground(position(&on_map, id)), "{id}");
        }
        // t2 is on no map and stays, and its claim closes in at the new spread.
        let (t2, f2) = (by_id["t2"], by_id["f2"]);
        assert_eq!(after[t2], grounds[t2]);
        assert!(!moves.iter().any(|&(i, _)| i == t2));
        assert!((after[f2].distance(after[t2]) - CLAIM_CLEARANCE).abs() < 1e-4);
        // Nothing moves and nothing respreads: nothing is listed.
        let still = Arrangement {
            spread: 1.0,
            unfiled,
        };
        assert!(
            follow(
                &ids,
                &kinds,
                &filing,
                &grounds,
                &HashMap::new(),
                still,
                still
            )
            .is_empty()
        );
    }
}
