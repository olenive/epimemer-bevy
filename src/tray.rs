//! The tray (DESIGN.md §4, §10): what claim time cannot put on the axis.
//!
//! [`Tray`] holds the claims linked to a timepoint that has no position,
//! with the reason (no bounds, contested, a kind this client does not
//! know, not in the snapshot), and the claims linked to more than one
//! timepoint, with the ones their column leaves out (`time_axis.rs`). It
//! also holds the nodes beyond the axis: those claim time draws clipped to
//! an end of the claim axis, with their true dates (`clip.rs`).
//!
//! The side panel shows it as a section of its own whenever claim time is
//! drawn (`panel.rs`), collapsed to its heading by a click on it, and a
//! click on a node's row focuses it. A claim with no position at all is
//! not drawn in claim time, so its row is the way to reach it. The focused
//! node's own timepoints and validity are the panel's time section.

use bevy::prelude::*;

use crate::clip::{BeyondEntry, beyond_tray};
use crate::graph_index::GraphIndex;
use crate::labels::label_text;
use crate::time_axis::{TimeModel, TrayEntry, standing_tray};

/// What the tray lists.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Tray {
    /// Every claim linked to a timepoint with no position or to several, in
    /// layout order, while it is in the active set.
    pub standing: Vec<TrayEntry>,
    /// Every node claim time draws clipped to an end of the axis, those
    /// before it first, while it is in the active set.
    pub beyond: Vec<BeyondEntry>,
}

/// Whether the tray's list is open.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayOpen(pub bool);

impl Default for TrayOpen {
    fn default() -> Self {
        TrayOpen(true)
    }
}

pub struct TrayPlugin;

impl Plugin for TrayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Tray>()
            .init_resource::<TrayOpen>()
            .init_resource::<TimeModel>()
            .add_systems(Update, refresh_tray);
    }
}

/// The tray of a model.
pub fn tray_for(model: &TimeModel) -> Tray {
    Tray {
        standing: standing_tray(model),
        beyond: beyond_tray(model),
    }
}

/// Keeps [`Tray`] up to date with the model, leaving out the claims that
/// have left the active set.
fn refresh_tray(model: Res<TimeModel>, index: Option<Res<GraphIndex>>, mut tray: ResMut<Tray>) {
    let index_changed = index.as_ref().is_some_and(|i| i.is_changed());
    if model.is_changed() || index_changed {
        let mut next = tray_for(&model);
        if let Some(index) = index.as_deref() {
            let active = |node: usize| index.active.get(node).copied().unwrap_or(true);
            next.standing.retain(|entry| active(entry.node));
            next.beyond.retain(|entry| active(entry.node));
        }
        tray.set_if_neq(next);
    }
}

/// The tray's heading: how many nodes it lists, and whether a click opens
/// or closes it.
pub fn heading(tray: &Tray, open: bool) -> String {
    let unplaced = tray.standing.iter().filter(|e| !e.placed).count();
    let several = tray.standing.len() - unplaced;
    format!(
        "Claim-time tray: {unplaced} not drawn, {several} with more timepoints, {} beyond the axis ({})",
        tray.beyond.len(),
        if open { "close" } else { "open" }
    )
}

/// A node's label in the tray.
fn node_label(node: usize, index: &GraphIndex) -> String {
    match (index.names.get(node), index.ids.get(node)) {
        (Some(name), Some(id)) => label_text(name, id),
        _ => format!("node {node}"),
    }
}

/// A node's row in the tray's "beyond the axis" section: its label.
pub fn beyond_text(entry: &BeyondEntry, index: &GraphIndex) -> String {
    node_label(entry.node, index)
}

/// A claim's row in the tray: its label, and whether it is drawn.
pub fn entry_text(entry: &TrayEntry, index: &GraphIndex) -> String {
    let label = node_label(entry.node, index);
    if entry.placed {
        format!("{label} (drawn by its earliest)")
    } else {
        format!("{label} (not drawn)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_axis::{TrayLine, Unplaced, line_text};

    fn entry(node: usize, placed: bool) -> TrayEntry {
        TrayEntry {
            node,
            placed,
            lines: vec![TrayLine::Unplaced {
                timepoint: "since the MCP".into(),
                reason: Unplaced::NoBounds,
            }],
        }
    }

    #[test]
    fn the_heading_counts_what_the_tray_lists() {
        let tray = Tray {
            standing: vec![entry(0, false), entry(1, true), entry(2, false)],
            beyond: vec![BeyondEntry {
                node: 3,
                placement: crate::time_axis::Placement::Instant(0),
                clipped: crate::clip::Clipped { below: 1, above: 0 },
            }],
        };
        assert_eq!(
            heading(&tray, true),
            "Claim-time tray: 2 not drawn, 1 with more timepoints, 1 beyond the axis (close)"
        );
        assert!(heading(&Tray::default(), false).ends_with("(open)"));
    }

    #[test]
    fn a_row_names_its_claim_and_whether_it_is_drawn() {
        let index = GraphIndex {
            ids: vec!["a".into(), "b".into()],
            names: vec!["The MCP server starts the hub.".into(), String::new()],
            ..GraphIndex::default()
        };
        assert_eq!(
            entry_text(&entry(0, false), &index),
            "The MCP server starts the hub. (not drawn)"
        );
        assert_eq!(
            entry_text(&entry(1, true), &index),
            "b (drawn by its earliest)"
        );
        assert_eq!(
            entry_text(&entry(7, true), &index),
            "node 7 (drawn by its earliest)"
        );
    }

    #[test]
    fn the_tray_follows_the_model_and_leaves_out_nodes_gone() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(TrayPlugin);
        let model = TimeModel {
            linked: vec![
                vec![],
                vec![crate::time_axis::Linked {
                    timepoint_id: "p".into(),
                    label: Some("in the Crown".into()),
                    place: Err(Unplaced::NoBounds),
                }],
            ],
            validity: vec![vec![], vec![]],
            claim: crate::time_axis::ModeTimes {
                spans: vec![None, None],
                ..Default::default()
            },
            ..TimeModel::default()
        };
        app.insert_resource(model).insert_resource(GraphIndex {
            active: vec![true, true],
            ..GraphIndex::default()
        });
        app.update();
        let tray = app.world().resource::<Tray>().clone();
        assert_eq!(tray.standing.len(), 1);
        assert_eq!(tray.standing[0].node, 1);
        assert!(!tray.standing[0].placed);
        assert_eq!(
            line_text(&tray.standing[0].lines[0]),
            "in the Crown: no place, vague, with no bounds"
        );
        app.world_mut().resource_mut::<GraphIndex>().active[1] = false;
        app.update();
        assert!(app.world().resource::<Tray>().standing.is_empty());
    }

    #[test]
    fn the_beyond_section_follows_the_model_and_leaves_out_nodes_gone() {
        use crate::clip::{Clipped, clipped_span};
        use crate::layout::TimeAxis;
        use crate::time_axis::{ModeTimes, Placement};

        let day = |text: &str| crate::hub::parse_timestamp(text).unwrap();
        let axis = TimeAxis {
            start: day("2026-07-01T00:00:00Z"),
            span: day("2026-11-01T00:00:00Z") - day("2026-07-01T00:00:00Z"),
            height: 400.0,
        };
        let places = vec![
            Some(Placement::Recorded(day("2026-08-01T00:00:00Z"))),
            Some(Placement::Interval {
                start: day("1860-01-01T00:00:00Z"),
                end: day("1861-01-01T00:00:00Z"),
            }),
            Some(Placement::Instant(day("2100-01-01T00:00:00Z"))),
        ];
        let model = TimeModel {
            claim: ModeTimes {
                axis,
                spans: places
                    .iter()
                    .map(|p| p.map(|p| clipped_span(&p, &axis)))
                    .collect(),
            },
            linked: vec![vec![]; 3],
            validity: vec![vec![]; 3],
            claim_places: places,
            ..TimeModel::default()
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(TrayPlugin);
        app.insert_resource(model).insert_resource(GraphIndex {
            ids: vec!["a".into(), "b".into(), "c".into()],
            names: vec![
                "Recorded in August.".into(),
                "What Milanese people knew by 1860.".into(),
                "A claim about 2100.".into(),
            ],
            active: vec![true, true, true],
            ..GraphIndex::default()
        });
        app.update();
        let tray = app.world().resource::<Tray>().clone();
        let listed: Vec<(usize, Clipped)> =
            tray.beyond.iter().map(|e| (e.node, e.clipped)).collect();
        assert_eq!(
            listed,
            [
                (1, Clipped { below: 2, above: 0 }),
                (2, Clipped { below: 0, above: 1 })
            ],
            "the earlier first; the disc in August is on the axis"
        );
        assert!(heading(&tray, true).contains("2 beyond the axis"));
        let index = app.world().resource::<GraphIndex>().clone();
        assert_eq!(
            beyond_text(&tray.beyond[0], &index),
            "What Milanese people knew by 1860."
        );
        app.world_mut().resource_mut::<GraphIndex>().active[1] = false;
        app.update();
        let tray = app.world().resource::<Tray>();
        assert_eq!(tray.beyond.len(), 1);
        assert_eq!(tray.beyond[0].node, 2);
    }
}
