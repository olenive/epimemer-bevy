//! The camera presets of DESIGN.md §6, as pure functions of where the nodes
//! are.
//!
//! A preset is a [`View`]: a point to look at, a direction, how much of the
//! scene the view spans and a projection. Every preset is fitted to the
//! positions of the nodes it frames and to nothing else, so each works on
//! the topic map of DESIGN.md §7 as it did on the topic circle before it,
//! and follows the map when it moves.
//!
//! - **Graph**: straight down, orthographic, every node on the map.
//! - **Timeline**: side-on, looking along the map's second axis (Bevy's z),
//!   orthographic, every node against the time axis.
//! - **3D**: perspective, the whole graph from above and to one side.
//! - **Now**: the 3D preset's direction, close enough that the latest day's
//!   layer of recorded nodes fills the view; in claim time, the nodes whose
//!   disc or column comes within a day of "now" ([`NowLayer`]).
//! - **A region**, a topic with the claims placed beside it or the nodes of
//!   one metacontext: those nodes only, from a direction and in a projection
//!   the caller chooses, which is the camera's own.
//!
//! Every fit frames the visible nodes only, so a node the filter hides
//! leaves the framing with its disc, and holds both ends of every column. A fit is for a [`Screen`]: the
//! viewport's shape and the band at its top that the bar covers, which the
//! fit leaves clear, so that what it frames is never under the bar. When none of the nodes a fit is for is
//! visible, it frames them all rather than collapse onto the origin.
//!
//! A change of view is a tween ([`between`]): the projection switches at the
//! start, and the point looked at, the direction and the span move together
//! along an ease-in-out curve. A view stands as far from its focus as a
//! perspective camera must to span the view's height there, so switching
//! between the projections keeps the focus and everything level with it the
//! same size on the screen.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use bevy::math::{EulerRot, Quat, Vec3};
use bevy::transform::components::Transform;

use crate::filter::is_visible;
use crate::hub::{EdgeView, MetacontextView};
use crate::layout::{Kind, Placed, TimeAxis};

/// The perspective camera's vertical field of view, Bevy's default.
pub const FOV: f32 = FRAC_PI_4;
/// Room left around the nodes in a fit, as a fraction of the view.
pub const FIT_MARGIN: f32 = 1.08;
/// Room around a node's centre for its disc, so that a node at the edge of
/// a fit is drawn whole. The largest disc, a topic's, has radius 1.8, and
/// every disc is 1 high.
pub const NODE_PAD: f32 = 2.0;
/// The smallest height a fit gives, in world units, so that a topic with
/// one claim does not fill the screen with two discs.
pub const MIN_HEIGHT: f32 = 16.0;
/// The 3D preset's direction: the angle around the vertical axis, and the
/// angle below the horizon the camera looks down at. Low, so that time
/// reads as height. The angle around was chosen on the topic circle, from
/// the side where its oldest topics stood nearest, and is kept on the map
/// so that the preset looks the way it always has.
pub const FREE_YAW: f32 = -0.9;
pub const FREE_PITCH: f32 = -0.2;
/// The graph preset looks straight down with the map's x axis across the
/// screen, and the timeline preset looks along -z with the same x axis
/// across, so the two read like the plan and the elevation of a drawing.
pub const GRAPH_YAW: f32 = 0.0;
pub const GRAPH_PITCH: f32 = -FRAC_PI_2;
pub const TIMELINE_YAW: f32 = 0.0;
pub const TIMELINE_PITCH: f32 = 0.0;
/// How long a tween from one view to another takes, in seconds.
pub const TWEEN_SECONDS: f32 = 0.5;
/// One day, in the time axis's microseconds.
pub const DAY_MICROS: i64 = 86_400_000_000;
/// A perspective camera starting a tween is taken to look at a point at
/// least this far in front of it.
const MIN_DEPTH: f32 = 1.0;

/// How the view projects the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lens {
    Perspective,
    Orthographic,
}

/// Where a camera is and what it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The point the camera looks at, in the middle of the view.
    pub focus: Vec3,
    /// The angle around the vertical axis, and the angle above the horizon,
    /// in radians; Bevy's YXZ Euler angles with no roll.
    pub yaw: f32,
    pub pitch: f32,
    /// How much of the scene the view spans from bottom to top, in world
    /// units, measured at the focus.
    pub height: f32,
    pub lens: Lens,
}

/// The presets that frame the whole graph, or its latest layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Graph,
    Timeline,
    Free,
    Now,
}

/// Each preset's name on the command line.
pub const PRESET_NAMES: [(&str, Preset); 4] = [
    ("graph", Preset::Graph),
    ("timeline", Preset::Timeline),
    ("3d", Preset::Free),
    ("now", Preset::Now),
];

/// The preset with this name, ignoring case.
pub fn parse_preset(name: &str) -> Option<Preset> {
    PRESET_NAMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, preset)| preset)
}

/// A set of nodes a view can be fitted to: a topic with the claims placed
/// beside it, or the nodes of one metacontext. `members` index the nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub id: String,
    pub name: String,
    pub members: Vec<usize>,
}

/// The viewport a view is fitted to: its aspect ratio, width over height,
/// the fraction of its height, from the top, that the bar covers, and the
/// fraction of its width, from the right, that the side panel covers. A
/// bare aspect ratio is a screen with nothing over it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub aspect: f32,
    pub inset: f32,
    pub right: f32,
}

impl From<f32> for Screen {
    fn from(aspect: f32) -> Self {
        Screen {
            aspect,
            inset: 0.0,
            right: 0.0,
        }
    }
}

/// The most of the viewport a fit leaves clear at the top, so that a bar
/// taller than half a small window still leaves half for the scene, and the
/// most it leaves clear at the right, for the side panel.
pub const MAX_INSET: f32 = 0.5;
pub const MAX_RIGHT: f32 = 0.5;

/// A covered fraction of the screen, kept between none and `most`.
fn covered(fraction: f32, most: f32) -> f32 {
    if fraction.is_finite() {
        fraction.clamp(0.0, most)
    } else {
        0.0
    }
}

/// The rotation of a camera facing this way.
pub fn rotation(yaw: f32, pitch: f32) -> Quat {
    Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
}

/// How far from its focus a perspective camera spans this height there.
pub fn distance(height: f32) -> f32 {
    height / (2.0 * (FOV / 2.0).tan())
}

/// Where the camera stands for a view.
pub fn transform(view: &View) -> Transform {
    let rotation = rotation(view.yaw, view.pitch);
    Transform {
        translation: view.focus + rotation * Vec3::Z * distance(view.height),
        rotation,
        ..Transform::IDENTITY
    }
}

/// The smallest box holding every point, or `None` for no points.
pub fn bounds(points: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let first = *points.first()?;
    Some(
        points
            .iter()
            .fold((first, first), |(lo, hi), &p| (lo.min(p), hi.max(p))),
    )
}

/// The view that frames every point from this direction, with this
/// projection, for this screen: an aspect ratio, or a [`Screen`] whose top
/// band is covered.
///
/// The focus is the centre of the points' box measured in the camera's own
/// axes, which for an axis-aligned direction is the centre of their box in
/// the world. An orthographic view spans the box across and up; a
/// perspective one stands as close as it can while every point lands
/// inside. Each point is padded by [`NODE_PAD`], the fit leaves
/// [`FIT_MARGIN`] around it, and no fit is smaller than [`MIN_HEIGHT`]. No
/// points frames the origin.
///
/// With a covered band, the points are framed in the part of the screen
/// below it: the view spans more, and its focus rises by half the band, so
/// that the box's centre sits in the middle of the part left clear. A
/// covered strip at the right, the side panel's, is left clear the same
/// way across: the view spans more, and its focus moves right by half the
/// strip.
pub fn fit(points: &[Vec3], yaw: f32, pitch: f32, lens: Lens, screen: impl Into<Screen>) -> View {
    let Screen {
        aspect,
        inset,
        right,
    } = screen.into();
    let rotation = rotation(yaw, pitch);
    let inverse = rotation.inverse();
    let local: Vec<Vec3> = points.iter().map(|&p| inverse * p).collect();
    let (min, max) = bounds(&local).unwrap_or((Vec3::ZERO, Vec3::ZERO));
    let centre = (min + max) / 2.0;
    let aspect = if aspect > 0.0 { aspect } else { 1.0 };
    let inset = covered(inset, MAX_INSET);
    let right = covered(right, MAX_RIGHT);
    let clear = 1.0 - inset;
    let clear_across = 1.0 - right;
    let tan = (FOV / 2.0).tan();
    let height = match lens {
        Lens::Orthographic => {
            let across = max.x - min.x + 2.0 * NODE_PAD;
            let up = max.y - min.y + 2.0 * NODE_PAD;
            (up / clear).max(across / (aspect * clear_across)) * FIT_MARGIN
        }
        Lens::Perspective => {
            // In the camera's frame the camera is at (cx + r, cy + s,
            // cz + d), looking along -z, where s lifts it so that the box's
            // centre lands in the middle of the clear part of the screen,
            // `shift` below the screen's middle as a tangent, and r moves it
            // right so that the centre lands `shift_across` left of the
            // middle. A point at (x, y, z) is cz + d - z in front of it and
            // must have its height above the clear part's middle within that
            // depth times the clear part's half-height, and its distance
            // across from that middle within that depth times the clear
            // part's half-width. With s = shift * d, that height works out
            // to y - cy - shift * (z - cz), whatever d is, and likewise
            // across with r = shift_across * d.
            let tan_up = tan * clear / FIT_MARGIN;
            let tan_across = tan * aspect * clear_across / FIT_MARGIN;
            let shift = tan * inset;
            let shift_across = tan * aspect * right;
            let d = local
                .iter()
                .map(|&p| {
                    let side = p.x - centre.x - shift_across * (p.z - centre.z);
                    let across = (side.abs() + NODE_PAD) / tan_across;
                    let rise = p.y - centre.y - shift * (p.z - centre.z);
                    let up = (rise.abs() + NODE_PAD) / tan_up;
                    across.max(up) + (p.z - centre.z)
                })
                .fold(0.0_f32, f32::max);
            2.0 * d * tan
        }
    }
    .max(MIN_HEIGHT);
    // Half the band, in both projections: for a perspective view the lift
    // s = shift * d is tan * inset * height / (2 * tan). Across, half the
    // strip of a view `height * aspect` wide.
    let lift = height * inset / 2.0;
    let aside = height * aspect * right / 2.0;
    View {
        focus: rotation * (centre + Vec3::Y * lift + Vec3::X * aside),
        yaw,
        pitch,
        height,
        lens,
    }
}

/// The height of one day on the time axis. With a single instant every
/// node is on one layer, and the layer is everything.
pub fn day_height(axis: &TimeAxis) -> f32 {
    if axis.span <= 0 {
        return f32::INFINITY;
    }
    (DAY_MICROS as f64 / axis.span as f64 * axis.height as f64) as f32
}

/// The latest layer: the points recorded within one day of the latest.
pub fn latest_layer(points: &[Vec3], day_height: f32) -> Vec<Vec3> {
    let top = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    points
        .iter()
        .copied()
        .filter(|p| p.y >= top - day_height)
        .collect()
}

/// Which layer the "now" preset frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NowLayer {
    /// The latest day's layer, one day high: the nodes within that height
    /// of the highest. Recorded time.
    Latest(f32),
    /// The layer at "now", `height` on the axis: the nodes whose disc or
    /// column comes within `day` of it. Claim time.
    At { height: f32, day: f32 },
}

impl Default for NowLayer {
    /// With no axis to measure a day on, every node is in the latest layer.
    fn default() -> Self {
        NowLayer::Latest(f32::INFINITY)
    }
}

/// The members that are visible, or all of them when none is, leaving out
/// any that have no point. `visible` is in node order, as `points` is.
pub fn shown_members(points: &[Vec3], visible: &[bool], members: &[usize]) -> Vec<usize> {
    let known = |&&i: &&usize| i < points.len();
    let shown: Vec<usize> = members
        .iter()
        .filter(known)
        .filter(|&&i| is_visible(visible, i))
        .copied()
        .collect();
    if shown.is_empty() {
        members.iter().filter(known).copied().collect()
    } else {
        shown
    }
}

/// The points a fit holds for node `i`: its place, and where it spans a
/// stretch of the time axis, its place at each end of it. With no spans,
/// every node is its place.
pub fn node_points(points: &[Vec3], spans: &[Option<(f32, f32)>], i: usize) -> Vec<Vec3> {
    let Some(&at) = points.get(i) else {
        return Vec::new();
    };
    match spans.get(i).copied().flatten() {
        Some((lo, hi)) => vec![at.with_y(lo), at.with_y(hi)],
        None => vec![at],
    }
}

/// The points of these members that are visible, or of all of them when
/// none is, each column by both its ends.
pub fn framed(
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    visible: &[bool],
    members: &[usize],
) -> Vec<Vec3> {
    shown_members(points, visible, members)
        .into_iter()
        .flat_map(|i| node_points(points, spans, i))
        .collect()
}

/// The visible points, or every point when none is.
pub fn visible_points(
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    visible: &[bool],
) -> Vec<Vec3> {
    let all: Vec<usize> = (0..points.len()).collect();
    framed(points, spans, visible, &all)
}

/// The points the "now" preset frames. In the latest layer, the points of
/// the visible nodes within a day of the highest. At a height, the visible
/// nodes whose disc or column comes within a day of it; where none does,
/// the visible nodes' places brought to that height, so that the view
/// shows the map at "now".
pub fn now_points(
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    visible: &[bool],
    layer: NowLayer,
) -> Vec<Vec3> {
    let all: Vec<usize> = (0..points.len()).collect();
    match layer {
        NowLayer::Latest(day) => latest_layer(&framed(points, spans, visible, &all), day),
        NowLayer::At { height, day } => {
            let shown = shown_members(points, visible, &all);
            let near: Vec<usize> = shown
                .iter()
                .copied()
                .filter(|&i| {
                    let (lo, hi) = spans
                        .get(i)
                        .copied()
                        .flatten()
                        .unwrap_or((points[i].y, points[i].y));
                    lo <= height + day && hi >= height - day
                })
                .collect();
            if near.is_empty() {
                shown.iter().map(|&i| points[i].with_y(height)).collect()
            } else {
                framed(points, spans, &[], &near)
            }
        }
    }
}

/// Whether any member of a region is visible.
pub fn region_visible(region: &Region, visible: &[bool]) -> bool {
    region.members.iter().any(|&i| is_visible(visible, i))
}

/// A preset's view of the visible nodes, for a screen: their places, or
/// where they span a stretch of the time axis, both ends of it.
pub fn preset_view(
    preset: Preset,
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    visible: &[bool],
    layer: NowLayer,
    screen: impl Into<Screen>,
) -> View {
    let screen = screen.into();
    let now = || now_points(points, spans, visible, layer);
    let points = &visible_points(points, spans, visible);
    match preset {
        Preset::Graph => fit(points, GRAPH_YAW, GRAPH_PITCH, Lens::Orthographic, screen),
        Preset::Timeline => fit(
            points,
            TIMELINE_YAW,
            TIMELINE_PITCH,
            Lens::Orthographic,
            screen,
        ),
        Preset::Free => fit(points, FREE_YAW, FREE_PITCH, Lens::Perspective, screen),
        Preset::Now => fit(&now(), FREE_YAW, FREE_PITCH, Lens::Perspective, screen),
    }
}

/// The view of one region's visible nodes from this direction and
/// projection.
#[allow(clippy::too_many_arguments)]
pub fn region_view(
    region: &Region,
    points: &[Vec3],
    spans: &[Option<(f32, f32)>],
    visible: &[bool],
    yaw: f32,
    pitch: f32,
    lens: Lens,
    screen: impl Into<Screen>,
) -> View {
    fit(
        &framed(points, spans, visible, &region.members),
        yaw,
        pitch,
        lens,
        screen,
    )
}

/// The view a camera shows now, as the start of a tween toward `toward`.
///
/// An orthographic camera's focus is the point its height is measured at.
/// A perspective camera has no focus of its own, so it takes the point in
/// front of it level with `toward`, the target's focus, and the height the
/// view spans there; starting from either, the tween begins exactly where
/// the camera is.
pub fn current_view(
    camera: &Transform,
    yaw: f32,
    pitch: f32,
    orthographic_height: Option<f32>,
    toward: Vec3,
) -> View {
    let forward = rotation(yaw, pitch) * Vec3::NEG_Z;
    let (depth, lens) = match orthographic_height {
        Some(height) => (distance(height), Lens::Orthographic),
        None => (
            (toward - camera.translation).dot(forward).max(MIN_DEPTH),
            Lens::Perspective,
        ),
    };
    View {
        focus: camera.translation + forward * depth,
        yaw,
        pitch,
        height: 2.0 * depth * (FOV / 2.0).tan(),
        lens,
    }
}

/// Cubic ease-in-out over `t` from 0 to 1: slow to leave, slow to arrive.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (2.0 - 2.0 * t).powi(3) / 2.0
    }
}

/// An angle brought into (-π, π].
fn wrap(angle: f32) -> f32 {
    let wrapped = (angle + PI).rem_euclid(TAU) - PI;
    if wrapped <= -PI {
        wrapped + TAU
    } else {
        wrapped
    }
}

/// The view a fraction `s` of the way from one view to another, where `s`
/// is already eased. The projection is the target's throughout. The focus
/// moves in a straight line, the yaw turns the short way round, and the
/// height changes by a constant factor per step, so that a zoom feels even.
pub fn between(from: &View, to: &View, s: f32) -> View {
    let s = s.clamp(0.0, 1.0);
    let height = (from.height.ln() + (to.height.ln() - from.height.ln()) * s).exp();
    View {
        focus: from.focus.lerp(to.focus, s),
        yaw: to.yaw - wrap(to.yaw - from.yaw) * (1.0 - s),
        pitch: from.pitch + (to.pitch - from.pitch) * s,
        height,
        lens: to.lens,
    }
}

/// Every topic's region, keyed by the topic node's index: the topic and the
/// claims the layout placed beside it, in node order.
pub fn topic_regions(nodes: &[Placed], names: &HashMap<&str, &str>) -> HashMap<usize, Region> {
    let mut regions: HashMap<usize, Region> = HashMap::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if node.kind == Kind::Topic {
            index.insert(node.node_id.as_str(), i);
            let name = names.get(node.node_id.as_str()).copied();
            regions.insert(
                i,
                Region {
                    id: node.node_id.clone(),
                    name: name.unwrap_or(&node.node_id).to_owned(),
                    members: vec![i],
                },
            );
        }
    }
    for (i, node) in nodes.iter().enumerate() {
        let topic = node.topic.as_deref().and_then(|t| index.get(t));
        if let Some(region) = topic.and_then(|t| regions.get_mut(t)) {
            region.members.push(i);
        }
    }
    regions
}

/// Every metacontext with at least one drawn node, in id order: the nodes
/// its `has_metacontext` edges come from. A metacontext the snapshot does
/// not list is named by its id.
pub fn metacontext_regions(
    nodes: &[Placed],
    edges: &[EdgeView],
    metacontexts: &[MetacontextView],
) -> Vec<Region> {
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.node_id.as_str(), i))
        .collect();
    let names: HashMap<&str, &str> = metacontexts
        .iter()
        .map(|m| (m.metacontext_id.as_str(), m.content.as_str()))
        .collect();
    let mut members: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    for edge in edges.iter().filter(|e| e.edge_type == "has_metacontext") {
        if let Some(&i) = index.get(edge.src_id.as_str()) {
            members.entry(edge.dst_id.as_str()).or_default().insert(i);
        }
    }
    members
        .into_iter()
        .map(|(id, members)| Region {
            id: id.to_owned(),
            name: names.get(id).copied().unwrap_or(id).to_owned(),
            members: members.into_iter().collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TIME_HEIGHT;
    use crate::scene::{DISC_HEIGHT, disc_radius};

    const ASPECT: f32 = 16.0 / 9.0;

    /// Topics on a ring with claims beside them, rising through the time
    /// axis like slice 1's picture, and a claim far outside the ring. The
    /// shape is deliberately not a circle about the origin, so a preset
    /// that assumed the circle would miss the outlier.
    fn points() -> Vec<Vec3> {
        let mut points: Vec<Vec3> = (0..64)
            .flat_map(|i| {
                let angle = i as f32 / 64.0 * TAU;
                let topic = Vec3::new(200.0 * angle.cos(), i as f32 * 4.0, 200.0 * angle.sin());
                [topic, topic + Vec3::new(4.0, 30.0, -3.0)]
            })
            .collect();
        points.push(Vec3::new(260.0, 10.0, -330.0));
        points
    }

    /// Where a point lands in a view, from -1 to 1 across and up when it is
    /// inside, and how far in front of the camera it is.
    fn project(view: &View, point: Vec3) -> (f32, f32, f32) {
        let camera = transform(view);
        let local = camera.rotation.inverse() * (point - camera.translation);
        let depth = -local.z;
        let half_up = match view.lens {
            Lens::Orthographic => view.height / 2.0,
            Lens::Perspective => depth * (FOV / 2.0).tan(),
        };
        (local.x / (half_up * ASPECT), local.y / half_up, depth)
    }

    fn inside(view: &View, point: Vec3) -> bool {
        let (x, y, depth) = project(view, point);
        let edge = 1.0 + 1e-4;
        x.abs() <= edge && y.abs() <= edge && (view.lens == Lens::Orthographic || depth > 0.0)
    }

    fn forward(view: &View) -> Vec3 {
        *transform(view).forward()
    }

    #[test]
    fn the_graph_preset_looks_straight_down_and_holds_every_point() {
        let points = points();
        let view = preset_view(
            Preset::Graph,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        assert_eq!(view.lens, Lens::Orthographic);
        assert!(
            forward(&view).distance(Vec3::NEG_Y) < 1e-5,
            "{}",
            forward(&view)
        );
        assert!(points.iter().all(|&p| inside(&view, p)));
        // Tight on the map: the outlier's z, 330 out, is what sets the height.
        let (min, max) = bounds(&points).unwrap();
        let expected = (max.z - min.z + 2.0 * NODE_PAD) * FIT_MARGIN;
        assert!((view.height - expected).abs() < 1e-2, "{}", view.height);
        assert!((view.focus.x - (min.x + max.x) / 2.0).abs() < 1e-3);
        assert!((view.focus.z - (min.z + max.z) / 2.0).abs() < 1e-3);
        // The camera is above the whole stack and sees it from the top.
        assert!(transform(&view).translation.y > max.y);
    }

    #[test]
    fn the_timeline_preset_is_side_on_and_holds_the_full_time_range() {
        let points = [
            Vec3::new(-150.0, 0.0, 10.0),
            Vec3::new(150.0, TIME_HEIGHT, -40.0),
        ];
        let view = preset_view(
            Preset::Timeline,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        assert_eq!(view.lens, Lens::Orthographic);
        assert!(forward(&view).distance(Vec3::NEG_Z) < 1e-5);
        assert!(transform(&view).up().distance(Vec3::Y) < 1e-5, "time is up");
        let bottom = view.focus.y - view.height / 2.0;
        let top = view.focus.y + view.height / 2.0;
        assert!(bottom < 0.0 && top > TIME_HEIGHT, "{bottom}..{top}");
        assert!(points.iter().all(|&p| inside(&view, p)));
        // Every point reads at its time: the screen height is the y.
        let (_, low, _) = project(&view, points[0]);
        let (_, high, _) = project(&view, points[1]);
        assert!(((high - low) * view.height / 2.0 - TIME_HEIGHT).abs() < 1e-2);
    }

    #[test]
    fn the_3d_preset_frames_everything_from_above_and_to_one_side() {
        let points = points();
        let view = preset_view(
            Preset::Free,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        assert_eq!(view.lens, Lens::Perspective);
        assert!(points.iter().all(|&p| inside(&view, p)));
        let look = forward(&view);
        assert!(look.y < 0.0 && look.y > -0.5, "down a little: {look}");
        let (min, max) = bounds(&points).unwrap();
        assert!(transform(&view).translation.y > (min.y + max.y) / 2.0);
        // Tight: some point reaches the edge of the margin.
        let widest = points
            .iter()
            .map(|&p| {
                let (x, y, _) = project(&view, p);
                x.abs().max(y.abs())
            })
            .fold(0.0_f32, f32::max);
        assert!(widest > 0.9 / FIT_MARGIN && widest <= 1.0, "{widest}");
    }

    #[test]
    fn a_fit_below_the_bar_holds_its_points_in_the_part_left_clear() {
        let points = points();
        let inset = 0.12;
        let screen = Screen {
            aspect: ASPECT,
            inset,
            right: 0.0,
        };
        let mut views: Vec<View> = [Preset::Graph, Preset::Timeline, Preset::Free]
            .into_iter()
            .map(|preset| preset_view(preset, &points, &[], &[], NowLayer::Latest(1.0), screen))
            .collect();
        for lens in [Lens::Perspective, Lens::Orthographic] {
            for &(yaw, pitch) in &[(0.0, 0.0), (2.5, -0.1), (-1.0, 1.2), (0.7, -FRAC_PI_2)] {
                views.push(fit(&points, yaw, pitch, lens, screen));
            }
        }
        // The band covers the screen's top from 1 - 2 * inset to 1.
        let top = 1.0 - 2.0 * inset + 1e-4;
        for view in &views {
            let highest = points
                .iter()
                .map(|&p| {
                    assert!(inside(view, p), "{view:?}: {p} outside");
                    project(view, p).1
                })
                .fold(f32::MIN, f32::max);
            assert!(
                highest <= top,
                "{view:?}: a point at {highest}, under the bar"
            );
        }
        // The band changes the fit only by what it covers: the graph
        // preset's height is the clear part's.
        let plain = preset_view(
            Preset::Graph,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        let below = &views[0];
        assert!((below.height * (1.0 - inset) - plain.height).abs() < 1e-2);
        assert_eq!(
            fit(
                &points,
                0.3,
                -0.4,
                Lens::Perspective,
                Screen {
                    aspect: ASPECT,
                    inset: 0.0,
                    right: 0.0,
                }
            ),
            fit(&points, 0.3, -0.4, Lens::Perspective, ASPECT),
            "no band, no change"
        );
    }

    #[test]
    fn a_fit_beside_the_panel_holds_its_points_in_the_part_left_clear() {
        let points = points();
        let (inset, right) = (0.1, 0.3);
        let screen = Screen {
            aspect: ASPECT,
            inset,
            right,
        };
        let mut views: Vec<View> = [Preset::Graph, Preset::Timeline, Preset::Free]
            .into_iter()
            .map(|preset| preset_view(preset, &points, &[], &[], NowLayer::Latest(1.0), screen))
            .collect();
        for lens in [Lens::Perspective, Lens::Orthographic] {
            for &(yaw, pitch) in &[(0.0, 0.0), (2.5, -0.1), (-1.0, 1.2), (0.7, -FRAC_PI_2)] {
                views.push(fit(&points, yaw, pitch, lens, screen));
            }
        }
        // The panel covers the screen's right from 1 - 2 * right to 1, and
        // the bar its top from 1 - 2 * inset.
        let (edge, top) = (1.0 - 2.0 * right + 1e-4, 1.0 - 2.0 * inset + 1e-4);
        for view in &views {
            let (rightmost, highest) = points
                .iter()
                .map(|&p| {
                    assert!(inside(view, p), "{view:?}: {p} outside");
                    let (x, y, _) = project(view, p);
                    (x, y)
                })
                .fold((f32::MIN, f32::MIN), |(a, b), (x, y)| (a.max(x), b.max(y)));
            assert!(
                rightmost <= edge,
                "{view:?}: a point at {rightmost}, under the panel"
            );
            assert!(
                highest <= top,
                "{view:?}: a point at {highest}, under the bar"
            );
        }
        // Side-on, the graph's width decides the fit beside the panel, and
        // the view spans more by what the panel covers.
        let plain = Screen {
            right: 0.0,
            ..screen
        };
        let wide = fit(&points, 0.0, 0.0, Lens::Orthographic, plain);
        let beside = fit(&points, 0.0, 0.0, Lens::Orthographic, screen);
        assert!(beside.height > wide.height, "{beside:?} {wide:?}");
        assert_eq!(
            fit(&points, 0.3, -0.4, Lens::Perspective, plain),
            fit(
                &points,
                0.3,
                -0.4,
                Lens::Perspective,
                Screen {
                    right: f32::NAN,
                    ..screen
                }
            ),
            "a fraction that is not a number covers nothing"
        );
    }

    #[test]
    fn every_fit_holds_its_points_from_any_direction() {
        let points = points();
        for lens in [Lens::Perspective, Lens::Orthographic] {
            for &(yaw, pitch) in &[(0.0, 0.0), (2.5, -0.1), (-1.0, 1.2), (0.7, -FRAC_PI_2)] {
                let view = fit(&points, yaw, pitch, lens, ASPECT);
                for &p in &points {
                    assert!(inside(&view, p), "{lens:?} {yaw} {pitch}: {p} outside");
                }
            }
        }
    }

    #[test]
    fn the_now_preset_looks_at_the_latest_layer() {
        let points = points();
        let top = points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        let day = 10.0;
        let layer = latest_layer(&points, day);
        assert!(!layer.is_empty() && layer.len() < points.len() / 4);
        assert!(layer.iter().all(|p| p.y >= top - day));

        let now = preset_view(
            Preset::Now,
            &points,
            &[],
            &[],
            NowLayer::Latest(day),
            ASPECT,
        );
        let whole = preset_view(
            Preset::Free,
            &points,
            &[],
            &[],
            NowLayer::Latest(day),
            ASPECT,
        );
        assert_eq!(now.lens, Lens::Perspective);
        assert!(
            now.focus.y >= top - day && now.focus.y <= top,
            "{}",
            now.focus
        );
        assert!(layer.iter().all(|&p| inside(&now, p)));
        assert!(now.height < whole.height, "closer than the whole graph");
        assert!(
            forward(&now).distance(forward(&whole)) < 1e-5,
            "the 3D direction"
        );
    }

    #[test]
    fn a_fit_holds_both_ends_of_every_column() {
        // Two discs and a column that reaches far above and below them.
        let points = [
            Vec3::new(-50.0, 100.0, 0.0),
            Vec3::new(50.0, 110.0, 0.0),
            Vec3::new(0.0, 105.0, 0.0),
        ];
        let spans = [
            Some((100.0, 100.0)),
            Some((110.0, 110.0)),
            Some((-200.0, 400.0)),
        ];
        let view = preset_view(
            Preset::Timeline,
            &points,
            &spans,
            &[],
            NowLayer::default(),
            ASPECT,
        );
        assert!(
            inside(&view, Vec3::new(0.0, -200.0, 0.0)) && inside(&view, Vec3::new(0.0, 400.0, 0.0))
        );
        let bare = preset_view(
            Preset::Timeline,
            &points,
            &[],
            &[],
            NowLayer::default(),
            ASPECT,
        );
        assert!(
            bare.height < view.height / 5.0,
            "without the spans, only the places"
        );
        assert_eq!(node_points(&points, &spans, 2).len(), 2);
        assert_eq!(node_points(&points, &[], 2), [points[2]]);
        assert!(node_points(&points, &spans, 9).is_empty());
    }

    #[test]
    fn in_claim_time_the_now_preset_looks_at_the_layer_at_now() {
        // A column through now, a disc at now, a disc long before, and a
        // disc after now that is not visible.
        let points = [
            Vec3::new(0.0, 250.0, 0.0),
            Vec3::new(30.0, 300.0, 0.0),
            Vec3::new(-200.0, 10.0, 0.0),
            Vec3::new(90.0, 302.0, 0.0),
        ];
        let spans = [
            Some((200.0, 301.0)),
            Some((300.0, 300.0)),
            Some((10.0, 10.0)),
            Some((302.0, 302.0)),
        ];
        let visible = [true, true, true, false];
        let layer = NowLayer::At {
            height: 300.5,
            day: 1.0,
        };
        let near = now_points(&points, &spans, &visible, layer);
        assert_eq!(near.len(), 4, "the column's two ends and the disc's");
        assert!(near.iter().all(|p| p.x == 0.0 || p.x == 30.0));
        let view = preset_view(Preset::Now, &points, &spans, &visible, layer, ASPECT);
        assert!(near.iter().all(|&p| inside(&view, p)));
        assert!(!inside(&view, points[2]), "long before now");
        // With nothing at now, the map at now's height.
        let later = NowLayer::At {
            height: 900.0,
            day: 1.0,
        };
        let map = now_points(&points, &spans, &visible, later);
        assert_eq!(map.len(), 3);
        assert!(map.iter().all(|p| p.y == 900.0));
    }

    #[test]
    fn a_day_on_the_axis_is_its_share_of_the_span() {
        let axis = TimeAxis {
            start: 0,
            span: 10 * DAY_MICROS,
            height: 300.0,
        };
        assert!((day_height(&axis) - 30.0).abs() < 1e-4);
        let single = TimeAxis {
            start: 5,
            span: 0,
            height: 300.0,
        };
        assert_eq!(
            latest_layer(&[Vec3::ZERO, Vec3::X], day_height(&single)).len(),
            2
        );
    }

    fn placed(id: &str, kind: Kind, position: Vec3, topic: Option<&str>) -> Placed {
        Placed {
            node_id: id.into(),
            kind,
            position,
            topic: topic.map(Into::into),
            filing: crate::layout::Filing::Filed,
            settled: true,
        }
    }

    /// Two topics far apart, each with claims at several heights, and an
    /// unfiled claim.
    fn nodes() -> Vec<Placed> {
        vec![
            placed("t1", Kind::Topic, Vec3::new(100.0, 0.0, 0.0), None),
            placed("t2", Kind::Topic, Vec3::new(-100.0, 10.0, 0.0), None),
            placed("a", Kind::Fact, Vec3::new(104.0, 50.0, 3.0), Some("t1")),
            placed(
                "b",
                Kind::Inference,
                Vec3::new(96.0, 200.0, -5.0),
                Some("t1"),
            ),
            placed("c", Kind::Fact, Vec3::new(-103.0, 80.0, 2.0), Some("t2")),
            placed("u", Kind::Fact, Vec3::new(250.0, 120.0, 0.0), None),
        ]
    }

    #[test]
    fn a_topic_region_is_the_topic_and_the_claims_placed_beside_it() {
        let nodes = nodes();
        let names: HashMap<&str, &str> = [("t1", "Petritype")].into_iter().collect();
        let regions = topic_regions(&nodes, &names);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[&0].members, [0, 2, 3]);
        assert_eq!(regions[&0].name, "Petritype");
        assert_eq!(regions[&1].members, [1, 4]);
        assert_eq!(regions[&1].name, "t2", "no name, so its id");
    }

    #[test]
    fn a_topic_fit_holds_only_that_topic() {
        let nodes = nodes();
        let points: Vec<Vec3> = nodes.iter().map(|n| n.position).collect();
        let regions = topic_regions(&nodes, &HashMap::new());
        let members = &regions[&0].members;
        let member_points: Vec<Vec3> = members.iter().map(|&i| points[i]).collect();
        for lens in [Lens::Orthographic, Lens::Perspective] {
            for &(yaw, pitch) in &[(GRAPH_YAW, GRAPH_PITCH), (FREE_YAW, FREE_PITCH)] {
                let view = region_view(&regions[&0], &points, &[], &[], yaw, pitch, lens, ASPECT);
                assert_eq!((view.yaw, view.pitch, view.lens), (yaw, pitch, lens));
                assert_eq!(view, fit(&member_points, yaw, pitch, lens, ASPECT));
                assert!(members.iter().all(|&i| inside(&view, points[i])));
                let whole = fit(&points, yaw, pitch, lens, ASPECT);
                assert!(view.height <= whole.height, "{lens:?} {pitch}");
            }
        }
        // Straight down, where nothing else stands in front of the topic or
        // behind it, every node outside the topic is outside the view.
        let down = region_view(
            &regions[&0],
            &points,
            &[],
            &[],
            GRAPH_YAW,
            GRAPH_PITCH,
            Lens::Orthographic,
            ASPECT,
        );
        for (i, &p) in points.iter().enumerate() {
            assert_eq!(inside(&down, p), members.contains(&i), "node {i}");
        }
        // From above, a topic's claims spread a few units on the map, so the
        // fit is its smallest, whatever their times.
        let down = region_view(
            &regions[&1],
            &points,
            &[],
            &[],
            GRAPH_YAW,
            GRAPH_PITCH,
            Lens::Orthographic,
            ASPECT,
        );
        assert_eq!(down.height, MIN_HEIGHT);
    }

    fn edge(src: &str, dst: &str, edge_type: &str) -> EdgeView {
        EdgeView {
            edge_id: format!("{src}-{dst}"),
            src_id: src.into(),
            dst_id: dst.into(),
            edge_type: edge_type.into(),
            weight: 1.0,
            validity: Vec::new(),
            created_at: "2026-10-20T00:00:00Z".into(),
            graph: "test".into(),
            metadata: Default::default(),
        }
    }

    #[test]
    fn a_metacontext_region_is_the_nodes_its_edges_come_from() {
        let nodes = nodes();
        let edges = [
            edge("a", "the-real", "has_metacontext"),
            edge("b", "the-real", "has_metacontext"),
            edge("b", "the-real", "has_metacontext"),
            edge("c", "night-city", "has_metacontext"),
            edge("t2", "night-city", "has_metacontext"),
            edge("gone", "the-real", "has_metacontext"),
            edge("t1", "the-real", "tagged_with_topic"),
        ];
        let metacontexts = [MetacontextView {
            metacontext_id: "the-real".into(),
            content: "The Real".into(),
            description: String::new(),
            graph: "test".into(),
        }];
        let regions = metacontext_regions(&nodes, &edges, &metacontexts);
        let ids: Vec<&str> = regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["night-city", "the-real"], "in id order");
        assert_eq!(regions[0].name, "night-city", "unlisted, so its id");
        assert_eq!(regions[0].members, [1, 4]);
        assert_eq!(regions[1].name, "The Real");
        assert_eq!(regions[1].members, [2, 3], "once each, drawn nodes only");

        let points: Vec<Vec3> = nodes.iter().map(|n| n.position).collect();
        let view = region_view(
            &regions[0],
            &points,
            &[],
            &[],
            TIMELINE_YAW,
            TIMELINE_PITCH,
            Lens::Orthographic,
            ASPECT,
        );
        for (i, &p) in points.iter().enumerate() {
            assert_eq!(
                inside(&view, p),
                regions[0].members.contains(&i),
                "node {i}"
            );
        }
    }

    #[test]
    fn the_ease_leaves_and_arrives_slowly_and_is_symmetric() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
        assert!(ease_in_out(0.1) < 0.1 && ease_in_out(0.9) > 0.9);
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            assert!((ease_in_out(t) + ease_in_out(1.0 - t) - 1.0).abs() < 1e-5);
        }
        let steps: Vec<f32> = (0..=100).map(|i| ease_in_out(i as f32 / 100.0)).collect();
        assert!(steps.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(ease_in_out(-1.0), 0.0);
        assert_eq!(ease_in_out(2.0), 1.0);
    }

    /// Whether two rotations turn the camera's axes the same way. Compared
    /// by the axes because `angle_between` loses precision near zero.
    fn same_rotation(a: Quat, b: Quat) -> bool {
        (a * Vec3::X).distance(b * Vec3::X) < 1e-4 && (a * Vec3::Z).distance(b * Vec3::Z) < 1e-4
    }

    fn same_place(a: &View, b: &View) -> bool {
        let (ta, tb) = (transform(a), transform(b));
        ta.translation.distance(tb.translation) < 1e-2
            && same_rotation(ta.rotation, tb.rotation)
            && (a.height - b.height).abs() < 1e-2
    }

    #[test]
    fn a_tween_runs_from_the_one_view_to_the_other() {
        let points = points();
        let graph = preset_view(
            Preset::Graph,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        let free = preset_view(
            Preset::Free,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        assert!(same_place(&between(&free, &graph, 0.0), &free));
        assert!(same_place(&between(&free, &graph, 1.0), &graph));
        assert_eq!(
            between(&free, &graph, 0.0).lens,
            Lens::Orthographic,
            "switched at the start"
        );
        assert_eq!(between(&graph, &free, 0.3).lens, Lens::Perspective);
        let half = between(&free, &graph, 0.5);
        assert!((half.height - (free.height * graph.height).sqrt()).abs() < 1e-2);
        assert!(half.focus.distance(free.focus.lerp(graph.focus, 0.5)) < 1e-3);
    }

    #[test]
    fn a_tween_turns_the_short_way_round() {
        let view = |yaw: f32| View {
            focus: Vec3::ZERO,
            yaw,
            pitch: 0.0,
            height: 10.0,
            lens: Lens::Perspective,
        };
        let half = between(&view(3.0), &view(-3.0), 0.5);
        assert!(
            (wrap(half.yaw).abs() - PI).abs() < 1e-4,
            "through π: {}",
            half.yaw
        );
        assert!(same_place(
            &between(&view(3.0), &view(-3.0), 0.0),
            &view(3.0)
        ));
        assert_eq!(between(&view(3.0), &view(-3.0), 1.0).yaw, -3.0);
        assert!((wrap(3.0 * TAU + 0.5) - 0.5).abs() < 1e-4);
        assert!((wrap(PI) - PI).abs() < 1e-6 && (wrap(-PI) - PI).abs() < 1e-6);
    }

    #[test]
    fn the_current_view_starts_where_the_camera_is() {
        let points = points();
        let target = preset_view(
            Preset::Graph,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        let camera = Transform::from_xyz(30.0, 400.0, 700.0);
        let (yaw, pitch) = (0.3, -0.4);
        let camera = Transform {
            rotation: rotation(yaw, pitch),
            ..camera
        };
        for height in [None, Some(250.0)] {
            let start = current_view(&camera, yaw, pitch, height, target.focus);
            let at = transform(&start);
            assert!(
                at.translation.distance(camera.translation) < 1e-2,
                "{height:?}"
            );
            assert!(same_rotation(at.rotation, camera.rotation));
        }
        let ortho = current_view(&camera, yaw, pitch, Some(250.0), target.focus);
        assert!((ortho.height - 250.0).abs() < 1e-3);
        assert_eq!(ortho.lens, Lens::Orthographic);
        // A perspective camera looking away from the target still starts
        // where it is, looking at a point just in front of it.
        let away = current_view(&camera, yaw + PI, pitch, None, target.focus);
        assert!(transform(&away).translation.distance(camera.translation) < 1e-2);
    }

    #[test]
    fn presets_parse_by_name() {
        assert_eq!(parse_preset("graph"), Some(Preset::Graph));
        assert_eq!(parse_preset("Timeline"), Some(Preset::Timeline));
        assert_eq!(parse_preset("3D"), Some(Preset::Free));
        assert_eq!(parse_preset("now"), Some(Preset::Now));
        assert_eq!(parse_preset("sideways"), None);
    }

    #[test]
    fn the_pad_holds_the_largest_disc() {
        for kind in [Kind::Topic, Kind::Fact, Kind::Inference] {
            assert!(NODE_PAD >= disc_radius(kind));
        }
        const { assert!(NODE_PAD >= DISC_HEIGHT / 2.0) };
    }

    #[test]
    fn a_fit_frames_only_the_visible_nodes() {
        let points = points();
        // Hide the outlier and the ring's far half: the graph preset shrinks
        // to what is left, and holds all of it.
        let visible: Vec<bool> = points.iter().map(|p| p.z < 0.0 && p.z > -300.0).collect();
        let shown: Vec<Vec3> = (0..points.len())
            .filter(|&i| visible[i])
            .map(|i| points[i])
            .collect();
        let all = preset_view(
            Preset::Graph,
            &points,
            &[],
            &[],
            NowLayer::Latest(1.0),
            ASPECT,
        );
        let some = preset_view(
            Preset::Graph,
            &points,
            &[],
            &visible,
            NowLayer::Latest(1.0),
            ASPECT,
        );
        assert_eq!(
            some,
            fit(&shown, GRAPH_YAW, GRAPH_PITCH, Lens::Orthographic, ASPECT)
        );
        assert!(some.height < all.height);
        assert!(shown.iter().all(|&p| inside(&some, p)));
        assert!(
            !inside(&some, *points.last().unwrap()),
            "the hidden outlier"
        );

        // Nothing visible frames everything rather than the origin.
        let none = vec![false; points.len()];
        assert_eq!(
            preset_view(
                Preset::Graph,
                &points,
                &[],
                &none,
                NowLayer::Latest(1.0),
                ASPECT
            ),
            all
        );
    }

    #[test]
    fn a_region_frames_its_visible_members_or_all_of_them() {
        let nodes = nodes();
        let points: Vec<Vec3> = nodes.iter().map(|n| n.position).collect();
        let regions = topic_regions(&nodes, &HashMap::new());
        let t1 = &regions[&0];
        assert_eq!(t1.members, [0, 2, 3]);
        let without_b = [true, true, true, false, true, true];
        assert_eq!(
            framed(&points, &[], &without_b, &t1.members),
            [points[0], points[2]]
        );
        assert!(region_visible(t1, &without_b));
        let hidden = [false, true, false, false, true, true];
        assert!(!region_visible(t1, &hidden));
        assert_eq!(framed(&points, &[], &hidden, &t1.members).len(), 3);
        let view = region_view(
            t1,
            &points,
            &[],
            &without_b,
            TIMELINE_YAW,
            TIMELINE_PITCH,
            Lens::Orthographic,
            ASPECT,
        );
        assert!(inside(&view, points[2]) && !inside(&view, points[3]));
    }
}
