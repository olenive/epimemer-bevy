//! The side panel (DESIGN.md §10): the focused node in full, on the right
//! of the window, in Bevy UI.
//!
//! With a retrieval record chosen in the bar, the panel's first line says
//! how many of the ids it retrieved name no active node of the graph drawn,
//! or that it declares none (`retrieval.rs`), whatever is focused. With a
//! node focused the panel shows, top to bottom:
//!
//! - every field of the contract's NodeView, named in plain words, the
//!   content and the description whole and wrapped where the label cuts
//!   them short (DESIGN.md §9), and the metadata as key and value rows;
//! - its edges by class, in the order DESIGN.md §10 lists them: its
//!   membership, the topic it is filed under and its tags, by name, its
//!   metacontexts by name and its source documents by id, and for a topic
//!   its subtopics, the topics tagged with it, how many claims stand
//!   beside it and where it stands on the topic map: placed by its
//!   embedding, or on its ring and why (`relayout.rs`); then its
//!   claim-to-claim edges and its succession and merge
//!   edges (DESIGN.md §5), each with its type, its direction and the node
//!   at its far end by its label, or by its id where the snapshot does not
//!   hold it, with a count of each and of the far ends the filter hides;
//! - its time in both modes: where recorded time places it, and in claim
//!   time the timepoints it is linked to, the one its column is drawn by,
//!   the others, and those with no place and why, which was the tray's
//!   line for the focused node, and for a node beyond the claim axis its
//!   true dates and how it is drawn clipped; then the validity intervals its sources
//!   assert, each end by its kind of instant, with the ends that have no
//!   coordinate named as such, and whether the source stated the dates or
//!   they were read from tense and context.
//!
//! A click on a node named in the panel focuses it, so a chain of supports
//! or successions can be walked, and Back, or `B`, returns along the focus
//! history (`focus.rs`). Without a focus the panel says how to get one.
//! While claim time is drawn the tray follows (`tray.rs`): the nodes beyond
//! the claim axis, drawn clipped at an end of it, and the claims claim time
//! cannot place or places by one of several timepoints, each a row whose
//! click focuses it.
//!
//! What the panel shows is worked out as plain lines by pure functions
//! ([`node_lines`], [`tray_lines`]), kept in resources and drawn again only
//! when they change: on a change of focus, of the filter, of the time mode,
//! or of the graph, which the event stream changes in place (`apply.rs`).
//!
//! The panel is collapsible, by its Hide button or `P`, to a button that
//! brings it back. It scrolls with the wheel, takes the pointer as the bar
//! does, so that nothing beneath it is hovered or clicked (`ui_focus.rs`),
//! and reports how much of the window it covers as [`PanelCover`], so that
//! every view is fitted and every label placed to its left.

use std::collections::BTreeMap;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{ScrollPosition, UiSystems};
use bevy::ui_widgets::ScrollArea;

use crate::apply::LiveGraph;
use crate::bar::style;
use crate::camera::{BarCover, PanelCover};
use crate::clip::{beyond_line, clip_note, is_clipped};
use crate::edge_index::{EdgeIndex, End};
use crate::filter::{Filter, RefreshShown, Shown, is_visible};
use crate::focus::{Focus, FocusHistory, GoBack, back_target};
use crate::graph_index::GraphIndex;
use crate::hub::{NodeType, NodeView, parse_timestamp};
use crate::keys::{BACK_KEY, PANEL_KEY};
use crate::labels::label_text;
use crate::layout::{Filing, Kind};
use crate::pickers::{Pickers, short_id};
use crate::relayout::MapState;
use crate::retrieval;
use crate::time_axis::{
    Linked, Placement, TimeMode, TimeModel, Validity, bound_text, date, line_text, reason_text,
};
use crate::tray::{self, Tray, TrayOpen};
use crate::ui_focus::UiFocus;

/// How wide the panel is, and the room around it, in logical pixels.
pub const PANEL_WIDTH: f32 = 380.0;
pub const PANEL_MARGIN: f32 = 8.0;
/// How wide the column of field names is.
const NAME_WIDTH: f32 = 118.0;
/// The most rows an edge section lists, and the most claims the tray does.
pub const MAX_EDGE_ROWS: usize = 200;
pub const MAX_TRAY_ENTRIES: usize = 300;

/// What the panel says with nothing focused.
pub const HINT: &str = "Nothing is focused. Click a disc to see its node here in full: every field, its time in both modes, and its edges. A click on a node named here focuses it, and Back returns. / types into the search field in the bar.";

/// One field of the focused node as the panel shows it: the NodeView
/// field, its name in plain words, and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeField {
    pub field: &'static str,
    pub name: &'static str,
    pub value: String,
}

/// One line of the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// A section's heading, with its count where it has one.
    Heading(String),
    /// A name and a value, which wraps.
    Field { name: String, value: String },
    /// A row naming another node after a caption: a click focuses the node
    /// where there is one. Faint for a node the filter hides or one the
    /// snapshot does not hold.
    Link {
        caption: String,
        text: String,
        node: Option<usize>,
        faint: bool,
    },
    /// A line of prose.
    Note(String),
    /// A line under the one before, indented.
    Detail(String),
    /// The tray's heading, whose click opens and closes its list.
    TrayHeading(String),
}

/// Whether the panel is collapsed to its button.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelCollapsed(pub bool);

/// The panel's heading: what is focused, and whether Back has somewhere to
/// go.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelHeader {
    pub title: String,
    pub back: bool,
}

/// The focused node's lines, or the hint.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct NodeLines(pub Vec<Line>);

/// The tray's lines, while claim time is drawn.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct TrayLines(pub Vec<Line>);

/// The panel's outer node, the full height of the window below the bar.
#[derive(Component)]
struct PanelRoot;

/// The panel itself.
#[derive(Component)]
struct PanelBox;

/// The panel's title.
#[derive(Component)]
struct PanelTitle;

/// The words of the Back button, faint while there is nowhere to go back
/// to.
#[derive(Component)]
struct BackWords;

/// The part of the panel that scrolls.
#[derive(Component)]
struct PanelBody;

/// The block of the focused node's lines, and the tray's.
#[derive(Component)]
struct NodeBlock;

#[derive(Component)]
struct TrayBlock;

/// The button the panel collapses to.
#[derive(Component)]
struct PanelStrip;

/// What a click on a part of the panel does.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum PanelControl {
    Back,
    Collapse,
    Expand,
    ToggleTray,
    Focus(usize),
}

pub struct PanelPlugin;

impl Plugin for PanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PanelCollapsed>()
            .init_resource::<PanelHeader>()
            .init_resource::<NodeLines>()
            .init_resource::<TrayLines>()
            .init_resource::<PanelCover>()
            .init_resource::<BarCover>()
            .init_resource::<Focus>()
            .init_resource::<FocusHistory>()
            .init_resource::<UiFocus>()
            .init_resource::<TimeMode>()
            .init_resource::<TimeModel>()
            .init_resource::<Tray>()
            .init_resource::<TrayOpen>()
            .init_resource::<Shown>()
            .add_message::<GoBack>()
            .add_observer(on_click)
            .add_systems(Startup, spawn_panel)
            .add_systems(Update, (toggle_on_key, style_controls))
            .add_systems(
                PostUpdate,
                (
                    refresh_lines,
                    show_header,
                    show_node_lines,
                    show_tray_lines,
                    show_collapsed,
                    place_below_bar,
                )
                    .chain()
                    .after(RefreshShown)
                    .before(UiSystems::Prepare),
            )
            .add_systems(PostUpdate, measure_cover.after(UiSystems::Layout));
    }
}

/// A node kind's name.
pub fn kind_name(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::Topic => "topic",
        NodeType::Fact => "fact",
        NodeType::Inference => "inference",
        NodeType::Unknown => "unknown",
    }
}

/// A timestamp to the second, in UTC, or as the hub wrote it where it does
/// not read.
pub fn stamp(text: &str) -> String {
    parse_timestamp(text)
        .and_then(chrono::DateTime::from_timestamp_micros)
        .map_or_else(
            || text.to_owned(),
            |t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        )
}

/// A value, or "empty" for a blank one.
fn or_empty(value: &str) -> String {
    if value.trim().is_empty() {
        "empty".to_owned()
    } else {
        value.to_owned()
    }
}

/// A metadata value as text: a string as it is, anything else as JSON.
pub fn metadata_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Every field of a node, in the order the panel shows them. The view is
/// taken apart field by field, so a field added to NodeView does not build
/// until it is shown here.
pub fn node_fields(view: &NodeView) -> Vec<NodeField> {
    let NodeView {
        node_id,
        node_type,
        content,
        description,
        status,
        source_id,
        extraction_method,
        confidence,
        retrieved_at,
        created_at,
        graph,
        metadata,
    } = view;
    let field = |field, name, value: String| NodeField { field, name, value };
    vec![
        field("node_type", "Kind", kind_name(*node_type).to_owned()),
        field("content", "Content", or_empty(content)),
        field("description", "Description", or_empty(description)),
        field("status", "Status", status.clone()),
        field(
            "confidence",
            "Confidence",
            confidence.map_or_else(|| "unrated".to_owned(), |c| c.to_string()),
        ),
        field(
            "extraction_method",
            "Extraction method",
            extraction_method.clone(),
        ),
        field(
            "source_id",
            "Source segment",
            source_id.clone().unwrap_or_else(|| "none".to_owned()),
        ),
        field("created_at", "Created at", stamp(created_at)),
        field(
            "retrieved_at",
            "Retrieved at",
            retrieved_at
                .as_deref()
                .map_or_else(|| "never returned by a search".to_owned(), stamp),
        ),
        field("graph", "Graph", graph.clone()),
        field("node_id", "Node id", node_id.clone()),
        field(
            "metadata",
            "Metadata",
            match metadata.len() {
                0 => "empty".to_owned(),
                1 => "1 key, below".to_owned(),
                n => format!("{n} keys, below"),
            },
        ),
    ]
}

/// The lines of a node's fields, and its metadata's key and value rows.
pub fn field_lines(view: &NodeView) -> Vec<Line> {
    let mut lines: Vec<Line> = node_fields(view)
        .into_iter()
        .map(|f| Line::Field {
            name: f.name.to_owned(),
            value: f.value,
        })
        .collect();
    lines.extend(view.metadata.iter().map(|(key, value)| Line::Field {
        name: key.clone(),
        value: metadata_value(value),
    }));
    lines
}

/// A timepoint's name in the panel: its label and the start of its id.
pub fn timepoint_title(linked: &Linked) -> String {
    let id = if linked.timepoint_id.is_empty() {
        "with no id".to_owned()
    } else {
        short_id(&linked.timepoint_id).to_owned()
    };
    match linked.label.as_deref().map(str::trim) {
        Some(label) if !label.is_empty() => format!("{label:?}, timepoint {id}"),
        _ => format!("timepoint {id}"),
    }
}

/// A placement by its kind of timepoint and its bounds.
pub fn kind_and_bounds(placement: &Placement) -> String {
    match *placement {
        Placement::Recorded(t) => format!("recorded {}", date(t)),
        Placement::Instant(t) => format!("instant at {}", date(t)),
        Placement::Interval { start, end } => {
            format!("interval from {} to {}", date(start), date(end))
        }
        Placement::Smear { earliest, latest } => match (earliest, latest) {
            (Some(e), Some(l)) => format!("vague, between {} and {}", date(e), date(l)),
            (Some(e), None) => format!("vague, after {}", date(e)),
            (None, Some(l)) => format!("vague, before {}", date(l)),
            (None, None) => "vague, with no bounds".to_owned(),
        },
    }
}

/// A validity interval's basis in plain words.
pub fn basis_text(validity: &Validity) -> &'static str {
    if validity.stated {
        "stated in the source"
    } else {
        "inferred from tense and context"
    }
}

/// The time section: where each mode places the node, its timepoints and
/// its validity intervals.
pub fn time_lines(model: &TimeModel, kind: Kind, node: usize, created_at: &str) -> Vec<Line> {
    let mut lines = vec![
        Line::Heading("Time".to_owned()),
        Line::Field {
            name: "Recorded time".to_owned(),
            value: format!("a disc at {}", stamp(created_at)),
        },
    ];
    let linked = model.linked.get(node).map_or(&[][..], Vec::as_slice);
    let drawn = model.claim.spans.get(node).is_some_and(Option::is_some);
    let claim = match (kind, linked.first()) {
        (Kind::Topic, _) => "a disc at its recorded time, as every topic is".to_owned(),
        (_, None) => "no timepoint, so a disc at its recorded time".to_owned(),
        (_, Some(first)) if drawn && first.place.is_ok() => {
            format!("drawn by {}", timepoint_title(first))
        }
        _ => "not drawn: none of its timepoints has a place".to_owned(),
    };
    lines.push(Line::Field {
        name: "Claim time".to_owned(),
        value: claim,
    });
    let clipped = model.claim.spans.get(node).copied().flatten();
    let place = model.claim_places.get(node).copied().flatten();
    if let (Some(span), Some(place)) = (clipped, place)
        && is_clipped(&span.clipped)
    {
        lines.push(Line::Field {
            name: "Beyond the axis".to_owned(),
            value: clip_note(&place, &span.clipped, &model.claim.axis),
        });
    }
    if kind != Kind::Topic {
        for (at, l) in linked.iter().enumerate() {
            let what = match &l.place {
                Ok(placement) if at == 0 && drawn => {
                    format!("{}, its column", kind_and_bounds(placement))
                }
                Ok(placement) => format!("{}, also linked", kind_and_bounds(placement)),
                Err(reason) => format!("no place: {}", reason_text(reason)),
            };
            lines.push(Line::Detail(format!("{}: {what}", timepoint_title(l))));
        }
    }
    let validity = model.validity.get(node).map_or(&[][..], Vec::as_slice);
    if validity.is_empty() {
        lines.push(Line::Field {
            name: "Validity".to_owned(),
            value: "none from its sources".to_owned(),
        });
    }
    for v in validity {
        lines.push(Line::Field {
            name: "Valid from".to_owned(),
            value: bound_text(&v.start),
        });
        lines.push(Line::Field {
            name: "Valid until".to_owned(),
            value: bound_text(&v.end),
        });
        lines.push(Line::Detail(format!(
            "{}, from document {}",
            basis_text(v),
            short_id(&v.document)
        )));
    }
    lines
}

/// A row naming a topic.
fn topic_link(index: &GraphIndex, topic: usize, caption: &str) -> Line {
    Line::Link {
        caption: caption.to_owned(),
        text: node_label(index, topic),
        node: Some(topic),
        faint: false,
    }
}

/// A drawn node's label, short as on the scene.
pub fn node_label(index: &GraphIndex, node: usize) -> String {
    match (index.names.get(node), index.ids.get(node)) {
        (Some(name), Some(id)) => label_text(name, id),
        _ => format!("node {node}"),
    }
}

/// The membership section: where the node belongs, by name, with a count
/// of the edges that say so. `rule` is how its place was chosen, where it
/// is known; `metacontexts` names the metacontexts by id.
pub fn membership_lines(
    index: &GraphIndex,
    node: usize,
    rule: Option<Filing>,
    metacontexts: &BTreeMap<&str, &str>,
) -> Vec<Line> {
    let list = |lists: &[Vec<usize>]| lists.get(node).cloned().unwrap_or_default();
    let mut rows = Vec::new();
    let mut count = 0;
    let topic = index.kinds.get(node) == Some(&Kind::Topic);
    if topic {
        let parents: Vec<usize> = (0..index.ids.len())
            .filter(|&t| index.subtopics.get(t).is_some_and(|s| s.contains(&node)))
            .collect();
        for (caption, nodes) in [
            ("Subtopic of", parents),
            ("Subtopic", list(&index.subtopics)),
            ("Tagged with", list(&index.tags)),
            ("Tags it", list(&index.tagged_topics)),
        ] {
            count += nodes.len();
            rows.extend(nodes.into_iter().map(|t| topic_link(index, t, caption)));
        }
        let beside = (0..index.ids.len())
            .filter(|&c| {
                index.filing.get(c) == Some(&Some(node))
                    && index.kinds.get(c) != Some(&Kind::Topic)
                    && index.active.get(c).copied().unwrap_or(true)
            })
            .count();
        rows.push(Line::Field {
            name: "Claims beside it".to_owned(),
            value: beside.to_string(),
        });
    } else {
        match (index.filing.get(node).copied().flatten(), rule) {
            (Some(t), Some(Filing::Tagged)) => {
                rows.push(topic_link(index, t, "Placed by its tag"));
            }
            (Some(t), _) => {
                count += 1;
                rows.push(topic_link(index, t, "Filed under"));
            }
            (None, _) => rows.push(Line::Field {
                name: "Filed under".to_owned(),
                value: "no topic, so it stands in the unfiled region".to_owned(),
            }),
        }
        let tags = list(&index.tags);
        count += tags.len();
        rows.extend(tags.into_iter().map(|t| topic_link(index, t, "Tag")));
    }
    for id in index.metacontexts.get(node).into_iter().flatten() {
        count += 1;
        rows.push(Line::Field {
            name: "Metacontext".to_owned(),
            value: metacontexts
                .get(id.as_str())
                .map_or_else(|| id.clone(), |name| format!("{name} ({id})")),
        });
    }
    for id in index.sources.get(node).into_iter().flatten() {
        count += 1;
        rows.push(Line::Field {
            name: "Source document".to_owned(),
            value: id.clone(),
        });
    }
    let mut lines = vec![Line::Heading(format!("Membership ({count})"))];
    lines.extend(rows);
    lines
}

/// The far end of an edge, from a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Far {
    /// A drawn node, and whether the filter hides it.
    Drawn { node: usize, hidden: bool },
    /// The node itself: the edge is a loop.
    Itself,
    /// An id the snapshot does not hold, named from the edge.
    Absent(String),
}

/// An edge as the panel lists it, from one of its nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeRow {
    pub edge_type: String,
    /// Whether the edge leaves the node: the node is its source.
    pub outgoing: bool,
    pub far: Far,
}

/// A node's edges of one class, by type, and how many of their far ends
/// the snapshot does not hold and the filter hides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeGroup {
    pub rows: Vec<EdgeRow>,
    pub absent: usize,
    pub hidden: usize,
}

/// The edges listed at `slots`, from `node`, by type and then in the
/// snapshot's order, with the far ends counted.
pub fn edge_group(edges: &EdgeIndex, slots: &[usize], node: usize, visible: &[bool]) -> EdgeGroup {
    let mut group = EdgeGroup::default();
    for edge in slots.iter().filter_map(|&slot| edges.edges.get(slot)) {
        let outgoing = edge.src == End::Drawn(node);
        let far = match edge.far_end(node) {
            End::Drawn(far) if *far == node => Far::Itself,
            End::Drawn(far) => Far::Drawn {
                node: *far,
                hidden: !is_visible(visible, *far),
            },
            End::Absent(id) => Far::Absent(id.clone()),
        };
        match far {
            Far::Absent(_) => group.absent += 1,
            Far::Drawn { hidden: true, .. } => group.hidden += 1,
            _ => {}
        }
        group.rows.push(EdgeRow {
            edge_type: edge.edge_type.clone(),
            outgoing,
            far,
        });
    }
    group.rows.sort_by(|a, b| a.edge_type.cmp(&b.edge_type));
    group
}

/// An edge section's heading: its name, the count, and the far ends not in
/// the snapshot or hidden by the filter.
pub fn edge_heading(name: &str, group: &EdgeGroup) -> String {
    let mut parts = vec![group.rows.len().to_string()];
    if group.absent > 0 {
        parts.push(format!("{} not in the snapshot", group.absent));
    }
    if group.hidden > 0 {
        parts.push(format!("{} hidden by the filter", group.hidden));
    }
    format!("{name} ({})", parts.join(", "))
}

/// An edge's row: its type and direction as the caption, then the far end.
pub fn edge_line(row: &EdgeRow, index: &GraphIndex) -> Line {
    let caption = match (&row.far, row.outgoing) {
        (Far::Itself, _) => format!("{}, to itself", row.edge_type),
        (_, true) => format!("{} ->", row.edge_type),
        (_, false) => format!("<- {}", row.edge_type),
    };
    let (text, node, faint) = match &row.far {
        Far::Drawn { node, hidden } => {
            let label = node_label(index, *node);
            let text = if *hidden {
                format!("{label} (hidden by the filter)")
            } else {
                label
            };
            (text, Some(*node), *hidden)
        }
        Far::Itself => ("this node".to_owned(), None, false),
        Far::Absent(id) => (format!("not in the snapshot: {id}"), None, true),
    };
    Line::Link {
        caption,
        text,
        node,
        faint,
    }
}

/// An edge section: its heading and its rows, at most [`MAX_EDGE_ROWS`].
pub fn edge_lines(name: &str, group: &EdgeGroup, index: &GraphIndex) -> Vec<Line> {
    let mut lines = vec![Line::Heading(edge_heading(name, group))];
    if group.rows.is_empty() {
        lines.push(Line::Note("none".to_owned()));
    }
    lines.extend(
        group
            .rows
            .iter()
            .take(MAX_EDGE_ROWS)
            .map(|row| edge_line(row, index)),
    );
    if group.rows.len() > MAX_EDGE_ROWS {
        lines.push(Line::Note(format!(
            "and {} more",
            group.rows.len() - MAX_EDGE_ROWS
        )));
    }
    lines
}

/// What the panel reads to say what is focused.
pub struct Context<'a> {
    pub index: &'a GraphIndex,
    pub model: &'a TimeModel,
    pub visible: &'a [bool],
    /// How each claim was placed, where it is known.
    pub filing: Option<&'a [Filing]>,
    pub metacontexts: BTreeMap<&'a str, &'a str>,
    /// The topic map, which says where a topic stands on it.
    pub map: Option<&'a MapState>,
}

/// The panel's title and lines for a focus: the hint without one, and
/// otherwise every section of the node.
pub fn node_lines(context: &Context, focus: Option<usize>) -> (String, Vec<Line>) {
    let index = context.index;
    let Some(node) = focus.filter(|&i| i < index.ids.len()) else {
        return ("Side panel".to_owned(), vec![Line::Note(HINT.to_owned())]);
    };
    let kind = index.kinds.get(node).copied().unwrap_or(Kind::Fact);
    let title = format!(
        "Focused {}",
        match kind {
            Kind::Topic => "topic",
            Kind::Fact => "fact",
            Kind::Inference => "inference",
        }
    );
    let mut lines = Vec::new();
    let created = match index.views.get(node) {
        Some(view) => {
            lines.extend(field_lines(view));
            view.created_at.clone()
        }
        None => {
            lines.push(Line::Field {
                name: "Node id".to_owned(),
                value: index.ids[node].clone(),
            });
            String::new()
        }
    };
    let rule = context.filing.and_then(|f| f.get(node).copied());
    lines.extend(membership_lines(index, node, rule, &context.metacontexts));
    if kind == Kind::Topic
        && let Some(map) = context.map
    {
        lines.push(Line::Field {
            name: "On the map".to_owned(),
            value: map.topic_note(&index.ids[node]),
        });
    }
    let edges = &index.edges;
    let list = |lists: &[Vec<usize>]| lists.get(node).cloned().unwrap_or_default();
    let claims = edge_group(edges, &list(&edges.claim), node, context.visible);
    lines.extend(edge_lines("Claim to claim", &claims, index));
    let successions = edge_group(edges, &list(&edges.succession), node, context.visible);
    lines.extend(edge_lines("Succession and merge", &successions, index));
    lines.extend(time_lines(context.model, kind, node, &created));
    (title, lines)
}

/// The tray's lines, in claim time: its heading, and while it is open each
/// claim's row and what claim time leaves out of it.
pub fn tray_lines(tray: &Tray, open: bool, index: &GraphIndex, mode: TimeMode) -> Vec<Line> {
    if mode != TimeMode::Claim {
        return Vec::new();
    }
    let mut lines = vec![Line::TrayHeading(tray::heading(tray, open))];
    if !open {
        return lines;
    }
    if !tray.beyond.is_empty() {
        lines.push(Line::Heading(format!(
            "Beyond the axis ({})",
            tray.beyond.len()
        )));
        lines.push(Line::Note(
            "Drawn clipped, at the end of the claim axis they lie beyond.".to_owned(),
        ));
        for entry in tray.beyond.iter().take(MAX_TRAY_ENTRIES) {
            lines.push(Line::Link {
                caption: String::new(),
                text: tray::beyond_text(entry, index),
                node: Some(entry.node),
                faint: false,
            });
            lines.push(Line::Detail(beyond_line(entry)));
        }
        if tray.beyond.len() > MAX_TRAY_ENTRIES {
            lines.push(Line::Note(format!(
                "and {} more",
                tray.beyond.len() - MAX_TRAY_ENTRIES
            )));
        }
        lines.push(Line::Heading(format!(
            "Not drawn, or with more timepoints ({})",
            tray.standing.len()
        )));
    }
    if tray.standing.is_empty() {
        lines.push(Line::Note(
            "Every claim linked to a timepoint is drawn by it.".to_owned(),
        ));
    }
    for entry in tray.standing.iter().take(MAX_TRAY_ENTRIES) {
        lines.push(Line::Link {
            caption: String::new(),
            text: tray::entry_text(entry, index),
            node: Some(entry.node),
            faint: false,
        });
        lines.extend(entry.lines.iter().map(|l| Line::Detail(line_text(l))));
    }
    if tray.standing.len() > MAX_TRAY_ENTRIES {
        lines.push(Line::Note(format!(
            "and {} more",
            tray.standing.len() - MAX_TRAY_ENTRIES
        )));
    }
    lines
}

/// What the panel's lines are worked out from.
#[derive(SystemParam)]
struct Sources<'w> {
    focus: Res<'w, Focus>,
    index: Option<Res<'w, GraphIndex>>,
    model: Res<'w, TimeModel>,
    mode: Res<'w, TimeMode>,
    shown: Res<'w, Shown>,
    tray: Res<'w, Tray>,
    open: Res<'w, TrayOpen>,
    pickers: Option<Res<'w, Pickers>>,
    live: Option<Res<'w, LiveGraph>>,
    history: Res<'w, FocusHistory>,
    map: Option<Res<'w, MapState>>,
    filter: Option<Res<'w, Filter>>,
}

impl Sources<'_> {
    fn changed(&self) -> bool {
        self.focus.is_changed()
            || self.filter.as_ref().is_some_and(|f| f.is_changed())
            || self.index.as_ref().is_some_and(|i| i.is_changed())
            || self.model.is_changed()
            || self.mode.is_changed()
            || self.shown.is_changed()
            || self.tray.is_changed()
            || self.open.is_changed()
            || self.pickers.as_ref().is_some_and(|p| p.is_changed())
            || self.live.as_ref().is_some_and(|l| l.is_changed())
            || self.history.is_changed()
            || self.map.as_ref().is_some_and(|m| m.is_changed())
    }
}

/// Works the panel's lines out again when anything they show changed,
/// keeping each resource as it is where its lines are the same.
fn refresh_lines(
    sources: Sources,
    mut header: ResMut<PanelHeader>,
    mut node: ResMut<NodeLines>,
    mut tray: ResMut<TrayLines>,
) {
    if !sources.changed() {
        return;
    }
    let empty = GraphIndex::default();
    let index = sources.index.as_deref().unwrap_or(&empty);
    let context = Context {
        index,
        model: &sources.model,
        visible: &sources.shown.visible,
        filing: sources.live.as_deref().map(|l| l.filing.as_slice()),
        metacontexts: sources
            .pickers
            .as_deref()
            .map(|p| {
                p.metacontexts
                    .iter()
                    .map(|m| (m.id.as_str(), m.name.as_str()))
                    .collect()
            })
            .unwrap_or_default(),
        map: sources.map.as_deref(),
    };
    let (title, mut lines) = node_lines(&context, sources.focus.0);
    if let Some(focus) = sources.filter.as_deref().and_then(|f| f.retrieval.as_ref()) {
        lines.insert(
            0,
            Line::Field {
                name: "Retrieval".to_owned(),
                value: retrieval::panel_line(focus, index),
            },
        );
    }
    header.set_if_neq(PanelHeader {
        title,
        back: back_target(&sources.history, index).is_some(),
    });
    node.set_if_neq(NodeLines(lines));
    tray.set_if_neq(TrayLines(tray_lines(
        &sources.tray,
        sources.open.0,
        index,
        *sources.mode,
    )));
}

/// A text in the panel's font, wrapped at the panel's edge, breaking a word
/// too long for a line, such as an id.
fn wrapped(words: impl Into<String>, colour: Color) -> impl Bundle {
    (
        Text::new(words),
        TextFont {
            font_size: FontSize::Px(style::FONT_SIZE),
            ..default()
        },
        TextColor(colour),
        TextLayout::linebreak(LineBreak::WordOrCharacter),
    )
}

/// A span of a text, in the panel's font.
fn span(words: impl Into<String>, colour: Color) -> impl Bundle {
    (
        TextSpan::new(words),
        TextFont {
            font_size: FontSize::Px(style::FONT_SIZE),
            ..default()
        },
        TextColor(colour),
    )
}

/// The node of a row that fills the panel's width.
fn row_node() -> Node {
    Node {
        width: percent(100),
        column_gap: px(8),
        padding: UiRect::axes(px(4), px(0)),
        border_radius: BorderRadius::all(px(2)),
        flex_shrink: 0.0,
        ..default()
    }
}

/// Spawns one line under `parent`.
fn spawn_line(commands: &mut Commands, parent: Entity, line: &Line) {
    match line {
        Line::Heading(words) => {
            commands.spawn((
                Node {
                    margin: UiRect::top(px(6)),
                    padding: UiRect::axes(px(4), px(0)),
                    flex_shrink: 0.0,
                    ..default()
                },
                ChildOf(parent),
                children![wrapped(words.clone(), style::ACCENT)],
            ));
        }
        Line::Field { name, value } => {
            commands.spawn((
                row_node(),
                ChildOf(parent),
                children![
                    (
                        Node {
                            width: px(NAME_WIDTH),
                            flex_shrink: 0.0,
                            ..default()
                        },
                        children![wrapped(name.clone(), style::CAPTION)],
                    ),
                    (
                        Node {
                            flex_grow: 1.0,
                            flex_shrink: 1.0,
                            flex_basis: px(0),
                            min_width: px(0),
                            ..default()
                        },
                        children![wrapped(value.clone(), style::TEXT)],
                    ),
                ],
            ));
        }
        Line::Link {
            caption,
            text,
            node,
            faint,
        } => {
            let colour = if *faint { style::FAINT } else { style::TEXT };
            let mut row =
                commands.spawn((row_node(), BackgroundColor(style::ROW), ChildOf(parent)));
            if let Some(node) = node {
                row.insert((Button, PanelControl::Focus(*node)));
            }
            let row = row.id();
            let words = commands
                .spawn((wrapped(String::new(), colour), ChildOf(row)))
                .id();
            if !caption.is_empty() {
                commands.spawn((span(format!("{caption}  "), style::CAPTION), ChildOf(words)));
            }
            commands.spawn((span(text.clone(), colour), ChildOf(words)));
        }
        Line::Note(words) => {
            commands.spawn((
                Node {
                    padding: UiRect::axes(px(4), px(0)),
                    flex_shrink: 0.0,
                    ..default()
                },
                ChildOf(parent),
                children![wrapped(words.clone(), style::CAPTION)],
            ));
        }
        Line::Detail(words) => {
            commands.spawn((
                Node {
                    padding: UiRect {
                        left: px(18),
                        right: px(4),
                        top: px(0),
                        bottom: px(0),
                    },
                    flex_shrink: 0.0,
                    ..default()
                },
                ChildOf(parent),
                children![wrapped(words.clone(), style::CAPTION)],
            ));
        }
        Line::TrayHeading(words) => {
            commands.spawn((
                row_node(),
                Button,
                PanelControl::ToggleTray,
                BackgroundColor(style::ROW),
                ChildOf(parent),
                children![wrapped(words.clone(), style::ACCENT)],
            ));
        }
    }
}

/// A button of the panel's heading.
fn header_button(words: String, control: PanelControl) -> impl Bundle {
    (
        control,
        Button,
        style::button_node(),
        BackgroundColor(style::BUTTON),
        children![style::text(words, style::TEXT)],
    )
}

fn spawn_panel(mut commands: Commands, collapsed: Res<PanelCollapsed>) {
    let display = |shown: bool| if shown { Display::Flex } else { Display::None };
    commands.spawn((
        PanelRoot,
        Node {
            position_type: PositionType::Absolute,
            top: px(0),
            right: px(0),
            bottom: px(0),
            width: px(PANEL_WIDTH + 2.0 * PANEL_MARGIN),
            padding: UiRect::all(px(PANEL_MARGIN)),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            ..default()
        },
        // The room around the panel lets the pointer through to the scene.
        Pickable::IGNORE,
        GlobalZIndex(5),
        children![
            (
                PanelBox,
                Node {
                    display: display(!collapsed.0),
                    width: percent(100),
                    max_height: percent(100),
                    flex_direction: FlexDirection::Column,
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(4)),
                    ..default()
                },
                BackgroundColor(style::PANEL),
                BorderColor::all(style::PANEL_EDGE),
                children![
                    (
                        Node {
                            align_items: AlignItems::Center,
                            column_gap: px(6),
                            padding: UiRect::all(px(6)),
                            flex_shrink: 0.0,
                            ..default()
                        },
                        children![
                            (
                                Node {
                                    flex_grow: 1.0,
                                    ..default()
                                },
                                children![(PanelTitle, style::text("", style::TEXT))],
                            ),
                            (
                                PanelControl::Back,
                                Button,
                                style::button_node(),
                                BackgroundColor(style::BUTTON),
                                children![(
                                    BackWords,
                                    style::text(format!("Back ({})", BACK_KEY.1), style::FAINT)
                                )],
                            ),
                            header_button(
                                format!("Hide ({})", PANEL_KEY.1),
                                PanelControl::Collapse
                            ),
                        ],
                    ),
                    (
                        PanelBody,
                        Node {
                            flex_direction: FlexDirection::Column,
                            flex_shrink: 1.0,
                            min_height: px(0),
                            padding: UiRect {
                                left: px(4),
                                right: px(4),
                                top: px(0),
                                bottom: px(8),
                            },
                            overflow: Overflow {
                                x: OverflowAxis::Clip,
                                y: OverflowAxis::Scroll,
                            },
                            ..default()
                        },
                        ScrollArea,
                        children![
                            (
                                NodeBlock,
                                Node {
                                    flex_direction: FlexDirection::Column,
                                    row_gap: px(1),
                                    flex_shrink: 0.0,
                                    ..default()
                                },
                            ),
                            (
                                TrayBlock,
                                Node {
                                    flex_direction: FlexDirection::Column,
                                    row_gap: px(1),
                                    margin: UiRect::top(px(10)),
                                    flex_shrink: 0.0,
                                    ..default()
                                },
                            ),
                        ],
                    ),
                ],
            ),
            (
                PanelStrip,
                Node {
                    display: display(collapsed.0),
                    ..default()
                },
                children![header_button(
                    format!("Show panel ({})", PANEL_KEY.1),
                    PanelControl::Expand
                )],
            ),
        ],
    ));
}

/// Shows the title, and Back as waiting while there is nowhere to go back
/// to.
fn show_header(
    header: Res<PanelHeader>,
    mut titles: Query<&mut Text, With<PanelTitle>>,
    mut back: Query<&mut TextColor, With<BackWords>>,
) {
    if !header.is_changed() {
        return;
    }
    for mut text in &mut titles {
        if text.0 != header.title {
            text.0.clone_from(&header.title);
        }
    }
    let colour = if header.back {
        style::TEXT
    } else {
        style::FAINT
    };
    for mut words in &mut back {
        words.set_if_neq(TextColor(colour));
    }
}

/// Draws the focused node's lines again when they change, and starts the
/// panel at its top when the focus does.
fn show_node_lines(
    mut commands: Commands,
    lines: Res<NodeLines>,
    focus: Res<Focus>,
    block: Single<(Entity, Option<&Children>), With<NodeBlock>>,
    mut body: Query<&mut ScrollPosition, With<PanelBody>>,
) {
    if focus.is_changed() {
        for mut scroll in &mut body {
            if scroll.0 != Vec2::ZERO {
                scroll.0 = Vec2::ZERO;
            }
        }
    }
    if !lines.is_changed() {
        return;
    }
    let (block, children) = block.into_inner();
    for child in children.into_iter().flatten() {
        commands.entity(*child).despawn();
    }
    for line in &lines.0 {
        spawn_line(&mut commands, block, line);
    }
}

/// Draws the tray's lines again when they change.
fn show_tray_lines(
    mut commands: Commands,
    lines: Res<TrayLines>,
    block: Single<(Entity, &mut Node, Option<&Children>), With<TrayBlock>>,
) {
    if !lines.is_changed() {
        return;
    }
    let (block, mut node, children) = block.into_inner();
    let display = if lines.0.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    if node.display != display {
        node.display = display;
    }
    for child in children.into_iter().flatten() {
        commands.entity(*child).despawn();
    }
    for line in &lines.0 {
        spawn_line(&mut commands, block, line);
    }
}

/// Shows the panel or the button it collapses to.
fn show_collapsed(
    collapsed: Res<PanelCollapsed>,
    mut panels: Query<&mut Node, (With<PanelBox>, Without<PanelStrip>)>,
    mut strips: Query<&mut Node, (With<PanelStrip>, Without<PanelBox>)>,
) {
    if !collapsed.is_changed() {
        return;
    }
    let display = |shown: bool| if shown { Display::Flex } else { Display::None };
    for mut node in &mut panels {
        node.display = display(!collapsed.0);
    }
    for mut node in &mut strips {
        node.display = display(collapsed.0);
    }
}

/// Starts the panel below the bar, whatever the bar's height.
fn place_below_bar(cover: Res<BarCover>, mut roots: Query<&mut Node, With<PanelRoot>>) {
    if !cover.is_changed() {
        return;
    }
    let top = percent(100.0 * cover.0.unwrap_or(0.0));
    for mut node in &mut roots {
        if node.top != top {
            node.top = top;
        }
    }
}

/// The fraction of a viewport `viewport` pixels wide that a panel `panel`
/// pixels wide covers, or `None` before either has been laid out.
pub fn cover_fraction(panel: f32, viewport: f32) -> Option<f32> {
    (panel > 0.0 && viewport > 0.0).then(|| (panel / viewport).min(1.0))
}

/// Tells the camera and the labels how much of the viewport's width the
/// open panel covers: none while it is collapsed.
fn measure_cover(
    collapsed: Res<PanelCollapsed>,
    roots: Query<&ComputedNode, With<PanelRoot>>,
    cameras: Query<&Camera, With<IsDefaultUiCamera>>,
    mut cover: ResMut<PanelCover>,
) {
    if collapsed.0 {
        cover.set_if_neq(PanelCover(0.0));
        return;
    }
    let panel = roots.iter().map(|node| node.size().x).fold(0.0, f32::max);
    let viewport = cameras
        .iter()
        .find_map(|camera| camera.physical_viewport_size())
        .map_or(0.0, |size| size.x as f32);
    if let Some(fraction) = cover_fraction(panel, viewport) {
        cover.set_if_neq(PanelCover(fraction));
    }
}

/// `P` shows or hides the panel, unless a text field has the keyboard.
fn toggle_on_key(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiFocus>,
    mut collapsed: ResMut<PanelCollapsed>,
) {
    if !ui.typing && keys.just_pressed(PANEL_KEY.0) {
        collapsed.0 = !collapsed.0;
    }
}

/// Colours the panel's buttons and rows by whether they are under the
/// pointer, and Back by whether it has somewhere to go.
fn style_controls(
    header: Res<PanelHeader>,
    mut controls: Query<(&PanelControl, &Interaction, &mut BackgroundColor)>,
) {
    for (control, interaction, mut background) in &mut controls {
        let rest = match control {
            PanelControl::Focus(_) | PanelControl::ToggleTray => style::ROW,
            _ => style::BUTTON,
        };
        let waiting = *control == PanelControl::Back && !header.back;
        let colour = style::button_colour(rest, false, *interaction != Interaction::None, waiting);
        background.set_if_neq(BackgroundColor(colour));
    }
}

/// Does what a click on the panel asks. The click climbs from the text
/// clicked to its row or button.
fn on_click(
    mut click: On<Pointer<Click>>,
    controls: Query<&PanelControl>,
    mut collapsed: ResMut<PanelCollapsed>,
    mut open: ResMut<TrayOpen>,
    mut focus: ResMut<Focus>,
    mut back: MessageWriter<GoBack>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(control) = controls.get(click.entity) else {
        return;
    };
    click.propagate(false);
    match *control {
        PanelControl::Back => {
            back.write(GoBack);
        }
        PanelControl::Collapse => collapsed.0 = true,
        PanelControl::Expand => collapsed.0 = false,
        PanelControl::ToggleTray => open.0 = !open.0,
        PanelControl::Focus(node) => {
            focus.set_if_neq(Focus(Some(node)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::{Location, PointerId};

    use super::*;
    use crate::graph_index::tests::{edge, node};
    use crate::hub::{ImpreciseInstant, ValidityInterval, parse_snapshot};
    use crate::layout::place;
    use crate::time_axis::{ModeTimes, Profile, Span, Unplaced};

    fn fixture_index() -> (GraphIndex, TimeModel) {
        let snapshot = parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        let layout = place(&snapshot.nodes, &snapshot.edges);
        let index = crate::graph_index::build(&snapshot.nodes, &snapshot.edges, &layout);
        let model = crate::time_axis::build(&snapshot, &layout, 0);
        (index, model)
    }

    #[test]
    fn the_panel_shows_every_field_of_the_contracts_node_view() {
        let (index, _) = fixture_index();
        for view in &index.views {
            let serialised = serde_json::to_value(view).unwrap();
            let fields: BTreeSet<String> =
                serialised.as_object().unwrap().keys().cloned().collect();
            let shown: BTreeSet<String> = node_fields(view)
                .iter()
                .map(|f| f.field.to_owned())
                .collect();
            assert_eq!(shown, fields, "every NodeView field, and no other");
            let names: BTreeSet<&str> = node_fields(view).iter().map(|f| f.name).collect();
            assert_eq!(names.len(), fields.len(), "each under its own name");
        }
    }

    #[test]
    fn a_field_with_nothing_in_it_says_so_in_plain_words() {
        let (index, _) = fixture_index();
        let topic = &index.views[index.by_id["a1e0c7d2-0000-4000-8000-000000000001"]];
        let value = |view: &NodeView, field: &str| {
            node_fields(view)
                .into_iter()
                .find(|f| f.field == field)
                .unwrap()
                .value
        };
        assert_eq!(value(topic, "confidence"), "unrated");
        assert_eq!(value(topic, "retrieved_at"), "never returned by a search");
        assert_eq!(value(topic, "source_id"), "none");
        assert_eq!(value(topic, "metadata"), "empty");
        assert_eq!(value(topic, "node_type"), "topic");
        assert_eq!(value(topic, "created_at"), "2026-07-23 22:20:22 UTC");
        let fact = &index.views[index.by_id["fac70000-0000-4000-8000-000000000001"]];
        assert_eq!(value(fact, "confidence"), "0.9");
        assert_eq!(value(fact, "description"), "empty");
        assert_eq!(
            value(fact, "content"),
            "Common swifts winter in sub-Saharan Africa."
        );
        assert_eq!(value(fact, "metadata"), "1 key, below");
        let lines = field_lines(fact);
        assert_eq!(
            lines.last(),
            Some(&Line::Field {
                name: "confidence_basis".into(),
                value: "stated in the source".into()
            }),
            "the metadata as key and value rows"
        );
        let swifts = &index.views[index.by_id["a1e0c7d2-0000-4000-8000-000000000002"]];
        assert_eq!(value(swifts, "retrieved_at"), "2026-10-19 08:00:00 UTC");
        assert_eq!(stamp("soon"), "soon", "a stamp that does not read");
        assert_eq!(
            metadata_value(&serde_json::json!({"a": [1, 2]})),
            r#"{"a":[1,2]}"#
        );
    }

    /// Drawn: a fact f, an inference i, a fact g the filter hides, and a
    /// topic t. f supports i, g supports i, f was corrected from old-f,
    /// which the snapshot does not hold, i followed old-i, f and g were
    /// merged and f relates to itself.
    fn edged() -> GraphIndex {
        let nodes = vec![
            node("t", NodeType::Topic, 1),
            node("f", NodeType::Fact, 2),
            node("i", NodeType::Inference, 3),
            node("g", NodeType::Fact, 4),
        ];
        let edges = vec![
            edge("f", "t", "extracted_under_topic"),
            edge("f", "i", "supports"),
            edge("g", "i", "supports"),
            edge("old-f", "f", "superseded_by"),
            edge("i", "old-i", "temporally_followed_by"),
            edge("g", "f", "merged_into"),
            edge("f", "f", "related"),
            edge("f", "gone", "assessed"),
        ];
        crate::graph_index::build(&nodes, &edges, &place(&nodes, &edges))
    }

    #[test]
    fn the_edges_are_grouped_by_class_and_counted_with_their_far_ends() {
        let index = edged();
        let at = |id: &str| index.by_id[id];
        let (f, i, g) = (at("f"), at("i"), at("g"));
        let mut visible = vec![true; 4];
        visible[g] = false;
        let claims = edge_group(&index.edges, &index.edges.claim[f], f, &visible);
        let rows: Vec<(&str, bool, &Far)> = claims
            .rows
            .iter()
            .map(|r| (r.edge_type.as_str(), r.outgoing, &r.far))
            .collect();
        assert_eq!(
            rows,
            [
                ("assessed", true, &Far::Absent("gone".into())),
                ("related", true, &Far::Itself),
                (
                    "supports",
                    true,
                    &Far::Drawn {
                        node: i,
                        hidden: false
                    }
                ),
            ],
            "by type"
        );
        assert_eq!((claims.absent, claims.hidden), (1, 0));
        assert_eq!(
            edge_heading("Claim to claim", &claims),
            "Claim to claim (3, 1 not in the snapshot)"
        );

        let successions = edge_group(&index.edges, &index.edges.succession[f], f, &visible);
        assert_eq!(successions.rows.len(), 2);
        assert_eq!((successions.absent, successions.hidden), (1, 1));
        assert_eq!(
            edge_heading("Succession and merge", &successions),
            "Succession and merge (2, 1 not in the snapshot, 1 hidden by the filter)"
        );
        let lines: Vec<Line> = successions
            .rows
            .iter()
            .map(|r| edge_line(r, &index))
            .collect();
        assert_eq!(
            lines,
            [
                Line::Link {
                    caption: "<- merged_into".into(),
                    text: "g (hidden by the filter)".into(),
                    node: Some(g),
                    faint: true,
                },
                Line::Link {
                    caption: "<- superseded_by".into(),
                    text: "not in the snapshot: old-f".into(),
                    node: None,
                    faint: true,
                },
            ],
            "the old end named from the edge, the hidden one by its label"
        );

        let inference = edge_group(&index.edges, &index.edges.succession[i], i, &visible);
        assert_eq!(
            edge_line(&inference.rows[0], &index),
            Line::Link {
                caption: "temporally_followed_by ->".into(),
                text: "not in the snapshot: old-i".into(),
                node: None,
                faint: true,
            }
        );
        let supported = edge_group(&index.edges, &index.edges.claim[i], i, &visible);
        assert_eq!(
            edge_heading("Claim to claim", &supported),
            "Claim to claim (2, 1 hidden by the filter)"
        );
        assert!(supported.rows.iter().all(|r| !r.outgoing), "both come in");
        let topic = edge_lines("Claim to claim", &EdgeGroup::default(), &index);
        assert_eq!(
            topic,
            [
                Line::Heading("Claim to claim (0)".into()),
                Line::Note("none".into())
            ]
        );
    }

    #[test]
    fn membership_names_the_topics_and_metacontexts_and_lists_the_documents() {
        let nodes = vec![
            node("parent", NodeType::Topic, 1),
            node("t", NodeType::Topic, 2),
            node("f", NodeType::Fact, 3),
            node("g", NodeType::Fact, 4),
        ];
        let edges = vec![
            edge("t", "parent", "subtopic_of"),
            edge("f", "t", "extracted_under_topic"),
            edge("f", "parent", "tagged_with_topic"),
            edge("f", "the-real", "has_metacontext"),
            edge("f", "doc-1", "sourced_from"),
            edge("g", "parent", "tagged_with_topic"),
        ];
        let index = crate::graph_index::build(&nodes, &edges, &place(&nodes, &edges));
        let at = |id: &str| index.by_id[id];
        let names: BTreeMap<&str, &str> = [("the-real", "The Real")].into();
        let lines = membership_lines(&index, at("f"), Some(Filing::Filed), &names);
        assert_eq!(
            lines,
            [
                Line::Heading("Membership (4)".into()),
                topic_link(&index, at("t"), "Filed under"),
                topic_link(&index, at("parent"), "Tag"),
                Line::Field {
                    name: "Metacontext".into(),
                    value: "The Real (the-real)".into()
                },
                Line::Field {
                    name: "Source document".into(),
                    value: "doc-1".into()
                },
            ]
        );
        let tagged = membership_lines(&index, at("g"), Some(Filing::Tagged), &names);
        assert_eq!(
            tagged[1],
            topic_link(&index, at("parent"), "Placed by its tag")
        );
        let parent = membership_lines(&index, at("parent"), None, &names);
        assert_eq!(
            parent,
            [
                Line::Heading("Membership (1)".into()),
                topic_link(&index, at("t"), "Subtopic"),
                Line::Field {
                    name: "Claims beside it".into(),
                    value: "1".into()
                },
            ],
            "g stands beside it by its tag"
        );
        let child = membership_lines(&index, at("t"), None, &names);
        assert_eq!(child[1], topic_link(&index, at("parent"), "Subtopic of"));
    }

    #[test]
    fn the_time_section_places_the_node_in_both_modes_and_names_what_has_no_coordinate() {
        let micros = |t: &str| parse_timestamp(t).unwrap();
        let model = TimeModel {
            linked: vec![
                vec![],
                vec![
                    Linked {
                        timepoint_id: "p-spring-0001".into(),
                        label: Some("the spring".into()),
                        place: Ok(Placement::Smear {
                            earliest: Some(micros("2023-03-01T00:00:00Z")),
                            latest: None,
                        }),
                    },
                    Linked {
                        timepoint_id: "p-late-0002".into(),
                        label: None,
                        place: Ok(Placement::Interval {
                            start: micros("2024-01-01T00:00:00Z"),
                            end: micros("2025-01-01T00:00:00Z"),
                        }),
                    },
                    Linked {
                        timepoint_id: "p-none-0003".into(),
                        label: None,
                        place: Err(Unplaced::NoBounds),
                    },
                ],
            ],
            validity: vec![
                vec![],
                vec![crate::time_axis::read_validity(
                    "e",
                    "d0c00000-0000",
                    &ValidityInterval {
                        start: ImpreciseInstant::Precise {
                            at: "2026-04-20T00:00:00Z".into(),
                            label: None,
                        },
                        end: ImpreciseInstant::Unknown,
                        timeline_id: None,
                        witnessed_at: None,
                        basis: "inferred".into(),
                    },
                )],
            ],
            claim: ModeTimes {
                spans: vec![
                    None,
                    Some(Span {
                        lo: 0.0,
                        hi: 1.0,
                        profile: Profile::FadeTop,
                        clipped: Default::default(),
                    }),
                ],
                ..ModeTimes::default()
            },
            ..TimeModel::default()
        };
        let lines = time_lines(&model, Kind::Fact, 1, "2026-10-20T11:17:45.5Z");
        assert_eq!(
            lines,
            [
                Line::Heading("Time".into()),
                Line::Field {
                    name: "Recorded time".into(),
                    value: "a disc at 2026-10-20 11:17:45 UTC".into()
                },
                Line::Field {
                    name: "Claim time".into(),
                    value: "drawn by \"the spring\", timepoint p-spring".into()
                },
                Line::Detail(
                    "\"the spring\", timepoint p-spring: vague, after 2023-03-01, its column"
                        .into()
                ),
                Line::Detail(
                    "timepoint p-late-0: interval from 2024-01-01 to 2025-01-01, also linked"
                        .into()
                ),
                Line::Detail("timepoint p-none-0: no place: vague, with no bounds".into()),
                Line::Field {
                    name: "Valid from".into(),
                    value: "precise, 2026-04-20".into()
                },
                Line::Field {
                    name: "Valid until".into(),
                    value: "unknown, no coordinate".into()
                },
                Line::Detail("inferred from tense and context, from document d0c00000".into()),
            ]
        );
        let unlinked = time_lines(&model, Kind::Fact, 0, "2026-10-20T00:00:00Z");
        assert_eq!(
            unlinked[2],
            Line::Field {
                name: "Claim time".into(),
                value: "no timepoint, so a disc at its recorded time".into()
            }
        );
        assert_eq!(
            unlinked[3],
            Line::Field {
                name: "Validity".into(),
                value: "none from its sources".into()
            }
        );
        let mut unplaced = model.clone();
        unplaced.claim.spans[1] = None;
        unplaced.linked[1].retain(|l| l.place.is_err());
        let lines = time_lines(&unplaced, Kind::Fact, 1, "");
        assert_eq!(
            lines[2],
            Line::Field {
                name: "Claim time".into(),
                value: "not drawn: none of its timepoints has a place".into()
            }
        );
        let topic = time_lines(&model, Kind::Topic, 1, "");
        assert!(matches!(
            &topic[2],
            Line::Field { value, .. } if value == "a disc at its recorded time, as every topic is"
        ));
    }

    #[test]
    fn without_a_focus_the_panel_says_how_to_get_one_and_the_tray_shows_in_claim_time() {
        let (index, model) = fixture_index();
        let context = Context {
            index: &index,
            model: &model,
            visible: &[],
            filing: None,
            metacontexts: BTreeMap::new(),
            map: None,
        };
        let (title, lines) = node_lines(&context, None);
        assert_eq!(title, "Side panel");
        assert_eq!(lines, [Line::Note(HINT.into())]);
        let tray = crate::tray::tray_for(&model);
        assert!(tray_lines(&tray, true, &index, TimeMode::Recorded).is_empty());
        let lines = tray_lines(&tray, true, &index, TimeMode::Claim);
        assert!(matches!(&lines[0], Line::TrayHeading(h) if h.starts_with("Claim-time tray")));
        assert_eq!(tray_lines(&tray, false, &index, TimeMode::Claim).len(), 1);
    }

    #[test]
    fn a_focused_node_has_every_section() {
        let (index, model) = fixture_index();
        let context = Context {
            index: &index,
            model: &model,
            visible: &[],
            filing: None,
            metacontexts: BTreeMap::new(),
            map: None,
        };
        let fact = index.by_id["fac70000-0000-4000-8000-000000000001"];
        let (title, lines) = node_lines(&context, Some(fact));
        assert_eq!(title, "Focused fact");
        let headings: Vec<&str> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Heading(h) => Some(h.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            [
                "Membership (3)",
                "Claim to claim (1)",
                "Succession and merge (0)",
                "Time",
            ]
        );
        let inference = index.by_id["1fe00000-0000-4000-8000-000000000001"];
        assert!(lines.contains(&Line::Link {
            caption: "supports ->".into(),
            text: "Swifts spend most of the year away from\u{2026}".into(),
            node: Some(inference),
            faint: false,
        }));
    }

    /// The panel with no window and no renderer.
    fn app() -> App {
        let (index, model) = fixture_index();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(index)
            .insert_resource(model)
            .add_plugins(PanelPlugin);
        app.world_mut().register_component::<Window>();
        app.update();
        app
    }

    fn click(app: &mut App, entity: Entity) {
        let location = Location {
            target: NormalizedRenderTarget::None {
                width: 1,
                height: 1,
            },
            position: Vec2::ZERO,
        };
        let event = Click {
            button: PointerButton::Primary,
            hit: HitData::new(entity, 0.0, None, None),
            duration: Duration::ZERO,
            count: 1,
        };
        app.world_mut()
            .trigger(Pointer::new(PointerId::Mouse, location, event, entity));
        app.update();
    }

    fn rows(app: &mut App) -> Vec<(Entity, PanelControl)> {
        let mut query = app.world_mut().query::<(Entity, &PanelControl)>();
        query.iter(app.world()).map(|(e, c)| (e, *c)).collect()
    }

    #[test]
    fn a_click_on_a_node_named_in_the_panel_focuses_it_and_back_asks_for_the_one_before() {
        let mut app = app();
        assert_eq!(
            app.world().resource::<NodeLines>().0,
            [Line::Note(HINT.into())]
        );
        let (fact, inference) = {
            let index = app.world().resource::<GraphIndex>();
            (
                index.by_id["fac70000-0000-4000-8000-000000000001"],
                index.by_id["1fe00000-0000-4000-8000-000000000001"],
            )
        };
        app.insert_resource(Focus(Some(fact)));
        app.update();
        assert_eq!(app.world().resource::<PanelHeader>().title, "Focused fact");
        let row = rows(&mut app)
            .into_iter()
            .find(|(_, c)| *c == PanelControl::Focus(inference))
            .map(|(e, _)| e)
            .expect("a row naming the inference it supports");
        // The click lands on the row's text and climbs to the row.
        let words = app.world().get::<Children>(row).unwrap()[0];
        click(&mut app, words);
        assert_eq!(app.world().resource::<Focus>().0, Some(inference));
        let back = rows(&mut app)
            .into_iter()
            .find(|(_, c)| *c == PanelControl::Back)
            .unwrap()
            .0;
        click(&mut app, back);
        let asked = app
            .world_mut()
            .resource_mut::<Messages<GoBack>>()
            .drain()
            .count();
        assert_eq!(asked, 1);
    }

    #[test]
    fn p_and_the_hide_button_collapse_the_panel_and_its_cover() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(PANEL_KEY.0);
        app.update();
        assert!(app.world().resource::<PanelCollapsed>().0);
        assert_eq!(app.world().resource::<PanelCover>().0, 0.0);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(PANEL_KEY.0);
        keys.clear();
        let expand = rows(&mut app)
            .into_iter()
            .find(|(_, c)| *c == PanelControl::Expand)
            .unwrap()
            .0;
        click(&mut app, expand);
        assert!(!app.world().resource::<PanelCollapsed>().0);
        let hide = rows(&mut app)
            .into_iter()
            .find(|(_, c)| *c == PanelControl::Collapse)
            .unwrap()
            .0;
        click(&mut app, hide);
        assert!(app.world().resource::<PanelCollapsed>().0);
        assert_eq!(cover_fraction(396.0, 1280.0), Some(396.0 / 1280.0));
        assert_eq!(cover_fraction(0.0, 1280.0), None);
    }

    #[test]
    fn a_node_changed_in_place_shows_its_new_fields() {
        let mut app = app();
        let fact =
            app.world().resource::<GraphIndex>().by_id["fac70000-0000-4000-8000-000000000001"];
        app.insert_resource(Focus(Some(fact)));
        app.update();
        let before = app.world().resource::<NodeLines>().0.clone();
        app.world_mut().resource_mut::<GraphIndex>().views[fact].description =
            "Seen again in 2026.".into();
        app.update();
        let after = app.world().resource::<NodeLines>().0.clone();
        assert_ne!(before, after);
        assert!(after.contains(&Line::Field {
            name: "Description".into(),
            value: "Seen again in 2026.".into()
        }));
    }

    #[test]
    fn a_node_beyond_the_axis_is_listed_in_the_tray_and_its_row_focuses_it() {
        let (index, mut model) = fixture_index();
        let fact = index.by_id["fac70000-0000-4000-8000-000000000001"];
        let far = Placement::Interval {
            start: parse_timestamp("1372-01-01T00:00:00Z").unwrap(),
            end: parse_timestamp("1400-01-01T00:00:00Z").unwrap(),
        };
        model.claim_places[fact] = Some(far);
        model.claim.spans[fact] = Some(crate::clip::clipped_span(&far, &model.claim.axis));
        let tray = crate::tray::tray_for(&model);
        let lines = tray_lines(&tray, true, &index, TimeMode::Claim);
        assert!(matches!(&lines[0], Line::TrayHeading(h) if h.contains("1 beyond the axis")));
        assert_eq!(lines[1], Line::Heading("Beyond the axis (1)".into()));
        let row = lines
            .iter()
            .position(|l| matches!(l, Line::Link { node: Some(n), .. } if *n == fact))
            .expect("a row for the clipped claim");
        assert_eq!(
            lines[row + 1],
            Line::Detail("from 1372-01-01 to 1400-01-01, before the axis".into())
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, Line::Heading(h) if h.starts_with("Not drawn, or with more"))),
            "the claims claim time cannot place follow"
        );

        // Focused, its time section says it is drawn clipped, with its dates.
        let context = Context {
            index: &index,
            model: &model,
            visible: &[],
            filing: None,
            metacontexts: BTreeMap::new(),
            map: None,
        };
        let (_, node) = node_lines(&context, Some(fact));
        assert!(node.iter().any(|l| matches!(
            l,
            Line::Field { name, value } if name == "Beyond the axis"
                && value.starts_with("from 1372-01-01 to 1400-01-01, before the axis, which runs from")
                && value.ends_with("so drawn flat at its foot")
        )));

        let mut app = app();
        app.insert_resource(model)
            .insert_resource(tray)
            .insert_resource(TimeMode::Claim);
        app.update();
        let row = rows(&mut app)
            .into_iter()
            .find(|(_, c)| *c == PanelControl::Focus(fact))
            .expect("the row in the tray")
            .0;
        click(&mut app, row);
        assert_eq!(app.world().resource::<Focus>().0, Some(fact));
    }

    #[test]
    fn the_tray_heading_opens_and_closes_its_list_in_claim_time() {
        let mut app = app();
        app.insert_resource(TimeMode::Claim);
        app.insert_resource(crate::tray::tray_for(
            &app.world().resource::<TimeModel>().clone(),
        ));
        app.update();
        let heading = rows(&mut app)
            .into_iter()
            .find(|(_, c)| *c == PanelControl::ToggleTray)
            .expect("the tray's heading in claim time")
            .0;
        let open = app.world().resource::<TrayOpen>().0;
        click(&mut app, heading);
        assert_eq!(app.world().resource::<TrayOpen>().0, !open);
        app.insert_resource(TimeMode::Recorded);
        app.update();
        assert!(app.world().resource::<TrayLines>().0.is_empty());
    }
}
