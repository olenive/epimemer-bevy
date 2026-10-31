//! What the bar's pickers list, built once per graph from the index and
//! the snapshot's metacontexts.
//!
//! Each picker is a plain list, worked out by a pure function so that the
//! lists can be checked without a window: the topics by name, the source
//! documents by how many nodes were read from them, and the metacontexts
//! by name. The bar (`bar.rs`) draws them and sets the filter from the
//! entry chosen.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::Resource;

use crate::graph_index::GraphIndex;
use crate::hub::MetacontextView;
use crate::layout::Kind;

/// How many characters of an id the bar shows.
pub const SHORT_ID: usize = 8;

/// A topic the topic picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicEntry {
    /// The topic node's index in the layout.
    pub index: usize,
    pub id: String,
    /// What the picker shows: the topic's name, with its short id after it
    /// where another drawn topic has the same name.
    pub label: String,
}

/// A document the source picker offers: the target of `sourced_from`
/// edges. The snapshot carries no document names, a known gap in the
/// contract: VIZ_CONTRACT.md §5.1 gives a document only as the far end of
/// an edge. So a document is shown by its short id and the count of drawn
/// nodes read from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEntry {
    pub id: String,
    /// How many drawn nodes have a `sourced_from` edge to it.
    pub nodes: usize,
    pub label: String,
}

/// A metacontext the metacontext picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetacontextEntry {
    pub id: String,
    /// Its name, or its id where the snapshot does not list it.
    pub name: String,
    /// How many drawn nodes have a `has_metacontext` edge to it.
    pub nodes: usize,
}

/// Every picker's list for the graph drawn.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct Pickers {
    pub topics: Vec<TopicEntry>,
    pub sources: Vec<SourceEntry>,
    pub metacontexts: Vec<MetacontextEntry>,
}

/// The first [`SHORT_ID`] characters of an id.
pub fn short_id(id: &str) -> &str {
    id.char_indices()
        .nth(SHORT_ID)
        .map_or(id, |(end, _)| &id[..end])
}

/// Every picker's list.
pub fn build(index: &GraphIndex, metacontexts: &[MetacontextView]) -> Pickers {
    Pickers {
        topics: topics(index),
        sources: sources(index),
        metacontexts: metacontext_entries(index, metacontexts),
    }
}

/// Every drawn topic, by name ignoring case, then by id. A name two topics
/// share gets the short id after it, so the two can be told apart.
pub fn topics(index: &GraphIndex) -> Vec<TopicEntry> {
    let topics: Vec<usize> = (0..index.ids.len())
        .filter(|&i| index.kinds[i] == Kind::Topic)
        .collect();
    let mut uses: BTreeMap<&str, usize> = BTreeMap::new();
    for &i in &topics {
        *uses.entry(index.names[i].as_str()).or_default() += 1;
    }
    let mut entries: Vec<TopicEntry> = topics
        .into_iter()
        .map(|i| {
            let (id, name) = (&index.ids[i], &index.names[i]);
            let label = match (name.trim().is_empty(), uses[name.as_str()] > 1) {
                (true, _) => short_id(id).to_owned(),
                (false, true) => format!("{name} ({})", short_id(id)),
                (false, false) => name.clone(),
            };
            TopicEntry {
                index: i,
                id: id.clone(),
                label,
            }
        })
        .collect();
    entries.sort_by_cached_key(|e| (e.label.to_lowercase(), e.id.clone()));
    entries
}

/// Whether a label starts with `prefix`, ignoring case and the spaces
/// around the prefix. An empty prefix matches every label.
pub fn matches(label: &str, prefix: &str) -> bool {
    label
        .to_lowercase()
        .starts_with(&prefix.trim().to_lowercase())
}

/// The topics whose label [`matches`] the prefix, in their order.
pub fn matching<'a>(topics: &'a [TopicEntry], prefix: &str) -> Vec<&'a TopicEntry> {
    topics
        .iter()
        .filter(|t| matches(&t.label, prefix))
        .collect()
}

/// Every document a drawn node was read from, the most read first, then
/// by id.
pub fn sources(index: &GraphIndex) -> Vec<SourceEntry> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for document in index.sources.iter().flatten() {
        *counts.entry(document.as_str()).or_default() += 1;
    }
    let mut entries: Vec<SourceEntry> = counts
        .into_iter()
        .map(|(id, nodes)| SourceEntry {
            id: id.to_owned(),
            nodes,
            label: format!(
                "{}  {nodes} {}",
                short_id(id),
                if nodes == 1 { "node" } else { "nodes" }
            ),
        })
        .collect();
    entries.sort_by(|a, b| b.nodes.cmp(&a.nodes).then_with(|| a.id.cmp(&b.id)));
    entries
}

/// Every metacontext the snapshot lists, and any other a drawn node points
/// at, by name ignoring case, then by id.
pub fn metacontext_entries(
    index: &GraphIndex,
    metacontexts: &[MetacontextView],
) -> Vec<MetacontextEntry> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for id in index.metacontexts.iter().flatten() {
        *counts.entry(id.as_str()).or_default() += 1;
    }
    let names: BTreeMap<&str, &str> = metacontexts
        .iter()
        .map(|m| (m.metacontext_id.as_str(), m.content.as_str()))
        .collect();
    let ids: BTreeSet<&str> = names.keys().chain(counts.keys()).copied().collect();
    let mut entries: Vec<MetacontextEntry> = ids
        .into_iter()
        .map(|id| MetacontextEntry {
            id: id.to_owned(),
            name: names
                .get(id)
                .filter(|name| !name.trim().is_empty())
                .map_or(id, |name| name)
                .to_owned(),
            nodes: counts.get(id).copied().unwrap_or(0),
        })
        .collect();
    entries.sort_by_cached_key(|e| (e.name.to_lowercase(), e.id.clone()));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_index::tests::{edge, node};
    use crate::graph_index::{self};
    use crate::hub::{NodeType, parse_snapshot};
    use crate::layout::place;

    fn fixture() -> Pickers {
        let snapshot = parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        let layout = place(&snapshot.nodes, &snapshot.edges);
        let index = graph_index::build(&snapshot.nodes, &snapshot.edges, &layout);
        build(&index, &snapshot.metacontexts)
    }

    #[test]
    fn the_fixture_lists_its_topics_document_and_metacontext() {
        let pickers = fixture();
        let topics: Vec<(&str, &str)> = pickers
            .topics
            .iter()
            .map(|t| (t.label.as_str(), t.id.as_str()))
            .collect();
        assert_eq!(
            topics,
            [
                ("Bird migration", "a1e0c7d2-0000-4000-8000-000000000001"),
                ("Swifts", "a1e0c7d2-0000-4000-8000-000000000002"),
            ]
        );
        assert_eq!(pickers.topics[1].index, 1, "the index the disc carries");
        assert_eq!(
            pickers.sources,
            [SourceEntry {
                id: "d0c00000-0000-4000-8000-000000000001".into(),
                nodes: 1,
                label: "d0c00000  1 node".into(),
            }]
        );
        assert_eq!(
            pickers.metacontexts,
            [MetacontextEntry {
                id: "the-real".into(),
                name: "The Real".into(),
                nodes: 1,
            }]
        );
    }

    #[test]
    fn topics_sort_by_name_and_a_shared_name_shows_its_short_id() {
        let mut nodes = vec![
            node("t-zebra", NodeType::Topic, 1),
            node("t-apple-2", NodeType::Topic, 2),
            node("t-apple-1", NodeType::Topic, 3),
            node("t-blank", NodeType::Topic, 4),
            node("claim", NodeType::Fact, 5),
        ];
        nodes[0].content = "zebra".into();
        nodes[1].content = "Apple".into();
        nodes[2].content = "Apple".into();
        nodes[3].content = " ".into();
        let index = graph_index::build(&nodes, &[], &place(&nodes, &[]));
        let labels: Vec<String> = topics(&index).into_iter().map(|t| t.label).collect();
        assert_eq!(
            labels,
            ["Apple (t-apple-)", "Apple (t-apple-)", "t-blank", "zebra"],
            "no claim; a blank name shows the id"
        );
        let entries = topics(&index);
        assert_eq!(entries[0].id, "t-apple-1", "the same label sorts by id");
        let found: Vec<&str> = matching(&entries, " ap ")
            .into_iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(found, ["t-apple-1", "t-apple-2"]);
        assert_eq!(matching(&entries, "").len(), 4);
        assert!(
            matching(&entries, "pple").is_empty(),
            "a prefix, not a substring"
        );
    }

    #[test]
    fn sources_count_the_nodes_read_from_each_and_list_the_most_read_first() {
        let nodes = vec![
            node("t", NodeType::Topic, 1),
            node("a", NodeType::Fact, 2),
            node("b", NodeType::Fact, 3),
        ];
        let edges = vec![
            edge("a", "doc-one-0000", "sourced_from"),
            edge("a", "doc-two-0000", "sourced_from"),
            edge("b", "doc-two-0000", "sourced_from"),
            edge("b", "doc-two-0000", "sourced_from"),
        ];
        let index = graph_index::build(&nodes, &edges, &place(&nodes, &edges));
        let entries = sources(&index);
        let listed: Vec<(&str, usize, &str)> = entries
            .iter()
            .map(|s| (s.id.as_str(), s.nodes, s.label.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("doc-two-0000", 2, "doc-two-  2 nodes"),
                ("doc-one-0000", 1, "doc-one-  1 node")
            ],
            "an edge repeated counts its node once"
        );
        assert_eq!(short_id("short"), "short");
    }

    #[test]
    fn metacontexts_list_the_snapshots_and_any_other_a_node_points_at() {
        let nodes = vec![node("t", NodeType::Topic, 1), node("a", NodeType::Fact, 2)];
        let edges = vec![
            edge("a", "the-real", "has_metacontext"),
            edge("t", "the-real", "has_metacontext"),
            edge("a", "unlisted", "has_metacontext"),
        ];
        let index = graph_index::build(&nodes, &edges, &place(&nodes, &edges));
        let view = |id: &str, name: &str| MetacontextView {
            metacontext_id: id.into(),
            content: name.into(),
            description: String::new(),
            graph: "test".into(),
        };
        let entries = metacontext_entries(
            &index,
            &[
                view("the-real", "The Real"),
                view("night-city", "Night City"),
            ],
        );
        let listed: Vec<(&str, &str, usize)> = entries
            .iter()
            .map(|m| (m.id.as_str(), m.name.as_str(), m.nodes))
            .collect();
        assert_eq!(
            listed,
            [
                ("night-city", "Night City", 0),
                ("the-real", "The Real", 2),
                ("unlisted", "unlisted", 1)
            ]
        );
    }
}
