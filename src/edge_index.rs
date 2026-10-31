//! The edges the scene draws as lines, classified once from the snapshot
//! (DESIGN.md §3 and §5, `VIZ_CONTRACT.md` §5.3).
//!
//! Every edge type falls into one of four classes. Membership edges say
//! where a node belongs and are never lines: `graph_index.rs` turns them
//! into keys. A `timelink` runs to a timeline, not to a node, and is read
//! with claim time. Claim-to-claim edges are lines on hover, on focus and
//! with the edge toggle. Succession and merge edges, where one node gave
//! way to another, are connectors that are always drawn. An edge type this
//! version does not know is a claim-to-claim line it can draw and not
//! name, as the contract asks.
//!
//! An edge of the last two classes is kept when at least one end is a
//! drawn node, and listed under every drawn node it touches, so that what
//! hovering or focusing a node draws costs as much as the node's degree.
//! An end that is not a drawn node, an inactive node most often, keeps its
//! id, so that the side panel can name it; the snapshot holds active nodes
//! only, so the old end of a succession is usually one of these.

use std::collections::HashMap;

use crate::hub::{EdgeView, parse_timestamp};

/// How the scene treats an edge type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeClass {
    /// One end files the other under something: a key, never a line.
    Membership,
    /// A node to a timeline, read with claim time.
    Timelink,
    /// Between two claims: a line on hover, on focus and with the toggle.
    Claim(Relation),
    /// One node gave way to another: a connector, always drawn.
    Succession(Succession),
}

/// The claim-to-claim relations, each drawn in its own colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Relation {
    Supports,
    Assessed,
    Related,
    Similarity,
    Contradiction,
    /// `derived_from`, `variant_of`, `supersession_candidate`, and any type
    /// this version does not know.
    Other,
}

/// The succession and merge edges. Each runs from the old node to the new:
/// the node corrected, followed or merged away to the one that took its
/// place, or a retired fact to the inference that depended on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Succession {
    /// `superseded_by`: a correction.
    Superseded,
    /// `temporally_followed_by`: the claim was right and the world moved on.
    FollowedBy,
    /// `merged_into`: a merged source to the survivor.
    Merged,
    /// `evidence_superseded`: a corrected fact to the inference it supported.
    EvidenceSuperseded,
    /// `evidence_merged`: a merged fact to the inference it supported.
    EvidenceMerged,
}

impl Succession {
    /// Whether the edge runs from a fact to an inference that depended on
    /// it, which is drawn in the second connector colour.
    pub fn is_evidence(self) -> bool {
        matches!(
            self,
            Succession::EvidenceSuperseded | Succession::EvidenceMerged
        )
    }
}

/// The class of an edge type.
pub fn classify(edge_type: &str) -> EdgeClass {
    match edge_type {
        "tagged_with_topic"
        | "has_metacontext"
        | "sourced_from"
        | "contains"
        | "extracted_under_topic"
        | "implies"
        | "abstracts"
        | "about"
        | "subtopic_of" => EdgeClass::Membership,
        "timelink" => EdgeClass::Timelink,
        "supports" => EdgeClass::Claim(Relation::Supports),
        "assessed" => EdgeClass::Claim(Relation::Assessed),
        "related" => EdgeClass::Claim(Relation::Related),
        "similarity" => EdgeClass::Claim(Relation::Similarity),
        "contradiction" => EdgeClass::Claim(Relation::Contradiction),
        "superseded_by" => EdgeClass::Succession(Succession::Superseded),
        "temporally_followed_by" => EdgeClass::Succession(Succession::FollowedBy),
        "merged_into" => EdgeClass::Succession(Succession::Merged),
        "evidence_superseded" => EdgeClass::Succession(Succession::EvidenceSuperseded),
        "evidence_merged" => EdgeClass::Succession(Succession::EvidenceMerged),
        _ => EdgeClass::Claim(Relation::Other),
    }
}

/// One end of an edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// A drawn node, by its index in the layout.
    Drawn(usize),
    /// An id that is not a drawn node: an inactive node, a segment or a
    /// document.
    Absent(String),
}

impl End {
    /// The drawn node, if this end is one.
    pub fn drawn(&self) -> Option<usize> {
        match self {
            End::Drawn(node) => Some(*node),
            End::Absent(_) => None,
        }
    }
}

/// A claim-to-claim or succession edge with at least one drawn end.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub id: String,
    pub edge_type: String,
    /// [`EdgeClass::Claim`] or [`EdgeClass::Succession`].
    pub class: EdgeClass,
    /// The edge's source; for a succession, the old node.
    pub src: End,
    /// The edge's destination; for a succession, the new node.
    pub dst: End,
    /// When the edge was recorded, in microseconds since the Unix epoch;
    /// `None` where `created_at` does not parse.
    pub created: Option<i64>,
}

impl Edge {
    /// The end that is not `node`; a loop's far end is `node` itself.
    pub fn far_end(&self, node: usize) -> &End {
        if self.src == End::Drawn(node) {
            &self.dst
        } else {
            &self.src
        }
    }

    /// Whether an end is not a drawn node.
    pub fn has_absent_end(&self) -> bool {
        self.src.drawn().is_none() || self.dst.drawn().is_none()
    }

    /// The relation, for a claim-to-claim edge.
    pub fn relation(&self) -> Option<Relation> {
        match self.class {
            EdgeClass::Claim(relation) => Some(relation),
            _ => None,
        }
    }

    /// The kind of succession, for a succession edge.
    pub fn succession(&self) -> Option<Succession> {
        match self.class {
            EdgeClass::Succession(succession) => Some(succession),
            _ => None,
        }
    }
}

/// The claim-to-claim and succession edges, and the ones touching each
/// drawn node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeIndex {
    /// The edges with at least one drawn end, in the snapshot's order.
    pub edges: Vec<Edge>,
    /// For each drawn node, in layout order, its claim-to-claim edges, as
    /// indices into `edges`, each once.
    pub claim: Vec<Vec<usize>>,
    /// For each drawn node, its succession and merge edges.
    pub succession: Vec<Vec<usize>>,
    /// The edges of either class left out because neither end is drawn.
    pub undrawn: usize,
}

/// Classifies the snapshot's edges and lists them under the drawn nodes
/// they touch. `by_id` gives each drawn node's index, out of `count`.
pub fn build(edges: &[EdgeView], by_id: &HashMap<String, usize>, count: usize) -> EdgeIndex {
    let mut index = EdgeIndex {
        claim: vec![Vec::new(); count],
        succession: vec![Vec::new(); count],
        ..EdgeIndex::default()
    };
    let end = |id: &str| {
        by_id
            .get(id)
            .copied()
            .filter(|&i| i < count)
            .map_or_else(|| End::Absent(id.to_owned()), End::Drawn)
    };
    for edge in edges {
        let class = classify(&edge.edge_type);
        if !matches!(class, EdgeClass::Claim(_) | EdgeClass::Succession(_)) {
            continue;
        }
        let (src, dst) = (end(&edge.src_id), end(&edge.dst_id));
        if src.drawn().is_none() && dst.drawn().is_none() {
            index.undrawn += 1;
            continue;
        }
        let at = index.edges.len();
        let lists = match class {
            EdgeClass::Claim(_) => &mut index.claim,
            _ => &mut index.succession,
        };
        let (a, b) = (src.drawn(), dst.drawn());
        for node in [a, b.filter(|&b| Some(b) != a)].into_iter().flatten() {
            lists[node].push(at);
        }
        index.edges.push(Edge {
            id: edge.edge_id.clone(),
            edge_type: edge.edge_type.clone(),
            class,
            src,
            dst,
            created: parse_timestamp(&edge.created_at),
        });
    }
    index
}

/// The drawn nodes at the far ends of a node's claim-to-claim edges, each
/// once, without the node itself.
pub fn claim_neighbours(index: &EdgeIndex, node: usize) -> Vec<usize> {
    let mut near: Vec<usize> = index
        .claim
        .get(node)
        .into_iter()
        .flatten()
        .filter_map(|&e| index.edges[e].far_end(node).drawn())
        .filter(|&far| far != node)
        .collect();
    near.sort_unstable();
    near.dedup();
    near
}

/// What the edge index holds, for the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeCounts {
    /// Claim-to-claim edges between two drawn nodes, and those with one end
    /// that is not drawn.
    pub claim_drawn: usize,
    pub claim_absent: usize,
    /// Succession edges between two drawn nodes, those whose old end is not
    /// drawn, and those whose new end is not drawn.
    pub succession_drawn: usize,
    pub succession_old_absent: usize,
    pub succession_new_absent: usize,
    /// Edges of either class with neither end drawn.
    pub undrawn: usize,
}

pub fn counts(index: &EdgeIndex) -> EdgeCounts {
    let mut counts = EdgeCounts {
        undrawn: index.undrawn,
        ..EdgeCounts::default()
    };
    for edge in &index.edges {
        let claim = matches!(edge.class, EdgeClass::Claim(_));
        let slot = match (claim, edge.has_absent_end()) {
            (true, false) => &mut counts.claim_drawn,
            (true, true) => &mut counts.claim_absent,
            (false, false) => &mut counts.succession_drawn,
            (false, true) if edge.src.drawn().is_none() => &mut counts.succession_old_absent,
            (false, true) => &mut counts.succession_new_absent,
        };
        *slot += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_index::tests::edge;

    #[test]
    fn every_edge_type_in_the_vocabulary_has_its_class() {
        let membership = [
            "tagged_with_topic",
            "has_metacontext",
            "sourced_from",
            "contains",
            "extracted_under_topic",
            "implies",
            "abstracts",
            "about",
            "subtopic_of",
        ];
        for edge_type in membership {
            assert_eq!(classify(edge_type), EdgeClass::Membership, "{edge_type}");
        }
        assert_eq!(classify("timelink"), EdgeClass::Timelink);
        for (edge_type, relation) in [
            ("supports", Relation::Supports),
            ("assessed", Relation::Assessed),
            ("related", Relation::Related),
            ("similarity", Relation::Similarity),
            ("contradiction", Relation::Contradiction),
            ("derived_from", Relation::Other),
            ("variant_of", Relation::Other),
            ("supersession_candidate", Relation::Other),
            ("a_type_from_a_later_version", Relation::Other),
        ] {
            assert_eq!(
                classify(edge_type),
                EdgeClass::Claim(relation),
                "{edge_type}"
            );
        }
        for (edge_type, succession, evidence) in [
            ("superseded_by", Succession::Superseded, false),
            ("temporally_followed_by", Succession::FollowedBy, false),
            ("merged_into", Succession::Merged, false),
            ("evidence_superseded", Succession::EvidenceSuperseded, true),
            ("evidence_merged", Succession::EvidenceMerged, true),
        ] {
            assert_eq!(
                classify(edge_type),
                EdgeClass::Succession(succession),
                "{edge_type}"
            );
            assert_eq!(succession.is_evidence(), evidence, "{edge_type}");
        }
    }

    fn ids(names: &[&str]) -> HashMap<String, usize> {
        names
            .iter()
            .enumerate()
            .map(|(i, &n)| (n.to_owned(), i))
            .collect()
    }

    #[test]
    fn each_node_lists_the_edges_touching_it_by_class() {
        // Drawn: a fact f, an inference i, a second fact g, a topic t.
        let by_id = ids(&["f", "i", "g", "t"]);
        let edges = vec![
            edge("f", "i", "supports"),
            edge("g", "i", "supports"),
            edge("f", "g", "assessed"),
            edge("f", "t", "tagged_with_topic"),
            edge("i", "timeline", "timelink"),
            edge("old-f", "f", "superseded_by"),
            edge("retired-fact", "i", "evidence_merged"),
            edge("f", "f", "related"),
            edge("gone-a", "gone-b", "supports"),
            edge("gone-a", "gone-b", "merged_into"),
        ];
        let index = build(&edges, &by_id, 4);
        let types = |list: &[usize]| -> Vec<&str> {
            list.iter()
                .map(|&e| index.edges[e].edge_type.as_str())
                .collect()
        };
        assert_eq!(index.edges.len(), 6, "membership, timelinks, undrawn out");
        assert_eq!(index.undrawn, 2);
        assert_eq!(types(&index.claim[0]), ["supports", "assessed", "related"]);
        assert_eq!(types(&index.claim[1]), ["supports", "supports"]);
        assert_eq!(types(&index.claim[2]), ["supports", "assessed"]);
        assert!(index.claim[3].is_empty() && index.succession[3].is_empty());
        assert_eq!(types(&index.succession[0]), ["superseded_by"]);
        assert_eq!(types(&index.succession[1]), ["evidence_merged"]);
        assert_eq!(claim_neighbours(&index, 0), [1, 2], "the loop is not one");
        assert_eq!(claim_neighbours(&index, 1), [0, 2]);
        assert!(claim_neighbours(&index, 3).is_empty());
        assert!(claim_neighbours(&index, 99).is_empty());
    }

    #[test]
    fn an_end_that_is_not_drawn_keeps_its_id() {
        let by_id = ids(&["new", "i"]);
        let mut late = edge("old", "new", "temporally_followed_by");
        late.created_at = "2026-09-02T00:00:00Z".into();
        let edges = vec![late, edge("i", "retired", "supports")];
        let index = build(&edges, &by_id, 2);
        let succession = &index.edges[index.succession[0][0]];
        assert_eq!(succession.src, End::Absent("old".into()));
        assert_eq!(succession.dst, End::Drawn(0));
        assert!(succession.has_absent_end());
        assert_eq!(succession.far_end(0), &End::Absent("old".into()));
        assert_eq!(succession.succession(), Some(Succession::FollowedBy));
        assert_eq!(
            succession.created,
            parse_timestamp("2026-09-02T00:00:00Z"),
            "the time it was recorded"
        );
        let claim = &index.edges[index.claim[1][0]];
        assert_eq!(claim.far_end(1), &End::Absent("retired".into()));
        assert_eq!(claim.relation(), Some(Relation::Supports));
        assert_eq!(
            counts(&index),
            EdgeCounts {
                claim_absent: 1,
                succession_old_absent: 1,
                ..EdgeCounts::default()
            }
        );
    }

    #[test]
    fn the_counts_tell_present_ends_from_absent_ones() {
        let by_id = ids(&["a", "b"]);
        let edges = vec![
            edge("a", "b", "supports"),
            edge("a", "gone", "similarity"),
            edge("a", "b", "merged_into"),
            edge("gone", "b", "superseded_by"),
            edge("a", "gone", "temporally_followed_by"),
            edge("gone", "also-gone", "assessed"),
        ];
        assert_eq!(
            counts(&build(&edges, &by_id, 2)),
            EdgeCounts {
                claim_drawn: 1,
                claim_absent: 1,
                succession_drawn: 1,
                succession_old_absent: 1,
                succession_new_absent: 1,
                undrawn: 1,
            }
        );
    }
}
