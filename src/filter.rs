//! The filters of DESIGN.md §10, as plain data and pure functions, and the
//! plugin that turns a change of filter into what the scene shows.
//!
//! [`Filter`] is the one resource a control changes: the command line at
//! startup, and the bar along the top (`bar.rs`). A node passes when it
//! meets every criterion that is set. What a node that fails looks like is
//! the filter's [`Mode`]: dimmed, so that the user keeps the shape of the
//! whole (DESIGN.md §8), or hidden, which also takes it out of picking and
//! out of every camera fit. A node with no position in the time mode drawn
//! (a claim whose timepoints have none, in claim time) is hidden whatever
//! the mode, and so is a node that has left the active set on the event
//! stream, which the snapshot would no longer hold; the time window passes
//! a node whose disc or column overlaps it. In claim time the window lies
//! within the claim axis ([`clamp_window`]), and a node clipped to an end of
//! the axis counts as standing at that end (`clip.rs`), so a window that
//! reaches the end passes it.
//!
//! A change reaches the scene through Bevy's change detection. Whatever
//! changes `Filter` through `ResMut<Filter>` marks it changed; in that
//! update's `PostUpdate`, [`RefreshShown`] recomputes [`Shown`], and the
//! scene and picking systems that read `Shown` run after it and act only
//! when it changed. A new [`GraphIndex`], after a graph switch, recomputes
//! it too. An update in which neither changes recomputes nothing and
//! rewrites no disc.

use std::collections::BTreeSet;

use bevy::prelude::*;

use crate::graph_index::{self, GraphIndex, Subtree};
use crate::layout::{Kind, TimeAxis, instant_at, time_y};
use crate::time_axis::TimeMode;

/// How much of its colour a node that fails the filter keeps in Dim mode.
pub const DIM_ALPHA: f32 = 0.12;

/// Which metacontexts pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Metacontexts {
    #[default]
    All,
    /// The nodes with a `has_metacontext` edge to one of these ids. A node
    /// with no metacontext fails.
    Only(BTreeSet<String>),
}

/// Which node kinds pass, each on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinds {
    pub topic: bool,
    pub fact: bool,
    pub inference: bool,
}

impl Default for Kinds {
    fn default() -> Self {
        Kinds {
            topic: true,
            fact: true,
            inference: true,
        }
    }
}

/// Which statuses pass. The snapshot holds active nodes only, so there is
/// one choice today and every node of a snapshot meets it; the bar shows
/// it so that the choice of historical nodes has a place when the contract
/// sends them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    ActiveOnly,
}

/// A stretch of the time axis, in the axis's own units: heights, from 0 at
/// the earliest node to `TIME_HEIGHT` at the latest. Both ends are inside,
/// and they may come in either order, so a handle dragged past the other
/// still makes a window. [`window_between`] and [`window_instants`] convert
/// from and to timestamps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeWindow {
    pub from: f32,
    pub to: f32,
}

/// What a node that fails the filter looks like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Dimmed to [`DIM_ALPHA`], still picked and framed.
    #[default]
    Dim,
    /// Not drawn, not picked, and left out of every camera fit.
    Hide,
}

/// The filter: every criterion of DESIGN.md §10 the scene applies, and the
/// mode. The default lets every node pass.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub metacontexts: Metacontexts,
    pub kinds: Kinds,
    pub status: Status,
    /// A topic node's id: that topic, every topic below it through
    /// `subtopic_of`, and every claim filed under or tagged with any of
    /// them; and, one level out, every topic tagged with the chosen topic
    /// and the claims filed under those (`graph_index::subtree`). An id
    /// that is not a drawn topic lets no node pass.
    pub topic: Option<String>,
    pub window: Option<TimeWindow>,
    /// A document's id, the target of `sourced_from` edges: the nodes read
    /// from that document.
    pub source: Option<String>,
    pub mode: Mode,
}

/// What the filter decided for each node, in layout order. Recomputed only
/// when [`Filter`] changes.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub mode: Mode,
    /// Whether the node passes the filter.
    pub passes: Vec<bool>,
    /// Whether the node is drawn, picked and framed: every node in Dim
    /// mode, the passing ones in Hide mode.
    pub visible: Vec<bool>,
}

/// The system set that recomputes [`Shown`] in `PostUpdate`. A system that
/// reads `Shown` there runs after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefreshShown;

/// Keeps [`Shown`] up to date with [`Filter`]. The app inserts `Filter` and
/// the [`GraphIndex`] before it runs.
pub struct FilterPlugin;

impl Plugin for FilterPlugin {
    fn build(&self, app: &mut App) {
        // Once before anything is spawned, so that the discs and the camera
        // start from the filter the command line asked for.
        app.init_resource::<Shown>()
            .add_systems(PreStartup, refresh_shown)
            .add_systems(
                PostUpdate,
                refresh_shown
                    .run_if(resource_changed::<Filter>.or_eager(resource_changed::<GraphIndex>))
                    .in_set(RefreshShown),
            );
    }
}

fn refresh_shown(filter: Res<Filter>, index: Res<GraphIndex>, mut shown: ResMut<Shown>) {
    let next = self::shown(&filter, &index);
    if shown.set_if_neq(next) {
        log_shown(&filter, &index, &shown);
    }
}

/// Logs what the filter shows: its criteria, and how many nodes pass.
pub fn log_shown(filter: &Filter, index: &GraphIndex, shown: &Shown) {
    let passing = shown.passes.iter().filter(|&&p| p).count();
    info!(
        "filter: {}; {passing} of {} nodes pass and the rest are {}",
        describe(filter, index),
        index.ids.len(),
        match filter.mode {
            Mode::Dim => "dimmed",
            Mode::Hide => "hidden",
        }
    );
}

/// The criteria that are set, in a line for the log.
pub fn describe(filter: &Filter, index: &GraphIndex) -> String {
    let mut parts = Vec::new();
    if filter.kinds != Kinds::default() {
        let kinds = [Kind::Topic, Kind::Fact, Kind::Inference]
            .into_iter()
            .filter(|&kind| kind_passes(&filter.kinds, kind))
            .map(|kind| format!("{kind:?}").to_lowercase())
            .collect::<Vec<_>>();
        parts.push(format!("kinds {}", kinds.join(", ")));
    }
    if let Metacontexts::Only(ids) = &filter.metacontexts {
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        parts.push(format!("metacontexts {}", ids.join(", ")));
    }
    if let Some(topic) = &filter.topic {
        parts.push(format!("topic {topic} and below"));
    }
    if let Some(window) = &filter.window {
        let (from, to) = window_instants(&index.time, window);
        let stamp = |micros: i64| {
            chrono::DateTime::from_timestamp_micros(micros).map_or_else(
                || micros.to_string(),
                |t| t.format("%Y-%m-%d %H:%M").to_string(),
            )
        };
        parts.push(format!(
            "{} from {} to {}",
            match index.mode {
                TimeMode::Recorded => "recorded",
                TimeMode::Claim => "in claim time",
            },
            stamp(from.min(to)),
            stamp(from.max(to))
        ));
    }
    if let Some(source) = &filter.source {
        parts.push(format!("read from document {source}"));
    }
    if parts.is_empty() {
        "no criterion".into()
    } else {
        parts.join("; ")
    }
}

/// What the filter shows of the graph.
pub fn shown(filter: &Filter, index: &GraphIndex) -> Shown {
    let passes = passing(filter, index);
    // A node with no position in the time mode drawn is never drawn, and
    // neither is one that has left the active set.
    let visible = passes
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            graph_index::extent(index, i).is_some()
                && index.active.get(i).copied().unwrap_or(true)
                && (filter.mode == Mode::Dim || p)
        })
        .collect();
    Shown {
        mode: filter.mode,
        passes,
        visible,
    }
}

/// Whether each node passes the filter, in layout order.
pub fn passing(filter: &Filter, index: &GraphIndex) -> Vec<bool> {
    let subtree: Option<Subtree> = filter.topic.as_ref().map(|id| {
        index.by_id.get(id).map_or_else(Subtree::default, |&topic| {
            graph_index::subtree(index, topic)
        })
    });
    (0..index.ids.len())
        .map(|i| passes(filter, index, subtree.as_ref(), i))
        .collect()
}

/// Whether node `i` meets every criterion. `subtree` is the topics the
/// topic criterion covers, worked out once for all nodes.
fn passes(filter: &Filter, index: &GraphIndex, subtree: Option<&Subtree>, i: usize) -> bool {
    kind_passes(&filter.kinds, index.kinds[i])
        && status_passes(filter.status, index.active[i])
        && match &filter.metacontexts {
            Metacontexts::All => true,
            Metacontexts::Only(ids) => index.metacontexts[i].iter().any(|m| ids.contains(m)),
        }
        && subtree.is_none_or(|topics| in_subtree(index, topics, i))
        && filter.window.is_none_or(|window| {
            graph_index::extent(index, i).is_some_and(|(lo, hi)| overlaps(&window, lo, hi))
        })
        && filter
            .source
            .as_ref()
            .is_none_or(|document| index.sources[i].contains(document))
}

pub fn kind_passes(kinds: &Kinds, kind: Kind) -> bool {
    match kind {
        Kind::Topic => kinds.topic,
        Kind::Fact => kinds.fact,
        Kind::Inference => kinds.inference,
    }
}

pub fn status_passes(status: Status, active: bool) -> bool {
    match status {
        Status::ActiveOnly => active,
    }
}

/// Whether node `i` is one of the subtree's topics, a claim filed under any
/// of them, or a claim tagged with one of the topics below the chosen one.
fn in_subtree(index: &GraphIndex, subtree: &Subtree, i: usize) -> bool {
    match index.kinds[i] {
        Kind::Topic => subtree.holds_topic(i),
        Kind::Fact | Kind::Inference => {
            index.filing[i].is_some_and(|t| subtree.holds_topic(t))
                || index.tags[i].iter().any(|t| subtree.below.contains(t))
        }
    }
}

/// Whether a stretch of the axis from `lo` to `hi` overlaps the window,
/// ends included: a disc inside it, or a column reaching into it.
pub fn overlaps(window: &TimeWindow, lo: f32, hi: f32) -> bool {
    let (low, high) = (window.from.min(window.to), window.from.max(window.to));
    lo.min(hi) <= high && lo.max(hi) >= low
}

/// The window from one instant to another, in microseconds since the Unix
/// epoch. Instants outside the axis's span give heights outside it.
pub fn window_between(axis: &TimeAxis, from: i64, to: i64) -> TimeWindow {
    TimeWindow {
        from: time_y(axis, from),
        to: time_y(axis, to),
    }
}

/// The window brought within the axis, each end to the axis's end it lies
/// beyond: the claim axis's window, which a handle cannot drag past it.
pub fn clamp_window(window: &TimeWindow, axis: &TimeAxis) -> TimeWindow {
    let top = axis.height.max(0.0);
    TimeWindow {
        from: window.from.clamp(0.0, top),
        to: window.to.clamp(0.0, top),
    }
}

/// The instants at a window's ends, to the precision of a height.
pub fn window_instants(axis: &TimeAxis, window: &TimeWindow) -> (i64, i64) {
    (instant_at(axis, window.from), instant_at(axis, window.to))
}

/// A node's alpha under the filter, as a fraction of its kind's own: whole
/// when it passes, [`DIM_ALPHA`] when it fails in Dim mode, none when it
/// fails in Hide mode.
pub fn filter_alpha(mode: Mode, passes: bool) -> f32 {
    match (passes, mode) {
        (true, _) => 1.0,
        (false, Mode::Dim) => DIM_ALPHA,
        (false, Mode::Hide) => 0.0,
    }
}

/// Whether node `i` is visible. A list too short to say counts it visible,
/// so an empty list hides nothing.
pub fn is_visible(visible: &[bool], i: usize) -> bool {
    visible.get(i).copied().unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_index::build;
    use crate::graph_index::tests::{edge, node};
    use crate::hub::{EdgeView, NodeType, NodeView};
    use crate::layout::{TIME_HEIGHT, place};
    use crate::presets::DAY_MICROS;

    /// Two topics, `parent` and its subtopic `child`, a third topic `other`,
    /// and claims: `filed` under `child`, `tagged` filed under `other` and
    /// tagged `parent`, `elsewhere` under `other` only, and `inferred`, an
    /// inference under `parent` in a second metacontext with no source. One
    /// node a day from 1 September, so heights rise in this order.
    fn graph() -> (Vec<NodeView>, Vec<EdgeView>) {
        let nodes = vec![
            node("parent", NodeType::Topic, 1),
            node("child", NodeType::Topic, 2),
            node("other", NodeType::Topic, 3),
            node("filed", NodeType::Fact, 4),
            node("tagged", NodeType::Fact, 5),
            node("elsewhere", NodeType::Fact, 6),
            node("inferred", NodeType::Inference, 7),
        ];
        let edges = vec![
            edge("child", "parent", "subtopic_of"),
            edge("filed", "child", "extracted_under_topic"),
            edge("tagged", "other", "extracted_under_topic"),
            edge("tagged", "parent", "tagged_with_topic"),
            edge("elsewhere", "other", "extracted_under_topic"),
            edge("inferred", "parent", "abstracts"),
            edge("parent", "the-real", "has_metacontext"),
            edge("filed", "the-real", "has_metacontext"),
            edge("tagged", "the-real", "has_metacontext"),
            edge("elsewhere", "the-real", "has_metacontext"),
            edge("inferred", "night-city", "has_metacontext"),
            edge("filed", "doc-a", "sourced_from"),
            edge("tagged", "doc-a", "sourced_from"),
            edge("elsewhere", "doc-b", "sourced_from"),
        ];
        (nodes, edges)
    }

    fn index() -> GraphIndex {
        let (nodes, edges) = graph();
        build(&nodes, &edges, &place(&nodes, &edges))
    }

    /// The ids of the nodes that pass, in layout order.
    fn pass(filter: &Filter) -> Vec<String> {
        let index = index();
        passing(filter, &index)
            .into_iter()
            .enumerate()
            .filter(|&(_, p)| p)
            .map(|(i, _)| index.ids[i].clone())
            .collect()
    }

    fn topic(id: &str) -> Filter {
        Filter {
            topic: Some(id.into()),
            ..Filter::default()
        }
    }

    fn day(index: &GraphIndex, d: i64) -> i64 {
        index.time.start + (d - 1) * DAY_MICROS
    }

    #[test]
    fn the_default_passes_every_node() {
        assert_eq!(pass(&Filter::default()).len(), 7);
    }

    #[test]
    fn kinds_pass_each_on_its_own() {
        let only = |topic, fact, inference| Filter {
            kinds: Kinds {
                topic,
                fact,
                inference,
            },
            ..Filter::default()
        };
        assert_eq!(
            pass(&only(true, false, false)),
            ["parent", "child", "other"]
        );
        assert_eq!(
            pass(&only(false, true, false)),
            ["filed", "tagged", "elsewhere"]
        );
        assert_eq!(pass(&only(false, false, true)), ["inferred"]);
        assert!(pass(&only(false, false, false)).is_empty());
    }

    #[test]
    fn a_metacontext_set_passes_its_members_only() {
        let only = |ids: &[&str]| Filter {
            metacontexts: Metacontexts::Only(ids.iter().map(|&s| s.to_owned()).collect()),
            ..Filter::default()
        };
        assert_eq!(
            pass(&only(&["the-real"])),
            ["parent", "filed", "tagged", "elsewhere"],
            "child and other have no metacontext"
        );
        assert_eq!(pass(&only(&["night-city"])), ["inferred"]);
        assert_eq!(pass(&only(&["the-real", "night-city"])).len(), 5);
        assert!(pass(&only(&[])).is_empty());
    }

    #[test]
    fn only_active_nodes_pass_the_status() {
        let (mut nodes, edges) = graph();
        nodes[3].status = "corrected".into();
        let index = build(&nodes, &edges, &place(&nodes, &edges));
        let passes = passing(&Filter::default(), &index);
        assert_eq!(index.ids[3], "filed");
        assert!(!passes[3] && passes.iter().filter(|&&p| p).count() == 6);
    }

    #[test]
    fn a_topic_subtree_holds_its_subtopics_and_claims_filed_or_tagged() {
        assert_eq!(
            pass(&topic("parent")),
            ["parent", "child", "filed", "tagged", "inferred"],
            "child by subtopic_of, filed under child, tagged by a tag, inferred by abstracts"
        );
        assert_eq!(
            pass(&topic("child")),
            ["child", "filed"],
            "the parent is above, not below"
        );
        assert_eq!(pass(&topic("nowhere")), Vec::<String>::new());
        assert!(pass(&topic("filed")).is_empty(), "a claim is no topic");
    }

    #[test]
    fn a_topic_with_no_subtopic_is_its_own_subtree() {
        assert_eq!(pass(&topic("other")), ["other", "tagged", "elsewhere"]);
        let (nodes, mut edges) = graph();
        edges.retain(|e| e.edge_type != "subtopic_of");
        let index = build(&nodes, &edges, &place(&nodes, &edges));
        let passes = passing(&topic("parent"), &index);
        let ids: Vec<&str> = (0..passes.len())
            .filter(|&i| passes[i])
            .map(|i| index.ids[i].as_str())
            .collect();
        assert_eq!(ids, ["parent", "tagged", "inferred"]);
    }

    #[test]
    fn a_topic_tagged_with_the_chosen_one_brings_its_filed_claims_only() {
        let (mut nodes, mut edges) = graph();
        // other is tagged with parent; tagged is filed under other and was
        // inside already by its own tag; elsewhere is filed under other;
        // late, tagged with other only, stays out, and so does far, a topic
        // tagged with other.
        nodes.push(node("late", NodeType::Fact, 8));
        nodes.push(node("far", NodeType::Topic, 9));
        edges.push(edge("other", "parent", "tagged_with_topic"));
        edges.push(edge("late", "child", "extracted_under_topic"));
        edges.push(edge("late", "other", "tagged_with_topic"));
        edges.push(edge("far", "other", "tagged_with_topic"));
        let index = build(&nodes, &edges, &place(&nodes, &edges));
        let passes = passing(&topic("parent"), &index);
        let ids: Vec<&str> = (0..passes.len())
            .filter(|&i| passes[i])
            .map(|i| index.ids[i].as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "parent",
                "child",
                "other",
                "filed",
                "tagged",
                "elsewhere",
                "inferred",
                "late"
            ],
            "late by its filing under child; far is two levels out"
        );
        edges.retain(|e| e.src_id != "late" || e.dst_id != "child");
        let index = build(&nodes, &edges, &place(&nodes, &edges));
        let late = index.by_id["late"];
        assert_eq!(index.filing[late], Some(index.by_id["other"]), "by its tag");
        assert!(
            passing(&topic("parent"), &index)[late],
            "a claim placed beside other by its tag counts as filed there"
        );
        assert!(!passing(&topic("child"), &index)[late]);
    }

    #[test]
    fn a_window_passes_what_lies_inside_it_in_either_direction() {
        let index = index();
        // Days 3 to 5: other, filed, tagged; parent and child before it,
        // elsewhere and inferred after.
        let forward = window_between(&index.time, day(&index, 3), day(&index, 5));
        let backward = TimeWindow {
            from: forward.to,
            to: forward.from,
        };
        for window in [forward, backward] {
            let filter = Filter {
                window: Some(window),
                ..Filter::default()
            };
            assert_eq!(pass(&filter), ["other", "filed", "tagged"], "{window:?}");
        }
        let all = window_between(&index.time, day(&index, 0), day(&index, 30));
        assert!(all.from < 0.0 && all.to > TIME_HEIGHT);
        assert!(overlaps(&all, 0.0, 0.0) && overlaps(&all, TIME_HEIGHT, TIME_HEIGHT));
        let none = window_between(&index.time, day(&index, 20), day(&index, 30));
        assert!(index.heights.iter().all(|&h| !overlaps(&none, h, h)));
    }

    #[test]
    fn in_claim_time_a_window_passes_every_column_that_overlaps_it() {
        let mut index = index();
        // As claim time draws them: parent a disc at 10, child a column from
        // 20 to 60, other a column from 70 to 80, filed a disc at 100, and
        // tagged with no position.
        index.spans = vec![
            Some((10.0, 10.0)),
            Some((20.0, 60.0)),
            Some((70.0, 80.0)),
            Some((100.0, 100.0)),
            None,
            Some((5.0, 6.0)),
            Some((95.0, 140.0)),
        ];
        index.mode = TimeMode::Claim;
        let window = |from, to| Filter {
            window: Some(TimeWindow { from, to }),
            ..Filter::default()
        };
        let ids = |filter: &Filter| -> Vec<&str> {
            passing(filter, &index)
                .into_iter()
                .enumerate()
                .filter(|&(_, p)| p)
                .map(|(i, _)| index.ids[i].as_str())
                .collect()
        };
        assert_eq!(
            ids(&window(50.0, 75.0)),
            ["child", "other"],
            "each reaches into it"
        );
        assert_eq!(ids(&window(75.0, 50.0)), ["child", "other"]);
        assert_eq!(ids(&window(60.0, 60.0)), ["child"], "the ends are inside");
        assert_eq!(ids(&window(98.0, 120.0)), ["filed", "inferred"]);
        assert!(ids(&window(150.0, 200.0)).is_empty());
        let shown = shown(&Filter::default(), &index);
        assert!(shown.passes[4], "the filter has nothing against it");
        assert!(
            !shown.visible[4],
            "but it has no position, so it is not drawn"
        );
        assert_eq!(shown.visible.iter().filter(|&&v| v).count(), 6);
        assert!(describe(&window(50.0, 75.0), &index).starts_with("in claim time from"));
    }

    #[test]
    fn a_window_brought_within_the_axis_stops_at_its_ends() {
        let axis = TimeAxis {
            start: 0,
            span: 10,
            height: 400.0,
        };
        let within = TimeWindow {
            from: 50.0,
            to: 120.0,
        };
        assert_eq!(clamp_window(&within, &axis), within);
        let beyond = TimeWindow {
            from: 900.0,
            to: -3_000.0,
        };
        assert_eq!(
            clamp_window(&beyond, &axis),
            TimeWindow {
                from: 400.0,
                to: 0.0
            }
        );
        // A node clipped to the foot stands at 0, so a window that reaches
        // the foot passes it.
        assert!(overlaps(&clamp_window(&beyond, &axis), 0.0, 0.0));
    }

    #[test]
    fn a_window_reads_back_as_its_instants() {
        let index = index();
        let (from, to) = (day(&index, 2) + 7, day(&index, 6) - 11);
        let (back_from, back_to) =
            window_instants(&index.time, &window_between(&index.time, from, to));
        let tolerance = index.time.span / 1_000_000;
        assert!((back_from - from).abs() <= tolerance);
        assert!((back_to - to).abs() <= tolerance);
    }

    #[test]
    fn a_source_passes_what_was_read_from_it() {
        let from = |document: &str| Filter {
            source: Some(document.into()),
            ..Filter::default()
        };
        assert_eq!(pass(&from("doc-a")), ["filed", "tagged"]);
        assert_eq!(pass(&from("doc-b")), ["elsewhere"]);
        assert!(pass(&from("doc-c")).is_empty());
    }

    #[test]
    fn criteria_combine_so_a_node_must_meet_them_all() {
        let index = index();
        let filter = Filter {
            kinds: Kinds {
                topic: false,
                ..Kinds::default()
            },
            topic: Some("parent".into()),
            source: Some("doc-a".into()),
            ..Filter::default()
        };
        assert_eq!(pass(&filter), ["filed", "tagged"]);
        let filter = Filter {
            window: Some(window_between(&index.time, day(&index, 5), day(&index, 9))),
            metacontexts: Metacontexts::Only(["the-real".to_owned()].into()),
            ..filter
        };
        assert_eq!(pass(&filter), ["tagged"]);
        let filter = Filter {
            kinds: Kinds {
                fact: false,
                ..filter.kinds
            },
            ..filter
        };
        assert!(pass(&filter).is_empty());
    }

    #[test]
    fn the_log_line_names_each_criterion_set() {
        let index = index();
        assert_eq!(describe(&Filter::default(), &index), "no criterion");
        let filter = Filter {
            kinds: Kinds {
                topic: false,
                ..Kinds::default()
            },
            topic: Some("parent".into()),
            window: Some(window_between(&index.time, day(&index, 3), day(&index, 5))),
            source: Some("doc-a".into()),
            ..Filter::default()
        };
        assert_eq!(
            describe(&filter, &index),
            "kinds fact, inference; topic parent and below; \
             recorded from 2026-09-03 12:00 to 2026-09-05 12:00; read from document doc-a"
        );
    }

    #[test]
    fn dim_keeps_every_node_visible_and_hide_only_the_passing() {
        let index = index();
        let dim = shown(&topic("child"), &index);
        assert_eq!(dim.passes.iter().filter(|&&p| p).count(), 2);
        assert!(dim.visible.iter().all(|&v| v));
        let hide = shown(
            &Filter {
                mode: Mode::Hide,
                ..topic("child")
            },
            &index,
        );
        assert_eq!(hide.visible, hide.passes);
        assert_eq!(filter_alpha(Mode::Dim, true), 1.0);
        assert_eq!(filter_alpha(Mode::Hide, true), 1.0);
        assert_eq!(filter_alpha(Mode::Dim, false), DIM_ALPHA);
        assert_eq!(filter_alpha(Mode::Hide, false), 0.0);
        assert!(is_visible(&[], 3) && !is_visible(&[true, false], 1));
    }

    /// How many updates `Shown` changed in.
    #[derive(Resource, Default)]
    struct Changes(usize);

    fn count_changes(shown: Res<Shown>, mut changes: ResMut<Changes>) {
        if shown.is_changed() {
            changes.0 += 1;
        }
    }

    #[test]
    fn shown_is_recomputed_only_when_the_filter_changes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(index())
            .insert_resource(Filter::default())
            .init_resource::<Changes>()
            .add_plugins(FilterPlugin)
            .add_systems(PostUpdate, count_changes.after(RefreshShown));
        app.update();
        assert!(app.world().resource::<Shown>().passes.iter().all(|&p| p));
        let changes = |app: &App| app.world().resource::<Changes>().0;
        assert_eq!(changes(&app), 1, "computed before the first update");

        app.update();
        app.update();
        assert_eq!(changes(&app), 1, "no change of filter, no new Shown");

        let mut filter = app.world_mut().resource_mut::<Filter>();
        filter.mode = Mode::Hide;
        filter.topic = Some("other".into());
        app.update();
        assert_eq!(changes(&app), 2);
        let shown = app.world().resource::<Shown>();
        assert_eq!(shown.mode, Mode::Hide);
        assert_eq!(shown.visible.iter().filter(|&&v| v).count(), 3);

        // A write that leaves the filter as it was changes nothing shown.
        app.world_mut().resource_mut::<Filter>().set_changed();
        app.update();
        app.update();
        assert_eq!(changes(&app), 2);
    }
}
