//! The scene: a disc or a column for every node, at the place the layout
//! gave it and the stretch of the time axis its time mode gives it.
//!
//! A disc is the shortest column (DESIGN.md §4): a node with no claim time
//! is a disc at its recorded time. Every node is drawn with one of four
//! cylinder meshes, one per profile along the axis: solid, for discs and
//! interval columns, and a smear that fades at both ends, at the top only
//! or at the bottom only. Each mesh is one unit high and carries its
//! profile in the alpha of its vertex colours, so one mesh serves every
//! column of its profile, scaled per node to its kind's radius and its
//! column's height ([`shape`]). Every node shares one material, and the four
//! meshes share one vertex layout, so Bevy draws them all in one batch
//! (DESIGN.md §8). What differs per node rides in its [`MeshTag`], the number
//! Bevy gives each instance's shader: the kind's colour and the node's
//! alpha, packed into four bytes by [`disc_tag`] and read back by
//! `disc.wgsl`, which multiplies that alpha by the mesh's along the column.
//! The material is unlit and blends additively, which needs no sorting and
//! turns overlap into brightness, and is why the background is dark.
//!
//! A node's alpha comes from the filter (`filter.rs`), times a falloff with
//! its distance from the focused node where one is set (`focus.rs`), so that
//! the focus's neighbourhood stands out and the rest of the graph stays in
//! view. The hovered disc is drawn lighter and whole, and the focused one
//! lighter still and larger. The nodes at the far ends of the hovered
//! node's claim-to-claim edges brighten a little, and those of the focused
//! node's more (DESIGN.md §5). The tags and the focused disc's size are
//! written again only in an update in which what the filter shows, the
//! focus or the hover changed, and a tag only where it differs.
//!
//! The colours and widths of the edges' lines (`edges.rs`) are kept here
//! too, beside the discs' colours, so that the palette is in one place.
//!
//! A graph switch (`reload.rs`) despawns every disc with [`despawn_discs`]
//! and spawns the new graph's with [`spawn_discs`], on the same mesh and
//! material, so the new graph is drawn in one call as well. A node arriving
//! on the event stream gets its disc from [`spawn_nodes`], on the same mesh
//! and material, and every other disc stays as it is (`apply.rs`).

use bevy::asset::embedded_asset;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use web_time::Instant;

use crate::camera::Places;
use crate::edge_index::{Relation, Succession};
use crate::filter::{RefreshShown, Shown, filter_alpha, is_visible};
use crate::focus::{self, Focus, Highlight, Hovered};
use crate::graph_index::GraphIndex;
use crate::layout::{Kind, Placed};
use crate::time_axis::{self, Profile, Span, TimeMode, TimeModel, centre};

/// The background. Additive blending washes out on anything light.
pub const BACKGROUND: Color = Color::srgb(0.015, 0.016, 0.03);

/// How tall every disc is, in world units: enough that a disc seen side-on,
/// in the timeline preset, reads as a dash a few pixels thick, and still a
/// disc from above. A column is never drawn shorter.
pub const DISC_HEIGHT: f32 = 1.0;
/// How tall an instant timepoint's disc is: thinner than a disc at a
/// recorded time.
pub const INSTANT_HEIGHT: f32 = 0.35;
const _: () = assert!(
    INSTANT_HEIGHT < DISC_HEIGHT,
    "an instant is thinner than a disc"
);
/// The alpha a smear across two known bounds keeps at each of them, as a
/// share of its middle's, so that the bounds still show.
pub const SMEAR_EDGE_ALPHA: f32 = 0.25;
/// How many rings a smear's mesh has along its height, for its fade.
pub const SMEAR_SEGMENTS: u32 = 16;
/// How many sides every mesh has around.
pub const MESH_RESOLUTION: u32 = 32;

/// How much wider the focused disc is drawn than others of its kind.
pub const FOCUS_SCALE: f32 = 1.6;
/// How far toward white the hovered and the focused disc's colour goes.
pub const HOVER_LIGHT: f32 = 0.35;
pub const FOCUS_LIGHT: f32 = 0.6;
/// How far toward white, and how far from its alpha toward whole, a node
/// at the far end of the hovered node's claim-to-claim edges goes, and one
/// at the far end of the focused node's: a little, and more.
pub const NEAR_HOVER_LIGHT: f32 = 0.12;
pub const NEAR_HOVER_LIFT: f32 = 0.3;
pub const NEAR_FOCUS_LIGHT: f32 = 0.25;
pub const NEAR_FOCUS_LIFT: f32 = 0.6;

/// The width of the lines the hover and the focus light, in the render
/// target's pixels, which is how Bevy's gizmos measure them: on a display
/// of twice the density a line is half as wide to the eye. The edge
/// toggle's lines and the connectors are a mesh of hardware lines, one
/// pixel wide.
pub const HOVER_LINE_WIDTH: f32 = 2.5;
pub const FOCUS_LINE_WIDTH: f32 = 4.0;
/// How much of its colour a line lit by the hover shows, and one lit by
/// the focus.
pub const HOVER_LINE_ALPHA: f32 = 0.6;
pub const FOCUS_LINE_ALPHA: f32 = 1.0;

/// Where `embedded_asset!` puts the discs' shader.
const DISC_SHADER: &str = "embedded://epimemer_bevy/disc.wgsl";

/// The nodes to draw, as the layout placed them. The spawn at startup
/// takes them.
#[derive(Resource, Debug, Clone, Default)]
pub struct Nodes(pub Vec<Placed>);

/// The meshes, one per profile, and the material every node shares, made
/// once.
#[derive(Resource, Debug, Clone)]
pub struct DiscAssets {
    pub solid: Handle<Mesh>,
    pub fade_both: Handle<Mesh>,
    pub fade_top: Handle<Mesh>,
    pub fade_bottom: Handle<Mesh>,
    pub material: Handle<DiscMaterial>,
}

/// The mesh a profile is drawn with.
pub fn profile_mesh(assets: &DiscAssets, profile: Profile) -> Handle<Mesh> {
    match profile {
        Profile::Disc | Profile::Instant | Profile::Solid => assets.solid.clone(),
        Profile::FadeBoth => assets.fade_both.clone(),
        Profile::FadeTop => assets.fade_top.clone(),
        Profile::FadeBottom => assets.fade_bottom.clone(),
    }
}

/// The share of its alpha a column of a profile has at `t` of the way up,
/// from 0 at its foot to 1 at its top. A smear across two bounds is whole
/// in the middle and falls to [`SMEAR_EDGE_ALPHA`] at each; a smear with
/// one bound is whole at it and fades to nothing at the other end.
pub fn profile_alpha(profile: Profile, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let rise = t * t * (3.0 - 2.0 * t);
    match profile {
        Profile::Disc | Profile::Instant | Profile::Solid => 1.0,
        Profile::FadeBoth => {
            SMEAR_EDGE_ALPHA + (1.0 - SMEAR_EDGE_ALPHA) * (std::f32::consts::PI * t).sin()
        }
        Profile::FadeTop => 1.0 - rise,
        Profile::FadeBottom => rise,
    }
}

/// A profile's mesh: a cylinder of radius 1 and height 1 about the origin,
/// with rings enough for its fade, its vertex colours white with the
/// profile's alpha at each ring's height.
pub fn column_mesh(profile: Profile) -> Mesh {
    let segments = match profile {
        Profile::Disc | Profile::Instant | Profile::Solid => 1,
        _ => SMEAR_SEGMENTS,
    };
    let mut mesh = Cylinder::new(1.0, 1.0)
        .mesh()
        .resolution(MESH_RESOLUTION)
        .segments(segments)
        .build();
    let colours: Vec<[f32; 4]> = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) => positions
            .iter()
            .map(|p| [1.0, 1.0, 1.0, profile_alpha(profile, p[1] + 0.5)])
            .collect(),
        _ => Vec::new(),
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours);
    mesh
}

/// The height a profile is never drawn below.
pub fn min_height(profile: Profile) -> f32 {
    match profile {
        Profile::Instant => INSTANT_HEIGHT,
        _ => DISC_HEIGHT,
    }
}

/// How a node is drawn on the time axis: the height of its middle, how
/// tall it is, and its profile. A node with no span, which has no position
/// in the time mode drawn, keeps a disc at `fallback`, its recorded height,
/// and is not drawn.
pub fn shape(span: Option<Span>, fallback: f32) -> (f32, f32, Profile) {
    match span {
        Some(span) => (
            centre(&span),
            (span.hi - span.lo).abs().max(min_height(span.profile)),
            span.profile,
        ),
        None => (fallback, DISC_HEIGHT, Profile::Disc),
    }
}

/// A node's disc: the node's index in the layout, which is also its index
/// among the camera's points and in the graph index, and its kind.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disc {
    pub index: usize,
    pub kind: Kind,
}

/// The one material every disc shares. It has no fields: the colour and
/// the alpha are each disc's [`MeshTag`].
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Default)]
pub struct DiscMaterial {}

impl Material for DiscMaterial {
    fn fragment_shader() -> ShaderRef {
        DISC_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }

    // A translucent disc writes no depth and casts no shadow, so the passes
    // for those have nothing to do.
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "disc.wgsl");
        app.add_plugins(MaterialPlugin::<DiscMaterial>::default())
            .insert_resource(ClearColor(BACKGROUND))
            .init_resource::<Focus>()
            .init_resource::<Hovered>()
            .add_systems(Startup, spawn_startup_discs)
            .add_systems(PostUpdate, apply_alphas.after(RefreshShown));
    }
}

/// The radius of a kind's disc. Topics are the landmarks, so they are the
/// largest; inferences sit a little larger than the facts they draw on.
pub fn disc_radius(kind: Kind) -> f32 {
    match kind {
        Kind::Topic => 1.8,
        Kind::Fact => 1.1,
        Kind::Inference => 1.3,
    }
}

/// A kind's colour, with the alpha that scales how much it adds.
pub fn kind_colour(kind: Kind) -> Color {
    match kind {
        Kind::Topic => Color::srgba(1.0, 0.72, 0.28, 0.9),
        Kind::Fact => Color::srgba(0.3, 0.68, 1.0, 0.55),
        Kind::Inference => Color::srgba(1.0, 0.34, 0.74, 0.7),
    }
}

/// A claim-to-claim line's colour, by its relation, with the alpha that
/// scales how much it adds.
pub fn relation_colour(relation: Relation) -> Color {
    match relation {
        Relation::Supports => Color::srgba(0.35, 0.95, 0.55, 0.75),
        Relation::Assessed => Color::srgba(1.0, 0.6, 0.25, 0.75),
        Relation::Related => Color::srgba(0.85, 0.5, 1.0, 0.75),
        Relation::Similarity => Color::srgba(0.25, 0.95, 0.95, 0.75),
        Relation::Contradiction => Color::srgba(1.0, 0.3, 0.3, 0.85),
        Relation::Other => Color::srgba(0.75, 0.75, 0.8, 0.6),
    }
}

/// A succession connector's colour: one for a node that gave way to
/// another, and a second for a fact to the inference that depended on it.
pub fn succession_colour(succession: Succession) -> Color {
    if succession.is_evidence() {
        Color::srgba(1.0, 0.85, 0.4, 0.9)
    } else {
        Color::srgba(0.88, 0.92, 1.0, 0.9)
    }
}

/// The time axis's line and ticks, and the dates beside them.
pub const AXIS_COLOUR: Color = Color::srgba(0.55, 0.6, 0.72, 0.8);
pub const TICK_TEXT_COLOUR: Color = Color::srgba(0.62, 0.67, 0.78, 0.95);
/// "Now": its tick and date, and the faint ring around the map at its
/// height.
pub const NOW_COLOUR: Color = Color::srgba(0.45, 1.0, 0.62, 0.95);
pub const NOW_RING_COLOUR: Color = Color::srgba(0.45, 1.0, 0.62, 0.22);
/// A validity bracket beside the focused column: a stated interval, and
/// one read from tense and context.
pub const BRACKET_COLOUR: Color = Color::srgba(1.0, 0.92, 0.45, 0.95);
pub const INFERRED_BRACKET_COLOUR: Color = Color::srgba(1.0, 0.92, 0.45, 0.55);

/// A colour and an alpha as the four bytes the shader reads: the colour's
/// sRGB red, green and blue, then the alpha, red in the lowest byte, which
/// is the order WGSL's `unpack4x8unorm` takes them in.
pub fn pack_rgba(colour: Color, alpha: f32) -> u32 {
    let [r, g, b, _] = colour.to_srgba().to_u8_array();
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    u32::from_le_bytes([r, g, b, a])
}

/// The tag of a disc of this kind at this alpha, where the alpha is a
/// fraction of the kind's own: 1 draws the kind as [`kind_colour`] gives
/// it, 0 draws nothing.
pub fn disc_tag(kind: Kind, alpha: f32) -> MeshTag {
    let colour = kind_colour(kind);
    MeshTag(pack_rgba(colour, colour.alpha() * alpha.clamp(0.0, 1.0)))
}

/// The tag of a disc at this alpha, as a fraction of its kind's own, that
/// stands out as `highlight` says: a hovered or focused disc is its kind's
/// colour taken toward white, and whole whatever the alpha, so that it shows
/// even where the filter dims it or the focus fades it; a disc at the far
/// end of the hovered or the focused node's edge is taken a little toward
/// white and toward whole. A hidden disc, at alpha 0, stays hidden.
pub fn node_tag(kind: Kind, alpha: f32, highlight: Highlight) -> MeshTag {
    let (light, lift) = match highlight {
        _ if alpha <= 0.0 => return disc_tag(kind, 0.0),
        Highlight::Plain => return disc_tag(kind, alpha),
        Highlight::NearHovered => (NEAR_HOVER_LIGHT, NEAR_HOVER_LIFT),
        Highlight::NearFocused => (NEAR_FOCUS_LIGHT, NEAR_FOCUS_LIFT),
        Highlight::Hovered => (HOVER_LIGHT, 1.0),
        Highlight::Focused => (FOCUS_LIGHT, 1.0),
    };
    let colour = kind_colour(kind);
    let lit = colour.to_srgba().mix(&Srgba::WHITE, light);
    let own = colour.alpha() * alpha.clamp(0.0, 1.0);
    MeshTag(pack_rgba(lit.into(), own + (1.0 - own) * lift))
}

/// A disc's scale: its kind's radius across, wider when it is focused, and
/// a disc's height. A column's height replaces the last.
pub fn disc_scale(kind: Kind, focused: bool) -> Vec3 {
    let radius = disc_radius(kind) * if focused { FOCUS_SCALE } else { 1.0 };
    Vec3::new(radius, DISC_HEIGHT, radius)
}

/// A node's alpha as the filter shows it: none where it is not visible,
/// as a node the filter hides or one with no position is not. A node the
/// filter has no word on is drawn whole.
fn shown_alpha(shown: &Shown, index: usize) -> f32 {
    if !is_visible(&shown.visible, index) {
        return 0.0;
    }
    filter_alpha(shown.mode, shown.passes.get(index).copied().unwrap_or(true))
}

/// Gives every disc the tag and the size the filter, the focus and the
/// hover give it, in any update in which one of them changed or the places
/// moved, as they do through a switch of time mode, and in no other. A tag or a size is written only where it differs, so that Bevy
/// sends the renderer only the discs that changed.
fn apply_alphas(
    shown: Res<Shown>,
    focus: Res<Focus>,
    hovered: Res<Hovered>,
    index: Res<GraphIndex>,
    places: Option<Res<Places>>,
    mut discs: Query<(&Disc, &mut MeshTag, &mut Transform)>,
) {
    let moved = places.is_some_and(|places| places.is_changed());
    if !(shown.is_changed() || focus.is_changed() || hovered.is_changed() || moved) {
        return;
    }
    let near = focus::near(&index.edges, *focus, *hovered);
    let centre = focus.0.and_then(|wanted| {
        discs
            .iter()
            .find(|(disc, _, _)| disc.index == wanted)
            .map(|(_, _, transform)| transform.translation)
    });
    for (disc, mut tag, mut transform) in &mut discs {
        let filter = shown_alpha(&shown, disc.index);
        let alpha = focus::node_alpha(filter, centre, transform.translation);
        let highlight = focus::highlight(disc.index, *focus, *hovered, &near);
        tag.set_if_neq(node_tag(disc.kind, alpha, highlight));
        let scale = disc_scale(disc.kind, highlight == Highlight::Focused && filter > 0.0)
            .with_y(transform.scale.y);
        if transform.scale != scale {
            transform.scale = scale;
        }
    }
}

/// Spawns the discs of the nodes the app was started with. An exclusive
/// system so that the time it logs is the whole spawn, not only the
/// queueing of commands.
fn spawn_startup_discs(world: &mut World) {
    let Some(Nodes(nodes)) = world.remove_resource::<Nodes>() else {
        warn!("no nodes to draw");
        return;
    };
    let shown = world.get_resource::<Shown>().cloned().unwrap_or_default();
    let spans = current_spans(world);
    spawn_discs(world, nodes, &spans, &shown);
}

/// The spans of the time mode drawn, from the time model; none, which
/// draws every node as a disc at its place, without one.
pub fn current_spans(world: &World) -> Vec<Option<Span>> {
    let mode = world
        .get_resource::<TimeMode>()
        .copied()
        .unwrap_or_default();
    world
        .get_resource::<TimeModel>()
        .map(|model| time_axis::times(model, mode).spans.clone())
        .unwrap_or_default()
}

/// The shared meshes and material, made on first use.
pub fn disc_assets(world: &mut World) -> DiscAssets {
    if let Some(assets) = world.get_resource::<DiscAssets>() {
        return assets.clone();
    }
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let mut add = |profile| meshes.add(column_mesh(profile));
    let (solid, fade_both, fade_top, fade_bottom) = (
        add(Profile::Solid),
        add(Profile::FadeBoth),
        add(Profile::FadeTop),
        add(Profile::FadeBottom),
    );
    let assets = DiscAssets {
        solid,
        fade_both,
        fade_top,
        fade_bottom,
        material: world
            .resource_mut::<Assets<DiscMaterial>>()
            .add(DiscMaterial::default()),
    };
    world.insert_resource(assets.clone());
    assets
}

/// A node's transform and profile: at its place on the map, the middle of
/// its span on the time axis, its kind's radius across and its column's
/// height.
pub fn node_transform(kind: Kind, place: Vec3, span: Option<Span>) -> (Transform, Profile) {
    let (y, height, profile) = shape(span, place.y);
    let transform = Transform::from_translation(place.with_y(y))
        .with_scale(disc_scale(kind, false).with_y(height));
    (transform, profile)
}

/// Spawns a disc or a column for every node, in layout order, as `spans`
/// places it on the time axis, at the alpha `shown` gives it, and logs how
/// long that took. With no span for a node, it is a disc at its place.
pub fn spawn_discs(world: &mut World, nodes: Vec<Placed>, spans: &[Option<Span>], shown: &Shown) {
    let started = Instant::now();
    let assets = disc_assets(world);
    let shown = shown.clone();
    let count = nodes.len();
    let spans = spans.to_vec();
    world.spawn_batch(nodes.into_iter().enumerate().map(move |(index, node)| {
        let span = match spans.get(index) {
            Some(span) => *span,
            None => Some(time_axis::disc_at(node.position.y)),
        };
        disc_bundle(
            &assets,
            index,
            node.kind,
            node.position,
            span,
            shown_alpha(&shown, index),
        )
    }));
    info!(
        "spawned {count} discs in {:.1} ms",
        started.elapsed().as_secs_f64() * 1e3
    );
}

/// What every disc is made of.
type DiscBundle = (
    Disc,
    Mesh3d,
    MeshMaterial3d<DiscMaterial>,
    MeshTag,
    Transform,
);

/// A node's disc.
fn disc_bundle(
    assets: &DiscAssets,
    index: usize,
    kind: Kind,
    place: Vec3,
    span: Option<Span>,
    alpha: f32,
) -> DiscBundle {
    let (transform, profile) = node_transform(kind, place, span);
    (
        Disc { index, kind },
        Mesh3d(profile_mesh(assets, profile)),
        MeshMaterial3d(assets.material.clone()),
        disc_tag(kind, alpha),
        transform,
    )
}

/// Spawns the discs of nodes that joined the picture after it was drawn:
/// each by its index, kind, place and span. They are drawn whole until the
/// filter, which runs later in the update, gives them their alpha.
pub fn spawn_nodes(
    world: &mut World,
    assets: &DiscAssets,
    nodes: Vec<(usize, Kind, Vec3, Option<Span>)>,
) {
    let assets = assets.clone();
    world.spawn_batch(nodes.into_iter().map(move |(index, kind, place, span)| {
        disc_bundle(&assets, index, kind, place, span, 1.0)
    }));
}

/// Despawns every disc.
pub fn despawn_discs(world: &mut World) {
    let discs: Vec<Entity> = world
        .query_filtered::<Entity, With<Disc>>()
        .iter(world)
        .collect();
    for disc in discs {
        world.despawn(disc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{DIM_ALPHA, Mode};

    #[test]
    fn a_tag_packs_the_srgb_bytes_and_the_alpha_red_lowest() {
        let tag = pack_rgba(Color::srgb_u8(0x12, 0x34, 0x56), 1.0);
        assert_eq!(tag, 0xff56_3412);
        assert_eq!(
            pack_rgba(Color::srgb_u8(0x12, 0x34, 0x56), 0.0),
            0x0056_3412
        );
        assert_eq!(pack_rgba(Color::WHITE, 2.0) >> 24, 0xff, "clamped");
        assert_eq!(pack_rgba(Color::WHITE, -1.0) >> 24, 0, "clamped");
    }

    #[test]
    fn a_discs_alpha_is_a_fraction_of_its_kinds() {
        for kind in [Kind::Topic, Kind::Fact, Kind::Inference] {
            let colour = kind_colour(kind);
            let [r, g, b, a] = disc_tag(kind, 1.0).0.to_le_bytes();
            assert_eq!([r, g, b, a], colour.to_srgba().to_u8_array(), "{kind:?}");
            let dim = disc_tag(kind, DIM_ALPHA).0 >> 24;
            assert_eq!(dim, (colour.alpha() * DIM_ALPHA * 255.0).round() as u32);
            assert!(dim > 0, "a dimmed disc still shows");
            assert_eq!(disc_tag(kind, 0.0).0 >> 24, 0, "a hidden disc adds nothing");
            assert_eq!(
                disc_tag(kind, 0.0).0 & 0xff_ffff,
                disc_tag(kind, 1.0).0 & 0xff_ffff
            );
        }
    }

    #[test]
    fn the_largest_disc_is_a_topic_and_every_disc_is_one_height() {
        assert!(disc_radius(Kind::Topic) > disc_radius(Kind::Inference));
        assert!(disc_radius(Kind::Inference) > disc_radius(Kind::Fact));
        assert!((0.5..=2.0).contains(&DISC_HEIGHT));
    }

    #[test]
    fn each_profile_carries_its_fade_in_its_vertex_alphas() {
        for profile in [Profile::Solid, Profile::Disc, Profile::Instant] {
            assert!((0..=10).all(|i| profile_alpha(profile, i as f32 / 10.0) == 1.0));
        }
        assert!((profile_alpha(Profile::FadeBoth, 0.5) - 1.0).abs() < 1e-6);
        assert!((profile_alpha(Profile::FadeBoth, 0.0) - SMEAR_EDGE_ALPHA).abs() < 1e-6);
        assert!((profile_alpha(Profile::FadeBoth, 1.0) - SMEAR_EDGE_ALPHA).abs() < 1e-6);
        assert_eq!(
            (
                profile_alpha(Profile::FadeTop, 0.0),
                profile_alpha(Profile::FadeTop, 1.0)
            ),
            (1.0, 0.0)
        );
        assert_eq!(
            (
                profile_alpha(Profile::FadeBottom, 0.0),
                profile_alpha(Profile::FadeBottom, 1.0)
            ),
            (0.0, 1.0)
        );
        let top: Vec<f32> = (0..=20)
            .map(|i| profile_alpha(Profile::FadeTop, i as f32 / 20.0))
            .collect();
        assert!(top.windows(2).all(|w| w[1] <= w[0]), "fades only one way");

        for profile in [
            Profile::Solid,
            Profile::FadeBoth,
            Profile::FadeTop,
            Profile::FadeBottom,
        ] {
            let mesh = column_mesh(profile);
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("{profile:?} has positions");
            };
            let Some(bevy::mesh::VertexAttributeValues::Float32x4(colours)) =
                mesh.attribute(Mesh::ATTRIBUTE_COLOR)
            else {
                panic!("{profile:?} has colours");
            };
            assert_eq!(positions.len(), colours.len());
            let heights = positions.iter().map(|p| p[1]);
            let (lo, hi) = heights.fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
            assert!(
                (lo + 0.5).abs() < 1e-6 && (hi - 0.5).abs() < 1e-6,
                "one unit high"
            );
            for (p, c) in positions.iter().zip(colours) {
                assert_eq!(c[3], profile_alpha(profile, p[1] + 0.5), "{profile:?}");
            }
        }
        assert_eq!(
            column_mesh(Profile::Solid)
                .attribute(Mesh::ATTRIBUTE_COLOR)
                .unwrap()
                .len(),
            column_mesh(Profile::Disc)
                .attribute(Mesh::ATTRIBUTE_COLOR)
                .unwrap()
                .len()
        );
    }

    #[test]
    fn a_column_is_drawn_over_its_span_and_never_thinner_than_its_profile_allows() {
        let column = Span {
            lo: 10.0,
            hi: 50.0,
            profile: Profile::Solid,
            clipped: Default::default(),
        };
        assert_eq!(shape(Some(column), 0.0), (30.0, 40.0, Profile::Solid));
        let day = Span {
            lo: 10.0,
            hi: 10.01,
            profile: Profile::Solid,
            clipped: Default::default(),
        };
        assert_eq!(
            shape(Some(day), 0.0).1,
            DISC_HEIGHT,
            "a short interval is as tall as a disc"
        );
        let instant = Span {
            lo: 7.0,
            hi: 7.0,
            profile: Profile::Instant,
            clipped: Default::default(),
        };
        assert_eq!(
            shape(Some(instant), 0.0),
            (7.0, INSTANT_HEIGHT, Profile::Instant)
        );
        assert_eq!(
            shape(None, 12.0),
            (12.0, DISC_HEIGHT, Profile::Disc),
            "no position"
        );
        let (transform, profile) =
            node_transform(Kind::Fact, Vec3::new(3.0, 99.0, 4.0), Some(column));
        assert_eq!(transform.translation, Vec3::new(3.0, 30.0, 4.0));
        assert_eq!(
            transform.scale,
            Vec3::new(disc_radius(Kind::Fact), 40.0, disc_radius(Kind::Fact))
        );
        assert_eq!(profile, Profile::Solid);
    }

    /// The alpha system over three discs, a topic, a fact and an inference,
    /// at the origin, at x = 10 and at the focus's reach along x.
    fn alpha_app() -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Shown::default())
            .init_resource::<Focus>()
            .init_resource::<Hovered>()
            .init_resource::<GraphIndex>()
            .add_systems(PostUpdate, apply_alphas);
        let kinds = [Kind::Topic, Kind::Fact, Kind::Inference];
        let xs = [0.0, 10.0, focus::FOCUS_REACH];
        let discs: Vec<Entity> = kinds
            .iter()
            .zip(xs)
            .enumerate()
            .map(|(index, (&kind, x))| {
                app.world_mut()
                    .spawn((
                        Disc { index, kind },
                        disc_tag(kind, 1.0),
                        Transform::from_xyz(x, 0.0, 0.0).with_scale(disc_scale(kind, false)),
                    ))
                    .id()
            })
            .collect();
        (app, discs)
    }

    #[test]
    fn alphas_follow_shown_and_are_written_only_when_it_changes() {
        let (mut app, discs) = alpha_app();
        let tag = |app: &App, i: usize| app.world().get::<MeshTag>(discs[i]).unwrap().0;
        app.update();
        assert_eq!(tag(&app, 1), disc_tag(Kind::Fact, 1.0).0);

        app.insert_resource(Shown {
            mode: Mode::Dim,
            passes: vec![true, false, true],
            visible: vec![true; 3],
        });
        app.update();
        assert_eq!(tag(&app, 0), disc_tag(Kind::Topic, 1.0).0);
        assert_eq!(tag(&app, 1), disc_tag(Kind::Fact, DIM_ALPHA).0);

        // Nothing rewrites a tag while Shown stands still.
        app.world_mut().entity_mut(discs[2]).insert(MeshTag(7));
        app.update();
        assert_eq!(tag(&app, 2), 7);

        app.world_mut().resource_mut::<Shown>().mode = Mode::Hide;
        app.update();
        assert_eq!(tag(&app, 1), disc_tag(Kind::Fact, 0.0).0);
        assert_eq!(tag(&app, 2), disc_tag(Kind::Inference, 1.0).0);
    }

    #[test]
    fn a_focus_fades_with_distance_and_lights_and_widens_its_disc() {
        let (mut app, discs) = alpha_app();
        let tag = |app: &App, i: usize| app.world().get::<MeshTag>(discs[i]).unwrap().0;
        let scale = |app: &App, i: usize| app.world().get::<Transform>(discs[i]).unwrap().scale;
        app.update();
        app.insert_resource(Focus(Some(0)))
            .insert_resource(Hovered(Some(1)));
        app.update();
        assert_eq!(
            tag(&app, 0),
            node_tag(Kind::Topic, 1.0, Highlight::Focused).0
        );
        assert_eq!(scale(&app, 0), disc_scale(Kind::Topic, true));
        assert_eq!(
            tag(&app, 1),
            node_tag(Kind::Fact, 1.0, Highlight::Hovered).0
        );
        assert_eq!(scale(&app, 1), disc_scale(Kind::Fact, false));
        assert_eq!(
            tag(&app, 2),
            disc_tag(Kind::Inference, focus::FAR_ALPHA).0,
            "at the reach, faded to the far alpha"
        );

        // Without the hover, the fact near the focus is nearly whole.
        app.insert_resource(Hovered(None));
        app.update();
        let near = focus::focus_falloff(10.0);
        assert!(near > focus::FAR_ALPHA && near < 1.0);
        assert_eq!(tag(&app, 1), disc_tag(Kind::Fact, near).0);

        // Cleared, every disc is as the filter shows it, at its own size.
        app.insert_resource(Focus(None));
        app.update();
        for (i, kind) in [Kind::Topic, Kind::Fact, Kind::Inference]
            .into_iter()
            .enumerate()
        {
            assert_eq!(tag(&app, i), disc_tag(kind, 1.0).0, "{kind:?}");
            assert_eq!(scale(&app, i), disc_scale(kind, false));
        }
    }

    #[test]
    fn the_far_ends_of_the_focused_and_hovered_nodes_edges_brighten() {
        use crate::graph_index::tests::edge;
        let (mut app, discs) = alpha_app();
        let tag = |app: &App, i: usize| app.world().get::<MeshTag>(discs[i]).unwrap().0;
        let by_id = ["t", "f", "i"]
            .iter()
            .enumerate()
            .map(|(i, &n)| (n.to_owned(), i))
            .collect();
        let edges = crate::edge_index::build(&[edge("f", "i", "supports")], &by_id, 3);
        app.insert_resource(GraphIndex {
            edges,
            ..GraphIndex::default()
        });
        app.update();
        app.insert_resource(Hovered(Some(1)));
        app.update();
        assert_eq!(
            tag(&app, 2),
            node_tag(Kind::Inference, 1.0, Highlight::NearHovered).0
        );
        assert_eq!(tag(&app, 0), disc_tag(Kind::Topic, 1.0).0, "no edge to it");
        app.insert_resource(Focus(Some(1)))
            .insert_resource(Hovered(None));
        app.update();
        let falloff = focus::focus_falloff(focus::FOCUS_REACH - 10.0);
        assert_eq!(
            tag(&app, 2),
            node_tag(Kind::Inference, falloff, Highlight::NearFocused).0
        );
    }

    #[test]
    fn a_near_disc_is_a_little_lighter_and_stronger_and_less_than_a_lit_one() {
        for kind in [Kind::Topic, Kind::Fact, Kind::Inference] {
            let bytes = |highlight| node_tag(kind, 0.5, highlight).0.to_le_bytes();
            let sum = |b: [u8; 4]| b[..3].iter().map(|&c| u32::from(c)).sum::<u32>();
            let order = [
                Highlight::Plain,
                Highlight::NearHovered,
                Highlight::NearFocused,
                Highlight::Hovered,
            ];
            for pair in order.windows(2) {
                let (low, high) = (bytes(pair[0]), bytes(pair[1]));
                assert!(sum(low) <= sum(high), "{kind:?} {pair:?}");
                assert!(low[3] < high[3], "{kind:?} {pair:?}");
            }
            assert_eq!(node_tag(kind, 0.0, Highlight::NearFocused).0 >> 24, 0);
        }
    }

    #[test]
    fn a_lit_disc_is_whole_and_lighter_unless_it_is_hidden() {
        for kind in [Kind::Topic, Kind::Fact, Kind::Inference] {
            let plain = disc_tag(kind, DIM_ALPHA).0.to_le_bytes();
            let hovered = node_tag(kind, DIM_ALPHA, Highlight::Hovered)
                .0
                .to_le_bytes();
            let focused = node_tag(kind, DIM_ALPHA, Highlight::Focused)
                .0
                .to_le_bytes();
            assert_eq!(hovered[3], 255, "{kind:?}");
            assert_eq!(focused[3], 255);
            let sum = |b: [u8; 4]| b[..3].iter().map(|&c| u32::from(c)).sum::<u32>();
            assert!(sum(plain) < sum(hovered) && sum(hovered) < sum(focused));
            assert_eq!(
                node_tag(kind, 0.0, Highlight::Focused).0 >> 24,
                0,
                "hidden stays hidden"
            );
            assert_eq!(node_tag(kind, 0.5, Highlight::Plain), disc_tag(kind, 0.5));
        }
        assert!(disc_scale(Kind::Fact, true).x > disc_scale(Kind::Fact, false).x);
        assert_eq!(disc_scale(Kind::Fact, true).y, 1.0);
    }
}
