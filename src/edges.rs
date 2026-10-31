//! Edges (DESIGN.md §5): claim-to-claim lines on hover, on focus and with
//! the edge toggle, and the succession and merge connectors, always drawn.
//!
//! **Hover and focus.** The hovered node's claim-to-claim edges and the
//! focused node's are drawn as straight lines from its disc to the disc at
//! each far end, each disc at its own height, in the edge's colour
//! (`scene.rs`), with Bevy's gizmos, since they follow the pointer from one
//! update to the next and are few. They come from the node's own list of
//! edges (`edge_index.rs`), so an update costs as much as the two nodes'
//! degrees. The focus's lines are wider and stronger than the hover's and
//! stay until the focus changes. A line's alpha follows the dimmer of its
//! two nodes as they are drawn: the lit node is whole, and a far end takes
//! the filter's alpha and the focus's falloff, raised a little for being
//! near the hover or the focus. An edge to a node that is not drawn, or
//! that the filter hides, draws nothing.
//!
//! **The edge toggle**, [`AllEdges`], `L` and its button in the bar, draws
//! every claim-to-claim edge between two nodes the filter leaves visible,
//! as one mesh of hardware lines, rebuilt only in an update in which the
//! toggle, the filter, the focus or the graph changed, never while the
//! scene stands still.
//!
//! **Connectors** run from the old node of a succession or merge edge to
//! the new one, always, as a second mesh of lines rebuilt on the same
//! changes, except on a change of toggle. Between two nodes the filter
//! leaves visible, a connector runs from the new node straight down its
//! own axis to the old node's height (up, where the old node is the later),
//! then across to the old node: in the timeline view it is a vertical under
//! the new node, in the graph view a line on the map. The old end is
//! usually not in the snapshot, which holds active nodes only: then a stub
//! of [`STUB_LENGTH`] runs down from the new node to a small cross, so that
//! the node reads as one with a history. Where the new end is the one
//! missing, the stub runs up from the old node. Where the far end is drawn
//! but hidden by the filter, the stub points toward it and ends open. A
//! connector from a fact to the inference that depended on it is drawn in
//! the second connector colour. A connector's alpha follows the new node's,
//! or the old node's where only that one is visible.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use crate::camera::Places;
use crate::edge_index::{EdgeIndex, End};
use crate::filter::{RefreshShown, Shown, filter_alpha, is_visible};
use crate::focus::{self, Focus, Highlight, Hovered, Near};
use crate::graph_index::GraphIndex;
use crate::keys::EDGE_KEY;
use crate::scene::{
    FOCUS_LINE_ALPHA, FOCUS_LINE_WIDTH, HOVER_LINE_ALPHA, HOVER_LINE_WIDTH, NEAR_FOCUS_LIFT,
    NEAR_HOVER_LIFT, relation_colour, succession_colour,
};
use crate::ui_focus::UiFocus;

/// How long a stub is, in world units, where a connector's far end is not
/// drawn: a few discs' height, short of a day's layer on the development
/// graph.
pub const STUB_LENGTH: f32 = 5.0;
/// Half the width of the cross at the foot of a stub to an absent end.
pub const MARKER_SIZE: f32 = 0.9;

/// Whether every claim-to-claim edge between visible nodes is drawn: the
/// edge toggle.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllEdges(pub bool);

/// A straight piece of line, in a linear colour whose alpha scales how much
/// it adds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub from: Vec3,
    pub to: Vec3,
    pub colour: LinearRgba,
}

/// The entity drawing the edge toggle's lines.
#[derive(Component)]
pub struct ClaimLines;

/// The entity drawing the connectors.
#[derive(Component)]
pub struct Connectors;

/// The two line meshes, made once and rewritten in place.
#[derive(Resource, Clone, Debug)]
pub struct EdgeMeshes {
    pub claim: Handle<Mesh>,
    pub connectors: Handle<Mesh>,
}

/// The gizmo lines the hover lights, and those the focus lights, each with
/// its own width.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct HoverLines;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct FocusLines;

/// Draws the edges. The app inserts the [`GraphIndex`], [`Places`] and
/// the filter before it runs, and may insert [`AllEdges`] to start with
/// the toggle on.
pub struct EdgePlugin;

impl Plugin for EdgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AllEdges>()
            .init_resource::<UiFocus>()
            .init_resource::<Focus>()
            .init_resource::<Hovered>()
            .insert_gizmo_config(HoverLines, gizmo_config(HOVER_LINE_WIDTH))
            .insert_gizmo_config(FocusLines, gizmo_config(FOCUS_LINE_WIDTH))
            .add_systems(Startup, spawn_line_meshes)
            .add_systems(Update, toggle_on_key)
            .add_systems(
                PostUpdate,
                (rebuild_claim_lines, rebuild_connectors, draw_lit_lines).after(RefreshShown),
            );
    }
}

fn gizmo_config(width: f32) -> GizmoConfig {
    GizmoConfig {
        line: GizmoLineConfig { width, ..default() },
        ..default()
    }
}

/// `alpha` raised by `lift` of the way toward whole.
fn lifted(alpha: f32, lift: f32) -> f32 {
    alpha + (1.0 - alpha) * lift
}

/// A node's alpha as a line's end, as a fraction of whole: none where the
/// filter hides it; whole where it is hovered or focused, as its disc is;
/// else the filter's alpha times the falloff from the focus at `centre`,
/// raised a little where the node is near the hover or the focus.
pub fn end_alpha(
    shown: &Shown,
    points: &[Vec3],
    centre: Option<Vec3>,
    node: usize,
    highlight: Highlight,
) -> f32 {
    let filter = filter_alpha(shown.mode, shown.passes.get(node).copied().unwrap_or(true));
    let Some(&at) = points.get(node) else {
        return 0.0;
    };
    if filter <= 0.0 || !is_visible(&shown.visible, node) {
        return 0.0;
    }
    let alpha = focus::node_alpha(filter, centre, at);
    match highlight {
        Highlight::Plain => alpha,
        Highlight::NearHovered => lifted(alpha, NEAR_HOVER_LIFT),
        Highlight::NearFocused => lifted(alpha, NEAR_FOCUS_LIFT),
        Highlight::Hovered | Highlight::Focused => 1.0,
    }
}

/// Every node's alpha as a line's end with this focus and no hover, for
/// the meshes, which change with the focus but not with the hover.
pub fn mesh_alphas(index: &EdgeIndex, shown: &Shown, points: &[Vec3], focus: Focus) -> Vec<f32> {
    let centre = focus.0.and_then(|f| points.get(f).copied());
    let near = focus::near(index, focus, Hovered(None));
    (0..points.len())
        .map(|node| {
            let highlight = focus::highlight(node, focus, Hovered(None), &near);
            end_alpha(shown, points, centre, node, highlight)
        })
        .collect()
}

/// A colour with its alpha scaled.
fn scaled(colour: Color, alpha: f32) -> LinearRgba {
    let linear = colour.to_linear();
    linear.with_alpha(linear.alpha * alpha.clamp(0.0, 1.0))
}

/// The stretch of the time axis node `i` spans, low end first: its span
/// where it has one, else its point's height at both ends.
pub fn node_extent(points: &[Vec3], spans: &[Option<(f32, f32)>], i: usize) -> Option<(f32, f32)> {
    let at = points.get(i)?;
    Some(spans.get(i).copied().flatten().unwrap_or((at.y, at.y)))
}

/// Where a line between nodes `a` and `b` runs (DESIGN.md §5): level, at the
/// middle of the stretch of time their spans share, or where they share
/// none, from the end of the one nearer the other to the end of the other
/// nearer it. Two discs at different heights share none, so their line
/// runs from disc to disc; in recorded time every node is a disc.
pub fn line_ends(
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    a: usize,
    b: usize,
) -> Option<(Vec3, Vec3)> {
    let (from, to) = (*points.get(a)?, *points.get(b)?);
    let (a_lo, a_hi) = node_extent(points, spans, a)?;
    let (b_lo, b_hi) = node_extent(points, spans, b)?;
    let (low, high) = (a_lo.max(b_lo), a_hi.min(b_hi));
    let (ya, yb) = if low <= high {
        let middle = (low + high) / 2.0;
        (middle, middle)
    } else if a_hi < b_lo {
        (a_hi, b_lo)
    } else {
        (a_lo, b_hi)
    };
    Some((from.with_y(ya), to.with_y(yb)))
}

/// The edge toggle's lines: every claim-to-claim edge between two
/// different nodes that are drawn and visible, at the dimmer of the two
/// ends' alphas, where [`line_ends`] puts them.
pub fn claim_segments(
    index: &EdgeIndex,
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    alphas: &[f32],
) -> Vec<Segment> {
    let alpha = |n: usize| alphas.get(n).copied().unwrap_or(0.0);
    index
        .edges
        .iter()
        .filter_map(|edge| {
            let relation = edge.relation()?;
            let (a, b) = (edge.src.drawn()?, edge.dst.drawn()?);
            let shade = alpha(a).min(alpha(b));
            if a == b || shade <= 0.0 {
                return None;
            }
            let (from, to) = line_ends(points, spans, a, b)?;
            Some(Segment {
                from,
                to,
                colour: scaled(relation_colour(relation), shade),
            })
        })
        .collect()
}

/// The lines of one node's claim-to-claim edges, read from its own list of
/// edges and no other: from its disc to each far end that is drawn, where
/// `alpha` says the far end is visible, at the dimmer of the two ends'
/// alphas times `strength`.
pub fn lit_segments(
    index: &EdgeIndex,
    node: usize,
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    alpha: impl Fn(usize) -> f32,
    strength: f32,
) -> Vec<Segment> {
    if points.get(node).is_none() {
        return Vec::new();
    }
    let near = alpha(node);
    index
        .claim
        .get(node)
        .into_iter()
        .flatten()
        .filter_map(|&e| {
            let edge = &index.edges[e];
            let far = edge.far_end(node).drawn().filter(|&far| far != node)?;
            let shade = near.min(alpha(far));
            if shade <= 0.0 {
                return None;
            }
            let (from, to) = line_ends(points, spans, node, far)?;
            Some(Segment {
                from,
                to,
                colour: scaled(relation_colour(edge.relation()?), shade * strength),
            })
        })
        .collect()
}

/// The lines the focus and the hover light, in that order. The hovered
/// node's are left out when it is the focused node, whose lines are drawn
/// already.
pub fn lit_lines(
    index: &EdgeIndex,
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    shown: &Shown,
    focus: Focus,
    hovered: Hovered,
) -> (Vec<Segment>, Vec<Segment>) {
    let centre = focus.0.and_then(|f| points.get(f).copied());
    let near: Near = focus::near(index, focus, hovered);
    let alpha = |n: usize| {
        end_alpha(
            shown,
            points,
            centre,
            n,
            focus::highlight(n, focus, hovered, &near),
        )
    };
    let lines = |node: Option<usize>, strength: f32| {
        node.map(|n| lit_segments(index, n, points, spans, alpha, strength))
            .unwrap_or_default()
    };
    let hover = hovered.0.filter(|&h| focus.0 != Some(h));
    (
        lines(focus.0, FOCUS_LINE_ALPHA),
        lines(hover, HOVER_LINE_ALPHA),
    )
}

/// Where one end of a connector is, as the filter shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// A drawn node the filter leaves visible.
    Shown(Vec3),
    /// A drawn node the filter hides.
    Hidden(Vec3),
    /// Not a drawn node.
    Absent,
}

/// Which end of a connector its alpha follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follows {
    Old,
    New,
}

/// The pieces of the connector from an old node to a new one, and the end
/// its alpha follows; `None` where neither end is visible.
pub fn connector(old: Place, new: Place) -> Option<(Vec<[Vec3; 2]>, Follows)> {
    match (old, new) {
        (Place::Shown(old), Place::Shown(new)) => Some((elbow(old, new), Follows::New)),
        (Place::Absent, Place::Shown(new)) => Some((absent_stub(new, -1.0), Follows::New)),
        (Place::Hidden(old), Place::Shown(new)) => Some((hidden_stub(new, old), Follows::New)),
        (Place::Shown(old), Place::Absent) => Some((absent_stub(old, 1.0), Follows::Old)),
        (Place::Shown(old), Place::Hidden(new)) => Some((hidden_stub(old, new), Follows::Old)),
        _ => None,
    }
}

/// From the new node along its own axis to the old node's height, then
/// across to the old node. A piece of no length is left out.
pub fn elbow(old: Vec3, new: Vec3) -> Vec<[Vec3; 2]> {
    let corner = Vec3::new(new.x, old.y, new.z);
    [[new, corner], [corner, old]]
        .into_iter()
        .filter(|[a, b]| a.distance_squared(*b) > f32::EPSILON)
        .collect()
}

/// A stub of [`STUB_LENGTH`] from a node, down for `direction` -1 and up
/// for 1, and a cross at its far end, flat on the map.
pub fn absent_stub(at: Vec3, direction: f32) -> Vec<[Vec3; 2]> {
    let foot = at + Vec3::Y * direction.signum() * STUB_LENGTH;
    vec![
        [at, foot],
        [foot - Vec3::X * MARKER_SIZE, foot + Vec3::X * MARKER_SIZE],
        [foot - Vec3::Z * MARKER_SIZE, foot + Vec3::Z * MARKER_SIZE],
    ]
}

/// A stub from a node toward a hidden one: [`STUB_LENGTH`] long, or half
/// the way where the hidden node is nearer than twice that, and down where
/// the two stand at one place.
pub fn hidden_stub(from: Vec3, toward: Vec3) -> Vec<[Vec3; 2]> {
    let offset = toward - from;
    let distance = offset.length();
    let (direction, length) = if distance > f32::EPSILON {
        (offset / distance, STUB_LENGTH.min(distance / 2.0))
    } else {
        (Vec3::NEG_Y, STUB_LENGTH)
    };
    vec![[from, from + direction * length]]
}

/// Every connector's pieces, coloured by the kind of succession, at the
/// alpha of the end each follows. A connector leaves the old node at the
/// top of its span and reaches the new node at the foot of its span, so
/// that in claim time it runs from where the old column ends to where the
/// new one begins; a disc's top and foot are its place.
pub fn connector_segments(
    index: &EdgeIndex,
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    alphas: &[f32],
) -> Vec<Segment> {
    let place = |end: &End, top: bool| {
        let found = end.drawn().and_then(|n| {
            let at = *points.get(n)?;
            let (lo, hi) = node_extent(points, spans, n)?;
            Some((n, at.with_y(if top { hi } else { lo })))
        });
        match found {
            None => Place::Absent,
            Some((n, at)) if alphas.get(n).copied().unwrap_or(0.0) > 0.0 => Place::Shown(at),
            Some((_, at)) => Place::Hidden(at),
        }
    };
    let mut segments = Vec::new();
    for edge in &index.edges {
        let Some(succession) = edge.succession() else {
            continue;
        };
        let Some((pieces, follows)) = connector(place(&edge.src, true), place(&edge.dst, false))
        else {
            continue;
        };
        let end = match follows {
            Follows::Old => &edge.src,
            Follows::New => &edge.dst,
        };
        let shade = end
            .drawn()
            .and_then(|n| alphas.get(n).copied())
            .unwrap_or(0.0);
        let colour = scaled(succession_colour(succession), shade);
        segments.extend(
            pieces
                .into_iter()
                .map(|[from, to]| Segment { from, to, colour }),
        );
    }
    segments
}

/// A mesh of hardware lines, a pair of vertices for each segment, coloured
/// per vertex. A mesh with nothing to draw holds one line of no length and
/// no colour, so that the renderer is never handed an empty buffer.
pub fn line_mesh(segments: &[Segment]) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(segments.len() * 2);
    let mut colours: Vec<[f32; 4]> = Vec::with_capacity(segments.len() * 2);
    for segment in segments {
        positions.extend([segment.from.to_array(), segment.to.to_array()]);
        colours.extend([segment.colour.to_f32_array(); 2]);
    }
    if positions.is_empty() {
        positions = vec![[0.0; 3]; 2];
        colours = vec![[0.0; 4]; 2];
    }
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
}

/// The lines' material: unlit, coloured by the vertices, and additive like
/// the discs, so that crossing lines brighten rather than hide each other.
fn line_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        alpha_mode: AlphaMode::Add,
        fog_enabled: false,
        ..default()
    }
}

/// Spawns the two line meshes, empty and hidden until the first rebuild.
/// They span the whole graph, so they are never culled.
fn spawn_line_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(line_material());
    let claim = meshes.add(line_mesh(&[]));
    let connectors = meshes.add(line_mesh(&[]));
    commands.spawn((
        ClaimLines,
        Mesh3d(claim.clone()),
        MeshMaterial3d(material.clone()),
        Transform::IDENTITY,
        Visibility::Hidden,
        NoFrustumCulling,
    ));
    commands.spawn((
        Connectors,
        Mesh3d(connectors.clone()),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        Visibility::Hidden,
        NoFrustumCulling,
    ));
    commands.insert_resource(EdgeMeshes { claim, connectors });
}

/// What the line meshes are built from.
#[derive(bevy::ecs::system::SystemParam)]
struct LineSource<'w> {
    index: Res<'w, GraphIndex>,
    places: Res<'w, Places>,
    shown: Res<'w, Shown>,
    focus: Res<'w, Focus>,
    handles: Option<Res<'w, EdgeMeshes>>,
    meshes: ResMut<'w, Assets<Mesh>>,
}

impl LineSource<'_> {
    /// Whether the graph, the filter or the focus changed in this update.
    fn changed(&self) -> bool {
        self.index.is_changed()
            || self.places.is_changed()
            || self.shown.is_changed()
            || self.focus.is_changed()
    }

    fn alphas(&self) -> Vec<f32> {
        mesh_alphas(
            &self.index.edges,
            &self.shown,
            &self.places.points,
            *self.focus,
        )
    }

    /// Rewrites one of the meshes.
    fn write(&mut self, which: impl Fn(&EdgeMeshes) -> &Handle<Mesh>, segments: &[Segment]) {
        let Some(handle) = self.handles.as_deref().map(|h| which(h).clone()) else {
            return;
        };
        if let Some(mut mesh) = self.meshes.get_mut(&handle) {
            *mesh = line_mesh(segments);
        }
    }
}

fn visibility(drawn: bool) -> Visibility {
    if drawn {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

/// Rebuilds the edge toggle's mesh in an update in which the toggle, the
/// graph, the filter or the focus changed, while the toggle is on, and
/// hides it while the toggle is off.
fn rebuild_claim_lines(
    all: Res<AllEdges>,
    mut source: LineSource,
    mut lines: Query<&mut Visibility, With<ClaimLines>>,
) {
    if !(all.is_changed() || source.changed()) {
        return;
    }
    let segments = if all.0 {
        claim_segments(
            &source.index.edges,
            &source.places.points,
            &source.places.spans,
            &source.alphas(),
        )
    } else {
        Vec::new()
    };
    if all.0 {
        if all.is_changed() || !source.focus.is_changed() {
            info!(
                "edge toggle on: {} claim-to-claim lines between visible nodes",
                segments.len()
            );
        }
        source.write(|h| &h.claim, &segments);
    } else if all.is_changed() && !all.is_added() {
        info!("edge toggle off");
    }
    for mut shown in &mut lines {
        shown.set_if_neq(visibility(!segments.is_empty()));
    }
}

/// Rebuilds the connectors' mesh in an update in which the graph, the
/// filter or the focus changed.
fn rebuild_connectors(mut source: LineSource, mut lines: Query<&mut Visibility, With<Connectors>>) {
    if !source.changed() {
        return;
    }
    let alphas = source.alphas();
    let segments = connector_segments(
        &source.index.edges,
        &source.places.points,
        &source.places.spans,
        &alphas,
    );
    if !source.focus.is_changed() || source.shown.is_changed() {
        let drawn = |end: &End| end.drawn().is_some_and(|n| alphas.get(n) > Some(&0.0));
        let visible = source
            .index
            .edges
            .edges
            .iter()
            .filter(|e| e.succession().is_some() && (drawn(&e.src) || drawn(&e.dst)))
            .count();
        info!(
            "connectors: {visible} successions and merges with a visible end, in {} pieces of line",
            segments.len()
        );
    }
    source.write(|h| &h.connectors, &segments);
    for mut shown in &mut lines {
        shown.set_if_neq(visibility(!segments.is_empty()));
    }
}

/// Draws the lines the focus and the hover light, in every update.
fn draw_lit_lines(
    index: Res<GraphIndex>,
    places: Res<Places>,
    shown: Res<Shown>,
    focus: Res<Focus>,
    hovered: Res<Hovered>,
    mut focused_lines: Gizmos<FocusLines>,
    mut hovered_lines: Gizmos<HoverLines>,
) {
    if focus.0.is_none() && hovered.0.is_none() {
        return;
    }
    let (focused, hover) = lit_lines(
        &index.edges,
        &places.points,
        &places.spans,
        &shown,
        *focus,
        *hovered,
    );
    for line in focused {
        focused_lines.line(line.from, line.to, line.colour);
    }
    for line in hover {
        hovered_lines.line(line.from, line.to, line.colour);
    }
}

/// `L` turns the edge toggle on and off, unless a text field has the
/// keyboard.
fn toggle_on_key(keys: Res<ButtonInput<KeyCode>>, ui: Res<UiFocus>, mut all: ResMut<AllEdges>) {
    if !ui.typing && keys.just_pressed(EDGE_KEY.0) {
        all.0 = !all.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_index::build;
    use crate::filter::{DIM_ALPHA, Mode};
    use crate::graph_index::tests::edge;
    use std::collections::HashMap;

    /// Drawn nodes f, i, g, t at these places; i is an inference that f
    /// and g support, f and g are assessed, and old-f, which is not drawn,
    /// was superseded by f.
    fn graph() -> (EdgeIndex, Vec<Vec3>) {
        let by_id: HashMap<String, usize> = ["f", "i", "g", "t"]
            .iter()
            .enumerate()
            .map(|(i, &n)| (n.to_owned(), i))
            .collect();
        let edges = vec![
            edge("f", "i", "supports"),
            edge("g", "i", "supports"),
            edge("f", "g", "assessed"),
            edge("f", "t", "tagged_with_topic"),
            edge("old-f", "f", "superseded_by"),
            edge("f", "retired", "supports"),
            edge("g", "g", "related"),
        ];
        let points = vec![
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(20.0, 30.0, 0.0),
            Vec3::new(0.0, 50.0, 20.0),
            Vec3::new(-20.0, 0.0, 0.0),
        ];
        (build(&edges, &by_id, 4), points)
    }

    fn everything(count: usize) -> Shown {
        Shown {
            mode: Mode::Dim,
            passes: vec![true; count],
            visible: vec![true; count],
        }
    }

    fn ends(segments: &[Segment]) -> Vec<(Vec3, Vec3)> {
        segments.iter().map(|s| (s.from, s.to)).collect()
    }

    #[test]
    fn the_toggles_lines_join_the_two_nodes_places_at_the_dimmer_alpha() {
        let (index, points) = graph();
        let alphas = [1.0, DIM_ALPHA, 1.0, 1.0];
        let segments = claim_segments(&index, &points, &[], &alphas);
        assert_eq!(
            ends(&segments),
            [
                (points[0], points[1]),
                (points[2], points[1]),
                (points[0], points[2])
            ],
            "membership, absent ends and loops draw nothing"
        );
        let supports = relation_colour(crate::edge_index::Relation::Supports).to_linear();
        assert!((segments[0].colour.alpha - supports.alpha * DIM_ALPHA).abs() < 1e-6);
        assert!(
            (segments[2].colour.alpha - 0.75).abs() < 1e-6,
            "assessed, whole"
        );
        let hidden = claim_segments(&index, &points, &[], &[1.0, 0.0, 1.0, 1.0]);
        assert_eq!(
            ends(&hidden),
            [(points[0], points[2])],
            "a hidden end draws nothing"
        );
    }

    #[test]
    fn hover_lines_come_from_the_hovered_nodes_own_list() {
        let (mut index, points) = graph();
        let shown = everything(4);
        let (focused, hovered) =
            lit_lines(&index, &points, &[], &shown, Focus(None), Hovered(Some(1)));
        assert!(focused.is_empty());
        assert_eq!(
            ends(&hovered),
            [(points[1], points[0]), (points[1], points[2])]
        );
        for line in &hovered {
            assert!(
                (line.colour.alpha
                    - relation_colour(crate::edge_index::Relation::Supports)
                        .to_linear()
                        .alpha
                        * HOVER_LINE_ALPHA
                        * lifted(1.0, NEAR_HOVER_LIFT))
                .abs()
                    < 1e-6
            );
        }
        // An edge left off the node's own list is not drawn, though it is
        // still in the index: nothing else is read.
        index.claim[1].retain(|&e| index.edges[e].src != End::Drawn(2));
        let (_, hovered) = lit_lines(&index, &points, &[], &shown, Focus(None), Hovered(Some(1)));
        assert_eq!(ends(&hovered), [(points[1], points[0])]);
    }

    #[test]
    fn the_focus_lines_outshine_the_hovers_and_a_hidden_end_draws_none() {
        let (index, points) = graph();
        let mut shown = everything(4);
        let (focused, hovered) = lit_lines(
            &index,
            &points,
            &[],
            &shown,
            Focus(Some(0)),
            Hovered(Some(2)),
        );
        assert_eq!(
            ends(&focused),
            [(points[0], points[1]), (points[0], points[2])],
            "the supports and the assessed edge, not the absent end"
        );
        assert_eq!(
            ends(&hovered),
            [(points[2], points[1]), (points[2], points[0])]
        );
        let strongest =
            |lines: &[Segment]| lines.iter().map(|l| l.colour.alpha).fold(0.0, f32::max);
        assert!(strongest(&focused) > strongest(&hovered));
        let (_, same) = lit_lines(
            &index,
            &points,
            &[],
            &shown,
            Focus(Some(0)),
            Hovered(Some(0)),
        );
        assert!(same.is_empty(), "the focus's lines are drawn once");

        shown.mode = Mode::Hide;
        shown.passes[1] = false;
        shown.visible[1] = false;
        let (focused, _) = lit_lines(&index, &points, &[], &shown, Focus(Some(0)), Hovered(None));
        assert_eq!(ends(&focused), [(points[0], points[2])]);
    }

    #[test]
    fn a_connector_between_two_drawn_nodes_runs_up_the_new_nodes_axis() {
        let old = Vec3::new(5.0, 10.0, -3.0);
        let new = Vec3::new(-2.0, 40.0, 7.0);
        let (pieces, follows) = connector(Place::Shown(old), Place::Shown(new)).unwrap();
        assert_eq!(follows, Follows::New);
        let corner = Vec3::new(-2.0, 10.0, 7.0);
        assert_eq!(pieces, [[new, corner], [corner, old]]);
        assert_eq!(
            elbow(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 9.0, 0.0)),
            [[Vec3::new(0.0, 9.0, 0.0), Vec3::new(0.0, 1.0, 0.0)]],
            "one above the other: a single vertical"
        );
    }

    #[test]
    fn a_stub_marks_an_absent_end_and_points_toward_a_hidden_one() {
        let new = Vec3::new(3.0, 20.0, 4.0);
        let (pieces, follows) = connector(Place::Absent, Place::Shown(new)).unwrap();
        assert_eq!(follows, Follows::New);
        let foot = new - Vec3::Y * STUB_LENGTH;
        assert_eq!(pieces[0], [new, foot], "straight down");
        assert_eq!(pieces.len(), 3, "and a cross at the foot");
        for [a, b] in &pieces[1..] {
            assert!(((*a + *b) / 2.0).distance(foot) < 1e-5);
            assert_eq!(a.y, foot.y, "flat on the map");
            assert!((a.distance(*b) - 2.0 * MARKER_SIZE).abs() < 1e-6);
        }
        let old = Vec3::new(0.0, 5.0, 0.0);
        let (up, follows) = connector(Place::Shown(old), Place::Absent).unwrap();
        assert_eq!(follows, Follows::Old);
        assert_eq!(
            up[0],
            [old, old + Vec3::Y * STUB_LENGTH],
            "a missing new end"
        );

        let hidden = Vec3::new(3.0, 20.0, 104.0);
        let (toward, follows) = connector(Place::Hidden(hidden), Place::Shown(new)).unwrap();
        assert_eq!(follows, Follows::New);
        assert_eq!(toward, [[new, new + Vec3::Z * STUB_LENGTH]]);
        let near = Vec3::new(3.0, 22.0, 4.0);
        assert_eq!(
            hidden_stub(new, near),
            [[new, new + Vec3::Y]],
            "half the way to a near one"
        );
        assert_eq!(hidden_stub(new, new), [[new, new - Vec3::Y * STUB_LENGTH]]);
        assert_eq!(connector(Place::Hidden(old), Place::Hidden(new)), None);
        assert_eq!(connector(Place::Absent, Place::Hidden(new)), None);
        assert_eq!(connector(Place::Absent, Place::Absent), None);
    }

    #[test]
    fn connectors_take_the_succession_colour_and_the_new_nodes_alpha() {
        let (index, points) = graph();
        let segments = connector_segments(&index, &points, &[], &[0.5, 1.0, 1.0, 1.0]);
        assert_eq!(segments.len(), 3, "a stub and its cross");
        assert_eq!(
            (segments[0].from, segments[0].to),
            (points[0], points[0] - Vec3::Y * STUB_LENGTH)
        );
        let colour = succession_colour(crate::edge_index::Succession::Superseded).to_linear();
        assert!((segments[0].colour.alpha - colour.alpha * 0.5).abs() < 1e-6);
        assert!(connector_segments(&index, &points, &[], &[0.0, 1.0, 1.0, 1.0]).is_empty());
    }

    #[test]
    fn in_claim_time_a_line_runs_level_through_the_overlap_or_between_the_nearer_ends() {
        let points = [
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(10.0, 70.0, 0.0),
            Vec3::new(20.0, 200.0, 5.0),
            Vec3::new(30.0, 40.0, 0.0),
        ];
        // Two columns sharing 60 to 80, a column far above both, and a disc
        // inside the first column.
        let spans = [
            Some((20.0, 80.0)),
            Some((60.0, 100.0)),
            Some((150.0, 250.0)),
            Some((40.0, 40.0)),
        ];
        let (a, b) = line_ends(&points, &spans, 0, 1).unwrap();
        assert_eq!(
            (a, b),
            (Vec3::new(0.0, 70.0, 0.0), Vec3::new(10.0, 70.0, 0.0)),
            "level at 70"
        );
        let (a, b) = line_ends(&points, &spans, 0, 2).unwrap();
        assert_eq!(
            (a.y, b.y),
            (80.0, 150.0),
            "the top of the lower to the foot of the upper"
        );
        let (a, b) = line_ends(&points, &spans, 2, 0).unwrap();
        assert_eq!((a.y, b.y), (150.0, 80.0), "either way round");
        let (a, b) = line_ends(&points, &spans, 3, 0).unwrap();
        assert_eq!(
            (a.y, b.y),
            (40.0, 40.0),
            "a disc inside a column: level at the disc"
        );
        // With no spans every node is a disc at its point, as in recorded time.
        assert_eq!(line_ends(&points, &[], 0, 1), Some((points[0], points[1])));
        assert_eq!(line_ends(&points, &spans, 0, 9), None);
    }

    #[test]
    fn in_claim_time_a_connector_runs_from_the_old_columns_top_to_the_new_ones_foot() {
        let by_id: HashMap<String, usize> = [("old", 0), ("new", 1), ("lone", 2)]
            .into_iter()
            .map(|(n, i)| (n.to_owned(), i))
            .collect();
        let index = build(
            &[
                edge("old", "new", "temporally_followed_by"),
                edge("gone", "lone", "superseded_by"),
            ],
            &by_id,
            3,
        );
        let points = [
            Vec3::new(0.0, 30.0, 0.0),
            Vec3::new(5.0, 90.0, 0.0),
            Vec3::new(9.0, 50.0, 0.0),
        ];
        let spans = [Some((10.0, 50.0)), Some((60.0, 120.0)), Some((40.0, 60.0))];
        let segments = connector_segments(&index, &points, &spans, &[1.0; 3]);
        let corner = Vec3::new(5.0, 50.0, 0.0);
        assert_eq!(
            (segments[0].from, segments[0].to),
            (Vec3::new(5.0, 60.0, 0.0), corner),
            "down the new column's axis from its foot"
        );
        assert_eq!(
            (segments[1].from, segments[1].to),
            (corner, Vec3::new(0.0, 50.0, 0.0))
        );
        assert_eq!(
            (segments[2].from, segments[2].to),
            (
                Vec3::new(9.0, 40.0, 0.0),
                Vec3::new(9.0, 40.0 - STUB_LENGTH, 0.0)
            ),
            "a stub from the foot of the column"
        );
    }

    #[test]
    fn a_mesh_holds_two_vertices_a_segment_and_never_none() {
        let segment = Segment {
            from: Vec3::ZERO,
            to: Vec3::ONE,
            colour: LinearRgba::RED,
        };
        let mesh = line_mesh(&[segment, segment]);
        assert_eq!(mesh.primitive_topology(), PrimitiveTopology::LineList);
        assert_eq!(mesh.count_vertices(), 4);
        assert_eq!(line_mesh(&[]).count_vertices(), 2);
    }

    /// The line meshes' systems over the four nodes of [`graph`], with no
    /// renderer.
    fn mesh_app() -> App {
        let (edges, points) = graph();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .insert_resource(GraphIndex {
                edges,
                ..GraphIndex::default()
            })
            .insert_resource(Places {
                points,
                ..Places::default()
            })
            .insert_resource(everything(4))
            .init_resource::<Focus>()
            .init_resource::<AllEdges>()
            .init_resource::<UiFocus>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Startup, spawn_line_meshes)
            .add_systems(Update, toggle_on_key)
            .add_systems(PostUpdate, (rebuild_claim_lines, rebuild_connectors));
        app
    }

    fn vertices(app: &App, which: impl Fn(&EdgeMeshes) -> &Handle<Mesh>) -> usize {
        let handles = app.world().resource::<EdgeMeshes>();
        let meshes = app.world().resource::<Assets<Mesh>>();
        meshes.get(which(handles)).unwrap().count_vertices()
    }

    fn shown<C: Component>(app: &mut App) -> bool {
        let mut query = app.world_mut().query_filtered::<&Visibility, With<C>>();
        query.iter(app.world()).all(|v| *v != Visibility::Hidden)
    }

    fn mark(app: &mut App, which: impl Fn(&EdgeMeshes) -> &Handle<Mesh>) {
        let handle = which(app.world().resource::<EdgeMeshes>()).clone();
        let marker = Segment {
            from: Vec3::ZERO,
            to: Vec3::X,
            colour: LinearRgba::BLUE,
        };
        let sentinel = line_mesh(&[marker; 7]);
        *app.world_mut()
            .resource_mut::<Assets<Mesh>>()
            .get_mut(&handle)
            .unwrap() = sentinel;
    }

    fn press(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        app.update();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(key);
        keys.clear();
    }

    #[test]
    fn the_toggle_mesh_is_rebuilt_only_when_something_it_shows_changed() {
        let mut app = mesh_app();
        app.update();
        assert!(!shown::<ClaimLines>(&mut app), "off at the start");
        assert!(
            shown::<Connectors>(&mut app),
            "the connectors are always on"
        );
        assert_eq!(vertices(&app, |h| &h.connectors), 6, "a stub and a cross");

        press(&mut app, EDGE_KEY.0);
        assert!(app.world().resource::<AllEdges>().0);
        assert!(shown::<ClaimLines>(&mut app));
        assert_eq!(vertices(&app, |h| &h.claim), 6, "three lines");

        // Nothing rewrites either mesh while nothing they show changes.
        mark(&mut app, |h| &h.claim);
        mark(&mut app, |h| &h.connectors);
        app.update();
        app.update();
        assert_eq!(vertices(&app, |h| &h.claim), 14);
        assert_eq!(vertices(&app, |h| &h.connectors), 14);

        // A change of filter rebuilds both.
        app.world_mut().resource_mut::<Shown>().mode = Mode::Hide;
        app.update();
        assert_eq!(vertices(&app, |h| &h.claim), 6);
        assert_eq!(vertices(&app, |h| &h.connectors), 6);

        // So does a focus, which fades the lines with distance.
        mark(&mut app, |h| &h.claim);
        app.insert_resource(Focus(Some(3)));
        app.update();
        assert_eq!(vertices(&app, |h| &h.claim), 6);

        // Off, the mesh is hidden and not rebuilt, even by a change of
        // filter; on again, it is rebuilt at once.
        press(&mut app, EDGE_KEY.0);
        assert!(!shown::<ClaimLines>(&mut app));
        mark(&mut app, |h| &h.claim);
        app.world_mut().resource_mut::<Shown>().mode = Mode::Dim;
        app.update();
        assert_eq!(vertices(&app, |h| &h.claim), 14);
        press(&mut app, EDGE_KEY.0);
        assert_eq!(vertices(&app, |h| &h.claim), 6);

        // Not while a text field has the keyboard.
        app.world_mut().resource_mut::<UiFocus>().typing = true;
        press(&mut app, EDGE_KEY.0);
        assert!(app.world().resource::<AllEdges>().0);
    }

    #[test]
    fn a_line_end_is_whole_when_lit_and_lifted_when_near() {
        let points = [Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0)];
        let shown = everything(2);
        let far = Some(points[0]);
        let plain = end_alpha(&shown, &points, far, 1, Highlight::Plain);
        assert!((plain - focus::FAR_ALPHA).abs() < 1e-6);
        let near = end_alpha(&shown, &points, far, 1, Highlight::NearFocused);
        assert!(near > plain && near < 1.0);
        assert!(end_alpha(&shown, &points, far, 1, Highlight::NearHovered) < near);
        assert_eq!(end_alpha(&shown, &points, far, 1, Highlight::Hovered), 1.0);
        let hidden = Shown {
            mode: Mode::Hide,
            passes: vec![true, false],
            visible: vec![true, false],
        };
        assert_eq!(end_alpha(&hidden, &points, far, 1, Highlight::Focused), 0.0);
        assert_eq!(end_alpha(&shown, &points, far, 7, Highlight::Plain), 0.0);
    }
}
