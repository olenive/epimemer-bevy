//! What the snapshot says about each drawn node, gathered once: the lookups
//! the filters, the focus and the labels use.
//!
//! A node is named by its index in the layout, the index its disc carries.
//! Every list here is in that order. Membership edges (DESIGN.md §3) become
//! keys: a node's metacontexts, its tags, the topic it is filed under, its
//! source documents, and for a topic its subtopics and the topics tagged
//! with it. An edge whose far end is
//! not a drawn node is kept where its far end is a key in its own right (a
//! metacontext, a document) and dropped where it would have to be a node (a
//! tag or a subtopic pointing at a topic that is not drawn). The edges drawn
//! as lines, claim to claim and succession, are classified and listed per
//! node in [`EdgeIndex`] (`edge_index.rs`).

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::Resource;

use crate::edge_index::{self, EdgeIndex};
use crate::hub::{EdgeView, NodeView};
use crate::layout::{Kind, Layout, TimeAxis};
use crate::time_axis::TimeMode;

/// The index, built by [`build`] and kept as a resource.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct GraphIndex {
    /// Each drawn node's id.
    pub ids: Vec<String>,
    /// Each drawn node's content: a topic's name, a claim's text.
    pub names: Vec<String>,
    /// Each drawn node as the hub last sent it, every field of the
    /// contract's NodeView, for the side panel (`panel.rs`).
    pub views: Vec<NodeView>,
    /// The index of each drawn node, by id.
    pub by_id: HashMap<String, usize>,
    pub kinds: Vec<Kind>,
    /// Whether the node's status is `active`. Every node in a snapshot is;
    /// a node arriving later on the event stream may not be.
    pub active: Vec<bool>,
    /// The node's height on the time axis: its disc's, or the middle of its
    /// column.
    pub heights: Vec<f32>,
    /// The stretch of the time axis the node spans, low end first: a
    /// disc's is its height at both ends. `None` for a node with no
    /// position in the time mode drawn, which is not drawn.
    pub spans: Vec<Option<(f32, f32)>>,
    /// The time axis the heights are on, and the time mode it is in. The
    /// heights, the spans and the axis are rewritten on a change of time
    /// mode (`time_mode.rs`); everything else is the snapshot's.
    pub time: TimeAxis,
    pub mode: TimeMode,
    /// The metacontext ids the node's `has_metacontext` edges point at, in
    /// order, each once.
    pub metacontexts: Vec<Vec<String>>,
    /// The topics the node's `tagged_with_topic` edges point at, as indices,
    /// in order, each once.
    pub tags: Vec<Vec<usize>>,
    /// For a claim, the topic the layout placed it beside: its filing topic,
    /// or its earliest tag where it has no filing edge. `None` for a topic
    /// and for an unfiled claim.
    pub filing: Vec<Option<usize>>,
    /// For a topic, its direct subtopics: the topics whose `subtopic_of`
    /// edge points at it. Empty for a claim.
    pub subtopics: Vec<Vec<usize>>,
    /// For a topic, the other topics whose `tagged_with_topic` edge points
    /// at it. Empty for a claim.
    pub tagged_topics: Vec<Vec<usize>>,
    /// The document ids the node's `sourced_from` edges point at, in order,
    /// each once.
    pub sources: Vec<Vec<String>>,
    /// How many edges of the snapshot touch the node, of every type: an
    /// edge counts once at each end that is a drawn node, and a loop from
    /// a node to itself counts once. The labels rank by it.
    pub degrees: Vec<u32>,
    /// The node's confidence, from 0 to 1, where it carries one: a topic
    /// never does, and a claim may not.
    pub confidence: Vec<Option<f32>>,
    /// The claim-to-claim and succession edges, and those touching each
    /// node.
    pub edges: EdgeIndex,
}

/// Builds the index of a snapshot as the layout placed it. Only the nodes
/// the layout drew are indexed, under the indices it gave them.
pub fn build(nodes: &[NodeView], edges: &[EdgeView], layout: &Layout) -> GraphIndex {
    let count = layout.nodes.len();
    let ids: Vec<String> = layout.nodes.iter().map(|n| n.node_id.clone()).collect();
    let by_id: HashMap<String, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i))
        .collect();
    // The layout draws a repeated id once, as it first appears, so the
    // status and the content are read from that first appearance too.
    let mut first: HashMap<&str, &NodeView> = HashMap::new();
    for node in nodes {
        first.entry(node.node_id.as_str()).or_insert(node);
    }
    let kinds: Vec<Kind> = layout.nodes.iter().map(|n| n.kind).collect();
    let is_topic = |i: usize| kinds[i] == Kind::Topic;

    let mut metacontexts = vec![BTreeSet::new(); count];
    let mut tags = vec![BTreeSet::new(); count];
    let mut subtopics = vec![BTreeSet::new(); count];
    let mut tagged_topics = vec![BTreeSet::new(); count];
    let mut sources = vec![BTreeSet::new(); count];
    let mut degrees = vec![0_u32; count];
    for edge in edges {
        let src = by_id.get(&edge.src_id).copied();
        let dst = by_id.get(&edge.dst_id).copied();
        for end in [src, dst.filter(|&d| Some(d) != src)].into_iter().flatten() {
            degrees[end] = degrees[end].saturating_add(1);
        }
        let Some(src) = src else {
            continue;
        };
        match edge.edge_type.as_str() {
            "has_metacontext" => {
                metacontexts[src].insert(edge.dst_id.clone());
            }
            "sourced_from" => {
                sources[src].insert(edge.dst_id.clone());
            }
            "tagged_with_topic" => {
                if let Some(topic) = dst.filter(|&t| is_topic(t)) {
                    tags[src].insert(topic);
                    if is_topic(src) && src != topic {
                        tagged_topics[topic].insert(src);
                    }
                }
            }
            "subtopic_of" => {
                if let Some(parent) = dst.filter(|&t| is_topic(t) && is_topic(src) && t != src) {
                    subtopics[parent].insert(src);
                }
            }
            _ => {}
        }
    }
    GraphIndex {
        active: ids
            .iter()
            .map(|id| first.get(id.as_str()).is_some_and(|n| n.status == "active"))
            .collect(),
        names: ids
            .iter()
            .map(|id| {
                first
                    .get(id.as_str())
                    .map_or_else(String::new, |n| n.content.clone())
            })
            .collect(),
        views: ids
            .iter()
            .filter_map(|id| first.get(id.as_str()).map(|&n| n.clone()))
            .collect(),
        heights: layout.nodes.iter().map(|n| n.position.y).collect(),
        spans: layout
            .nodes
            .iter()
            .map(|n| Some((n.position.y, n.position.y)))
            .collect(),
        time: layout.time,
        mode: TimeMode::Recorded,
        metacontexts: into_vecs(metacontexts),
        tags: into_vecs(tags),
        filing: layout
            .nodes
            .iter()
            .map(|n| n.topic.as_ref().and_then(|t| by_id.get(t).copied()))
            .collect(),
        subtopics: into_vecs(subtopics),
        tagged_topics: into_vecs(tagged_topics),
        sources: into_vecs(sources),
        degrees,
        confidence: ids
            .iter()
            .map(|id| {
                first
                    .get(id.as_str())
                    .and_then(|n| n.confidence)
                    .filter(|c| c.is_finite())
                    .map(|c| c.clamp(0.0, 1.0) as f32)
            })
            .collect(),
        edges: edge_index::build(edges, &by_id, count),
        kinds,
        ids,
        by_id,
    }
}

/// The stretch of the time axis node `i` spans, low end first, or `None`
/// where it has no position. An index with no spans gives each node its
/// height at both ends.
pub fn extent(index: &GraphIndex, i: usize) -> Option<(f32, f32)> {
    match index.spans.get(i) {
        Some(span) => *span,
        None => index.heights.get(i).map(|&h| (h, h)),
    }
}

/// Sets as sorted lists.
fn into_vecs<T: Ord>(sets: Vec<BTreeSet<T>>) -> Vec<Vec<T>> {
    sets.into_iter().map(Vec::from_iter).collect()
}

/// The topics a topic filter covers, as indices: the topic chosen and
/// every topic below it, and the topics tagged with the chosen one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Subtree {
    /// The chosen topic and every topic below it through `subtopic_of`, each
    /// once. A claim filed under or tagged with any of them is inside.
    pub below: BTreeSet<usize>,
    /// The topics with a `tagged_with_topic` edge to the chosen topic and no
    /// place in `below`. A tag is followed one level only, so a topic tagged
    /// with one of these is outside, and so are their subtopics. A claim is
    /// inside through one of these only when it is filed under it.
    pub tagged: BTreeSet<usize>,
}

impl Subtree {
    /// Whether a topic is inside, either way.
    pub fn holds_topic(&self, topic: usize) -> bool {
        self.below.contains(&topic) || self.tagged.contains(&topic)
    }
}

/// The subtree of a topic: below it, every topic through `subtopic_of`,
/// followed down every level, and beside it, one level of topics tagged
/// with it. A topic with neither is its own subtree. A cycle in the
/// hierarchy ends where it meets a topic already visited. An index that is
/// not a topic gives an empty subtree.
pub fn subtree(index: &GraphIndex, topic: usize) -> Subtree {
    let mut below = BTreeSet::new();
    if index.kinds.get(topic) != Some(&Kind::Topic) {
        return Subtree::default();
    }
    let mut stack = vec![topic];
    while let Some(next) = stack.pop() {
        if below.insert(next) {
            stack.extend(index.subtopics[next].iter().copied());
        }
    }
    let tagged = index.tagged_topics[topic]
        .iter()
        .copied()
        .filter(|t| !below.contains(t))
        .collect();
    Subtree { below, tagged }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hub::{NodeType, parse_snapshot};
    use crate::layout::place;

    pub(crate) fn node(id: &str, node_type: NodeType, day: u32) -> NodeView {
        NodeView {
            node_id: id.into(),
            node_type,
            content: id.into(),
            description: String::new(),
            status: "active".into(),
            source_id: None,
            extraction_method: "unspecified".into(),
            confidence: None,
            retrieved_at: None,
            created_at: format!("2026-09-{day:02}T12:00:00Z"),
            graph: "test".into(),
            metadata: Default::default(),
        }
    }

    pub(crate) fn edge(src: &str, dst: &str, edge_type: &str) -> EdgeView {
        EdgeView {
            edge_id: format!("{src}-{edge_type}-{dst}"),
            src_id: src.into(),
            dst_id: dst.into(),
            edge_type: edge_type.into(),
            weight: 1.0,
            validity: Vec::new(),
            created_at: "2026-09-01T12:00:00Z".into(),
            graph: "test".into(),
            metadata: Default::default(),
        }
    }

    #[test]
    fn membership_edges_become_keys_in_layout_order() {
        let nodes = vec![
            node("parent", NodeType::Topic, 1),
            node("child", NodeType::Topic, 2),
            node("f", NodeType::Fact, 3),
            node("i", NodeType::Inference, 4),
        ];
        let edges = vec![
            edge("child", "parent", "subtopic_of"),
            edge("f", "child", "extracted_under_topic"),
            edge("f", "parent", "tagged_with_topic"),
            edge("f", "parent", "tagged_with_topic"),
            edge("f", "gone", "tagged_with_topic"),
            edge("f", "the-real", "has_metacontext"),
            edge("f", "doc-b", "sourced_from"),
            edge("f", "doc-a", "sourced_from"),
            edge("i", "f", "tagged_with_topic"),
            edge("parent", "parent", "subtopic_of"),
            edge("gone", "parent", "subtopic_of"),
        ];
        let layout = place(&nodes, &edges);
        let index = build(&nodes, &edges, &layout);
        let at = |id: &str| index.by_id[id];
        assert_eq!(index.ids.len(), 4);
        assert_eq!(index.ids[at("f")], "f");
        assert_eq!(index.kinds[at("i")], Kind::Inference);
        assert_eq!(
            index.tags[at("f")],
            [at("parent")],
            "once, drawn topics only"
        );
        assert!(
            index.tags[at("i")].is_empty(),
            "a tag must point at a topic"
        );
        assert_eq!(index.filing[at("f")], Some(at("child")));
        assert_eq!(index.filing[at("i")], None, "unfiled");
        assert_eq!(index.filing[at("parent")], None);
        assert_eq!(index.subtopics[at("parent")], [at("child")], "no self loop");
        assert!(
            index.tagged_topics.iter().all(Vec::is_empty),
            "f tags parent, a claim"
        );
        assert_eq!(index.names[at("f")], "f");
        assert_eq!(index.views.len(), 4);
        assert_eq!(index.views[at("i")], nodes[3], "the whole view");
        assert_eq!(index.metacontexts[at("f")], ["the-real"]);
        assert_eq!(index.sources[at("f")], ["doc-a", "doc-b"]);
        assert_eq!(index.degrees[at("parent")], 5);
        assert_eq!(index.degrees[at("f")], 8);
        assert_eq!(index.degrees[at("i")], 1);
        assert!(index.active.iter().all(|&a| a));
        assert_eq!(index.heights[at("parent")], 0.0);
        assert_eq!(index.time, layout.time);
    }

    #[test]
    fn a_degree_counts_every_edge_touching_the_node_once() {
        let mut sure = node("f", NodeType::Fact, 3);
        sure.confidence = Some(0.9);
        let mut odd = node("g", NodeType::Fact, 4);
        odd.confidence = Some(f64::NAN);
        let nodes = vec![node("t", NodeType::Topic, 1), sure, odd];
        let edges = vec![
            edge("f", "t", "extracted_under_topic"),
            edge("f", "t", "tagged_with_topic"),
            edge("f", "the-real", "has_metacontext"),
            edge("segment", "f", "contains"),
            edge("t", "t", "related"),
            edge("g", "f", "supports"),
        ];
        let index = build(&nodes, &edges, &place(&nodes, &edges));
        let at = |id: &str| index.by_id[id];
        assert_eq!(index.degrees[at("t")], 3, "a loop counts once");
        assert_eq!(
            index.degrees[at("f")],
            5,
            "a far end that is not a node counts at the near end"
        );
        assert_eq!(index.degrees[at("g")], 1);
        assert_eq!(index.confidence[at("f")], Some(0.9));
        assert_eq!(index.confidence[at("t")], None);
        assert_eq!(
            index.confidence[at("g")],
            None,
            "a value that is not a number"
        );
    }

    #[test]
    fn a_status_other_than_active_is_recorded() {
        let mut gone = node("old", NodeType::Fact, 2);
        gone.status = "corrected".into();
        let nodes = vec![node("t", NodeType::Topic, 1), gone];
        let index = build(&nodes, &[], &place(&nodes, &[]));
        assert_eq!(index.active, [true, false]);
    }

    #[test]
    fn a_subtree_runs_down_every_level_and_survives_a_cycle() {
        let nodes: Vec<NodeView> = ["a", "b", "c", "d", "lone"]
            .iter()
            .enumerate()
            .map(|(i, id)| node(id, NodeType::Topic, 1 + i as u32))
            .collect();
        let edges = vec![
            edge("b", "a", "subtopic_of"),
            edge("c", "b", "subtopic_of"),
            edge("d", "c", "subtopic_of"),
            // A cycle: a below d as well.
            edge("a", "d", "subtopic_of"),
        ];
        let index = build(&nodes, &edges, &place(&nodes, &edges));
        let at = |id: &str| index.by_id[id];
        let ids = |set: BTreeSet<usize>| -> Vec<&str> {
            set.into_iter().map(|i| index.ids[i].as_str()).collect()
        };
        let b = subtree(&index, at("b"));
        assert_eq!(ids(b.below), ["a", "b", "c", "d"]);
        assert!(b.tagged.is_empty());
        let lone = subtree(&index, at("lone"));
        assert_eq!(ids(lone.below), ["lone"], "its own subtree");
        assert_eq!(subtree(&index, 99), Subtree::default());
    }

    #[test]
    fn a_topic_tagged_with_the_chosen_one_is_inside_one_level_deep() {
        // chosen has a subtopic, sub; near is tagged with chosen and has a
        // subtopic of its own, nearsub; far is tagged with near; sub2 is
        // below chosen and tagged with it too; a claim tags chosen.
        let nodes = vec![
            node("chosen", NodeType::Topic, 1),
            node("sub", NodeType::Topic, 2),
            node("near", NodeType::Topic, 3),
            node("nearsub", NodeType::Topic, 4),
            node("far", NodeType::Topic, 5),
            node("sub2", NodeType::Topic, 6),
            node("claim", NodeType::Fact, 7),
        ];
        let edges = vec![
            edge("sub", "chosen", "subtopic_of"),
            edge("sub2", "chosen", "subtopic_of"),
            edge("near", "chosen", "tagged_with_topic"),
            edge("nearsub", "near", "subtopic_of"),
            edge("far", "near", "tagged_with_topic"),
            edge("sub2", "chosen", "tagged_with_topic"),
            edge("chosen", "chosen", "tagged_with_topic"),
            edge("claim", "chosen", "tagged_with_topic"),
        ];
        let index = build(&nodes, &edges, &place(&nodes, &edges));
        let at = |id: &str| index.by_id[id];
        let ids = |set: &BTreeSet<usize>| -> Vec<&str> {
            set.iter().map(|&i| index.ids[i].as_str()).collect()
        };
        assert_eq!(
            ids(&index.tagged_topics[at("chosen")].iter().copied().collect()),
            ["near", "sub2"],
            "topics only, and no self loop"
        );
        let chosen = subtree(&index, at("chosen"));
        assert_eq!(ids(&chosen.below), ["chosen", "sub", "sub2"]);
        assert_eq!(
            ids(&chosen.tagged),
            ["near"],
            "sub2 is below already; far and nearsub are a level further"
        );
        assert!(chosen.holds_topic(at("near")) && !chosen.holds_topic(at("far")));
        let near = subtree(&index, at("near"));
        assert_eq!(ids(&near.below), ["near", "nearsub"]);
        assert_eq!(ids(&near.tagged), ["far"]);
    }

    #[test]
    fn the_fixture_indexes_its_metacontext_and_document() {
        let snapshot = parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        let layout = place(&snapshot.nodes, &snapshot.edges);
        let index = build(&snapshot.nodes, &snapshot.edges, &layout);
        let fact = index.by_id["fac70000-0000-4000-8000-000000000001"];
        assert_eq!(index.metacontexts[fact], ["the-real"]);
        assert_eq!(
            index.sources[fact],
            ["d0c00000-0000-4000-8000-000000000001"]
        );
        let swifts = index.by_id["a1e0c7d2-0000-4000-8000-000000000002"];
        assert_eq!(index.filing[fact], Some(swifts));
        assert_eq!(index.ids.len(), 5, "the node of unknown kind is not drawn");
    }
}
