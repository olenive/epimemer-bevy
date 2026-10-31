//! Labels (DESIGN.md §9): a budget of names, not a name for every node.
//!
//! In each update in which the focus, the hover, the camera, the viewport,
//! the bar's height, the side panel's width or what the filter shows
//! changed, every visible node whose disc is in front of the camera and
//! inside the part of the viewport below the bar and left of the side
//! panel is scored ([`score`]): by its distance from the focused node, or
//! from the point the camera looks at when nothing is focused, as
//! a share of how much of the scene the view spans there; by its kind,
//! topics before inferences before facts; by its degree; and a little by
//! its confidence where it carries one. A node the filter dims ranks after
//! every node that passes. The budget, [`budget`], is one label per
//! [`PIXELS_PER_LABEL`] square logical pixels of the viewport, between a
//! floor and a ceiling. [`choose`] walks the ranking and accepts a label
//! unless its rectangle, estimated from the text's length and the font size,
//! overlaps one already accepted. A label may stand in any of four places
//! beside its disc, to the right, to the left, above or below, and takes the
//! first that overlaps nothing, so that a dense cluster of discs can still
//! carry several labels; a place that would run under the bar, under the
//! side panel or off the viewport is not offered. The focused and the hovered node are
//! accepted first, whatever they overlap, and topic names have a reserved
//! share of the budget ([`TOPIC_SHARE`]) so that the map's place names are
//! always there to read. The accepted labels fade with their rank
//! ([`fade`]), so the last of them are faint.
//!
//! A dense neighbourhood fills the budget close to the focus and a sparse
//! one lets it reach further, with no radius to tune: the "sphere" of the
//! design comes from the ranking and the overlap rejection together.
//!
//! The labels are Bevy UI text nodes in a layer under the bar, placed
//! beside the disc at `Camera::world_to_viewport`, each on a plate of the
//! background colour that keeps it readable over bright discs. They are a
//! pool: a node that keeps its label keeps its text entity, an entity whose
//! node lost its label takes the next new one, and an entity with nothing
//! to show is hidden, so an update spawns an entity only when more labels
//! are shown than ever before and never despawns one. A label is a topic's
//! name or a claim's text, cut to its first [`LABEL_CHARS`] characters with
//! an ellipsis: a topic's name is cut too, since half the development
//! graph's are longer than that. The labels take no part in picking.

use std::collections::HashSet;

use bevy::camera::CameraUpdateSystems;
use bevy::prelude::*;
use bevy::ui::UiSystems;

use crate::camera::{BarCover, FlyCamera, PanelCover, Places, look_at, orthographic_height};
use crate::filter::{RefreshShown, Shown, is_visible};
use crate::focus::{Focus, Hovered};
use crate::graph_index::GraphIndex;
use crate::layout::Kind;
use crate::pickers::short_id;
use crate::presets::FOV;
use crate::scene;

/// Square logical pixels of viewport per label.
pub const PIXELS_PER_LABEL: f32 = 40_000.0;
/// The fewest and the most labels a viewport gets.
pub const MIN_LABELS: usize = 8;
pub const MAX_LABELS: usize = 120;
/// The share of the budget kept for topic names.
pub const TOPIC_SHARE: f32 = 0.4;
/// The alpha of the last label accepted; the first is whole.
pub const FADE_FLOOR: f32 = 0.5;
/// How many characters of a claim a label shows.
pub const LABEL_CHARS: usize = 40;
/// The font size of a topic's label and of a claim's, in logical pixels.
pub const TOPIC_FONT: f32 = 14.0;
pub const CLAIM_FONT: f32 = 12.0;
/// A label's line height, as a multiple of its font size, and the room
/// added around its estimated rectangle, in logical pixels, so that two
/// accepted labels never touch.
pub const LINE_HEIGHT: f32 = 1.25;
pub const LABEL_PAD: f32 = 3.0;
/// How far from its disc's centre a label's text starts, in logical
/// pixels.
pub const LABEL_GAP: f32 = 7.0;
/// How many places beside its disc a label may take.
pub const PLACES: usize = 4;
/// The distance, as a share of the view's height at the point ranked
/// from, at which a node's closeness has halved.
pub const DISTANCE_SCALE: f32 = 0.15;
/// What a kind adds to a node's score.
pub const TOPIC_WEIGHT: f32 = 0.15;
pub const INFERENCE_WEIGHT: f32 = 0.07;
/// What degree adds, at most, and the degree at which it adds half that.
pub const DEGREE_WEIGHT: f32 = 0.12;
pub const DEGREE_HALF: f32 = 20.0;
/// What confidence adds, at most, and the confidence taken for a node that
/// carries none, so that a missing value neither helps nor hurts.
pub const CONFIDENCE_WEIGHT: f32 = 0.05;
pub const NEUTRAL_CONFIDENCE: f32 = 0.5;
/// What a node the filter dims loses: more than any node can score, so
/// that every node that passes ranks first.
pub const DIMMED_PENALTY: f32 = 10.0;

/// What a node's score is made of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Traits {
    pub kind: Kind,
    /// The distance from the point ranked from, as a share of the view's
    /// height there.
    pub distance: f32,
    pub degree: u32,
    pub confidence: Option<f32>,
    /// Whether the node passes the filter.
    pub passes: bool,
}

/// A node that could be labelled: its score and where its label could go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub node: usize,
    pub kind: Kind,
    pub score: f32,
    /// The rectangles the label may take, in logical viewport pixels, with
    /// [`LABEL_PAD`] around the text, in the order they are tried; `None`
    /// for a place that is not offered.
    pub places: [Option<Rect>; PLACES],
}

/// A label to show: the node, its rectangle's top left corner in logical
/// viewport pixels, and its alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LabelPlacement {
    pub node: usize,
    pub kind: Kind,
    pub at: Vec2,
    pub alpha: f32,
}

/// The labels to show, best first.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Labels(pub Vec<LabelPlacement>);

/// Every node's label text and the size its rectangle is estimated at,
/// in node order, and the id and content each was made from, so that a
/// change to the graph makes again only the labels whose node changed.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct LabelTexts {
    pub texts: Vec<String>,
    pub sizes: Vec<Vec2>,
    pub made_from: Vec<(String, String)>,
}

/// The layer the labels are drawn in.
#[derive(Component, Debug)]
pub struct LabelLayer;

/// A text entity of the pool: its place in the pool, and the node it
/// labels, if any.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelSlot {
    pub place: usize,
    pub node: Option<usize>,
}

/// The labels of DESIGN.md §9.
pub struct LabelPlugin;

impl Plugin for LabelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Labels>()
            .init_resource::<LabelTexts>()
            .init_resource::<Focus>()
            .init_resource::<Hovered>()
            .init_resource::<BarCover>()
            .add_systems(Startup, spawn_layer)
            .add_systems(
                PostUpdate,
                (refresh_texts, choose_labels, show_labels)
                    .chain()
                    .after(CameraUpdateSystems)
                    .after(RefreshShown)
                    .before(UiSystems::Prepare),
            );
    }
}

/// The label budget for a viewport this size in logical pixels.
pub fn budget(viewport: Vec2) -> usize {
    let area = viewport.x.max(0.0) * viewport.y.max(0.0);
    ((area / PIXELS_PER_LABEL).round() as usize).clamp(MIN_LABELS, MAX_LABELS)
}

/// How many labels of a budget are kept for topics.
pub fn topic_reserve(budget: usize) -> usize {
    (budget as f32 * TOPIC_SHARE).round() as usize
}

/// A node's score: the higher, the sooner it is labelled. Closeness counts
/// most, from 1 at the point ranked from down toward 0, halving at
/// [`DISTANCE_SCALE`]; then the kind, the degree and the confidence.
pub fn score(traits: &Traits) -> f32 {
    let distance = if traits.distance.is_nan() {
        f32::INFINITY
    } else {
        traits.distance.max(0.0)
    };
    let closeness = 1.0 / (1.0 + (distance / DISTANCE_SCALE).powi(2));
    let kind = match traits.kind {
        Kind::Topic => TOPIC_WEIGHT,
        Kind::Inference => INFERENCE_WEIGHT,
        Kind::Fact => 0.0,
    };
    let degree = traits.degree as f32;
    let degree = DEGREE_WEIGHT * degree / (degree + DEGREE_HALF);
    let confidence = CONFIDENCE_WEIGHT
        * traits
            .confidence
            .unwrap_or(NEUTRAL_CONFIDENCE)
            .clamp(0.0, 1.0);
    let penalty = if traits.passes { 0.0 } else { DIMMED_PENALTY };
    closeness + kind + degree + confidence - penalty
}

/// The candidates best first, ties by node index, so a ranking does not
/// depend on the order they came in.
pub fn rank(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.node.cmp(&b.node)));
    candidates
}

/// Whether two rectangles overlap; touching edges do not.
pub fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
}

/// The first of a candidate's places that overlaps no accepted label.
fn free_place(candidate: &Candidate, accepted: &[(usize, Rect)]) -> Option<Rect> {
    candidate
        .places
        .iter()
        .flatten()
        .copied()
        .find(|rect| accepted.iter().all(|(_, other)| !overlaps(rect, other)))
}

/// The labels to show from candidates ranked best first, each as its
/// position in `ranked` and the rectangle it takes, in the order they rank.
/// The nodes in `pinned` come first, whatever they overlap and however full
/// the budget, each in its first place that is free of the others, or its
/// first place. Then one walk down the ranking accepts each label that has
/// a place overlapping none accepted, until there are `budget`, taking
/// claims only while they leave `reserve` places for topics, so that a
/// better-ranked label always wins an overlap and topics keep their share.
/// Where too few topics fit to fill their share, a second walk fills it
/// with claims. A pinned node that is not a candidate gets no label.
pub fn choose(
    ranked: &[Candidate],
    budget: usize,
    reserve: usize,
    pinned: &[usize],
) -> Vec<(usize, Rect)> {
    let mut accepted: Vec<(usize, Rect)> = Vec::new();
    let taken = |accepted: &[(usize, Rect)], at: usize| accepted.iter().any(|&(a, _)| a == at);
    for &node in pinned {
        let Some(at) = ranked.iter().position(|c| c.node == node) else {
            continue;
        };
        let candidate = &ranked[at];
        let place = free_place(candidate, &accepted)
            .or_else(|| candidate.places.iter().flatten().next().copied());
        if let Some(rect) = place
            && !taken(&accepted, at)
        {
            accepted.push((at, rect));
        }
    }
    let pinned_count = accepted.len();
    let limit = budget.max(pinned_count);
    let claim_places = limit - reserve.min(limit);
    let is_claim = |candidate: &Candidate| candidate.kind != Kind::Topic;
    let mut claims = accepted
        .iter()
        .filter(|&&(at, _)| is_claim(&ranked[at]))
        .count();
    // The first walk keeps the topics' share; the second fills what topics
    // could not.
    for keep_share in [true, false] {
        for (at, candidate) in ranked.iter().enumerate() {
            if accepted.len() >= limit {
                break;
            }
            let claim = is_claim(candidate);
            if (keep_share && claim && claims >= claim_places) || taken(&accepted, at) {
                continue;
            }
            if let Some(rect) = free_place(candidate, &accepted) {
                accepted.push((at, rect));
                claims += usize::from(claim);
            }
        }
    }
    accepted[pinned_count..].sort_unstable_by_key(|&(at, _)| at);
    accepted
}

/// The alpha of the label at this position among `count` accepted: whole
/// for the first, falling evenly to [`FADE_FLOOR`] for the last.
pub fn fade(position: usize, count: usize) -> f32 {
    if count <= 1 {
        return 1.0;
    }
    let t = position.min(count - 1) as f32 / (count - 1) as f32;
    1.0 - (1.0 - FADE_FLOOR) * t
}

/// A label's text: the content with its runs of white space made single
/// spaces, cut to [`LABEL_CHARS`] characters with an ellipsis, or the
/// node's short id when the content is blank.
pub fn label_text(content: &str, id: &str) -> String {
    let words = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.is_empty() {
        return short_id(id).to_owned();
    }
    if words.chars().count() <= LABEL_CHARS {
        return words;
    }
    let cut: String = words.chars().take(LABEL_CHARS).collect();
    format!("{}\u{2026}", cut.trim_end())
}

/// A character's advance as a share of the font size, an estimate a little
/// over Inter's own: narrow letters and punctuation, wide letters, capitals,
/// digits, the wide scripts, and the rest.
fn advance(c: char) -> f32 {
    match c {
        ' ' | 'i' | 'l' | 'j' | 'I' | 'f' | 't' | 'r' | '.' | ',' | ':' | ';' | '\'' | '!'
        | '|' | '(' | ')' | '[' | ']' | '-' => 0.36,
        'm' | 'w' | 'M' | 'W' | '\u{2026}' | '@' | '%' | '&' => 0.95,
        c if c >= '\u{2E80}' => 1.05,
        c if c.is_uppercase() => 0.74,
        c if c.is_ascii_digit() => 0.64,
        _ => 0.6,
    }
}

/// The estimated width of a text in a font this size, in logical pixels.
pub fn text_width(text: &str, font_size: f32) -> f32 {
    text.chars().map(advance).sum::<f32>() * font_size
}

/// The font size of a kind's label.
pub fn font_size(kind: Kind) -> f32 {
    match kind {
        Kind::Topic => TOPIC_FONT,
        Kind::Fact | Kind::Inference => CLAIM_FONT,
    }
}

/// A label's estimated rectangle size, padding included.
pub fn label_size(text: &str, kind: Kind) -> Vec2 {
    let size = font_size(kind);
    Vec2::new(text_width(text, size), size * LINE_HEIGHT) + Vec2::splat(2.0 * LABEL_PAD)
}

/// The rectangles a label of this size may take for a disc at this
/// viewport point, in the order they are tried: its text starting
/// [`LABEL_GAP`] to the right of the disc's centre, or ending as far to its
/// left, each centred on the disc vertically; then centred across the
/// disc, in the row above those two or the row below. No two of the places
/// overlap.
pub fn label_places(disc: Vec2, size: Vec2) -> [Rect; PLACES] {
    let inset = LABEL_GAP - LABEL_PAD;
    let middle = disc.y - size.y / 2.0;
    let centre = disc.x - size.x / 2.0;
    [
        Vec2::new(disc.x + inset, middle),
        Vec2::new(disc.x - inset - size.x, middle),
        Vec2::new(centre, middle - size.y),
        Vec2::new(centre, middle + size.y),
    ]
    .map(|min| Rect::from_corners(min, min + size))
}

/// Whether one rectangle lies wholly inside another.
fn inside(rect: &Rect, bounds: &Rect) -> bool {
    rect.min.cmpge(bounds.min).all() && rect.max.cmple(bounds.max).all()
}

/// The distance between two points as the view shows it: straight, in a
/// perspective view; across the view only, in an orthographic one, where
/// nearer and further look the same. `forward` is the view's direction.
pub fn view_distance(a: Vec3, b: Vec3, forward: Vec3, orthographic: bool) -> f32 {
    let d = a - b;
    if orthographic {
        (d - forward * d.dot(forward)).length()
    } else {
        d.length()
    }
}

/// A label's text colour: a light tint of its kind.
pub fn label_colour(kind: Kind) -> Color {
    match kind {
        Kind::Topic => Color::srgb(1.0, 0.87, 0.64),
        Kind::Fact => Color::srgb(0.8, 0.9, 1.0),
        Kind::Inference => Color::srgb(1.0, 0.8, 0.92),
    }
}

/// The shadow under a label's text and the plate behind it, which keep it
/// readable over bright discs. The plate is the scene's background, partly
/// clear so that the discs beneath still show, and as strong behind a faint
/// label as behind a whole one, since a faint text needs it most.
fn shadow_colour(alpha: f32) -> Color {
    Color::srgba(0.0, 0.0, 0.0, 0.85 * alpha)
}

/// How opaque a label's plate is.
const PLATE_ALPHA: f32 = 0.65;

/// A label's node: placed absolutely, its text inset by [`LABEL_PAD`], so
/// that the node covers the rectangle its size was estimated for.
fn label_node(at: Vec2) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(at.x),
        top: px(at.y),
        padding: UiRect::all(px(LABEL_PAD)),
        border_radius: BorderRadius::all(px(3)),
        ..default()
    }
}

/// Makes again the label of every node whose id or content differs from
/// what its label was made from, and of every node new since; whether any
/// changed. A node arriving on the event stream, or a claim whose content
/// changed, costs its own label, not every label.
pub fn update_texts(texts: &mut LabelTexts, index: &GraphIndex) -> bool {
    let count = index.ids.len();
    let mut changed = texts.texts.len() != count;
    texts.texts.truncate(count);
    texts.sizes.truncate(count);
    texts.made_from.truncate(count);
    for i in 0..count {
        let (id, content, kind) = (&index.ids[i], &index.names[i], index.kinds[i]);
        let same = texts
            .made_from
            .get(i)
            .is_some_and(|(made_id, made_content)| made_id == id && made_content == content);
        if same {
            continue;
        }
        let text = label_text(content, id);
        let size = label_size(&text, kind);
        let source = (id.clone(), content.clone());
        if i < texts.texts.len() {
            texts.texts[i] = text;
            texts.sizes[i] = size;
            texts.made_from[i] = source;
        } else {
            texts.texts.push(text);
            texts.sizes.push(size);
            texts.made_from.push(source);
        }
        changed = true;
    }
    changed
}

/// Which node each slot of the pool shows next: a slot keeps its node if
/// that node is still wanted, a wanted node not yet shown takes the first
/// free slot, and slots are added at the end when none is free. The result
/// is at least as long as `slots`.
pub fn assign_slots(slots: &[Option<usize>], wanted: &[usize]) -> Vec<Option<usize>> {
    let wanted_set: HashSet<usize> = wanted.iter().copied().collect();
    let mut next: Vec<Option<usize>> = slots
        .iter()
        .map(|slot| slot.filter(|node| wanted_set.contains(node)))
        .collect();
    let shown: HashSet<usize> = next.iter().flatten().copied().collect();
    let mut free = (0..next.len())
        .filter(|&i| next[i].is_none())
        .collect::<Vec<_>>()
        .into_iter();
    for &node in wanted.iter().filter(|node| !shown.contains(node)) {
        match free.next() {
            Some(i) => next[i] = Some(node),
            None => next.push(Some(node)),
        }
    }
    next
}

/// The layer, under the bar, that the pool's text entities are drawn in.
fn spawn_layer(mut commands: Commands) {
    commands.spawn((
        LabelLayer,
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            top: px(0),
            width: percent(100),
            height: percent(100),
            ..default()
        },
        GlobalZIndex(1),
        Pickable::IGNORE,
    ));
}

/// Makes the labels of the nodes that changed again when the graph
/// changes, touching the texts only where one did.
fn refresh_texts(index: Res<GraphIndex>, mut texts: ResMut<LabelTexts>) {
    if index.is_changed() && update_texts(texts.bypass_change_detection(), &index) {
        texts.set_changed();
    }
}

/// What the camera is, for the labels.
type CameraView<'a> = (
    &'a Camera,
    Ref<'a, Transform>,
    Ref<'a, Projection>,
    &'a FlyCamera,
);

/// What choosing the labels reads.
#[derive(bevy::ecs::system::SystemParam)]
struct Scene<'w> {
    index: Res<'w, GraphIndex>,
    texts: Res<'w, LabelTexts>,
    places: Res<'w, Places>,
    shown: Res<'w, Shown>,
    focus: Res<'w, Focus>,
    hovered: Res<'w, Hovered>,
    cover: Res<'w, BarCover>,
    panel: Option<Res<'w, PanelCover>>,
}

/// Whether anything the labels are chosen from, other than the camera,
/// changed since they were last chosen.
fn scene_changed(scene: &Scene) -> bool {
    scene.texts.is_changed()
        || scene.places.is_changed()
        || scene.shown.is_changed()
        || scene.focus.is_changed()
        || scene.hovered.is_changed()
        || scene.cover.is_changed()
        || scene.panel.as_ref().is_some_and(|panel| panel.is_changed())
}

/// Chooses the labels, in any update in which what they depend on changed.
fn choose_labels(
    camera: Single<CameraView>,
    scene: Scene,
    mut labels: ResMut<Labels>,
    mut last_viewport: Local<Option<Rect>>,
    mut logged: Local<bool>,
) {
    let (camera, transform, projection, fly) = camera.into_inner();
    let Some(viewport) = camera.logical_viewport_rect() else {
        return;
    };
    let resized = *last_viewport != Some(viewport);
    if !(resized || transform.is_changed() || projection.is_changed() || scene_changed(&scene)) {
        return;
    }
    *last_viewport = Some(viewport);
    let budget = budget(viewport.size());
    if !*logged {
        *logged = true;
        info!(
            "labels: a budget of {budget} for a {:.0} by {:.0} viewport, {} kept for topics",
            viewport.width(),
            viewport.height(),
            topic_reserve(budget)
        );
    }

    let ortho = orthographic_height(&projection);
    let forward = *transform.forward();
    let anchor = scene
        .focus
        .0
        .and_then(|i| scene.places.points.get(i).copied())
        .unwrap_or_else(|| look_at(&transform, fly));
    let reach = ortho.unwrap_or_else(|| {
        let depth = (anchor - transform.translation).dot(forward).max(1.0);
        2.0 * depth * (FOV / 2.0).tan()
    });
    let global = GlobalTransform::from(*transform);
    // The part of the viewport below the bar and left of the side panel,
    // where a label can be read.
    let panel = scene.panel.as_ref().map_or(0.0, |panel| panel.0);
    let clear = Rect::from_corners(
        Vec2::new(
            viewport.min.x,
            viewport.min.y + scene.cover.0.unwrap_or(0.0) * viewport.height(),
        ),
        Vec2::new(viewport.max.x - panel * viewport.width(), viewport.max.y),
    );

    let index = &scene.index;
    let candidates: Vec<Candidate> = (0..index.ids.len())
        .filter(|&i| is_visible(&scene.shown.visible, i))
        .filter_map(|i| {
            let point = *scene.places.points.get(i)?;
            let disc = camera.world_to_viewport(&global, point).ok()?;
            if !clear.contains(disc) {
                return None;
            }
            let places = label_places(disc, *scene.texts.sizes.get(i)?)
                .map(|rect| inside(&rect, &clear).then_some(rect));
            let traits = Traits {
                kind: index.kinds[i],
                distance: view_distance(point, anchor, forward, ortho.is_some()) / reach,
                degree: index.degrees.get(i).copied().unwrap_or(0),
                confidence: index.confidence.get(i).copied().flatten(),
                passes: scene.shown.passes.get(i).copied().unwrap_or(true),
            };
            Some(Candidate {
                node: i,
                kind: traits.kind,
                score: score(&traits),
                places,
            })
        })
        .collect();
    let ranked = rank(candidates);
    let pinned: Vec<usize> = [scene.focus.0, scene.hovered.0]
        .into_iter()
        .flatten()
        .collect();
    let accepted = choose(&ranked, budget, topic_reserve(budget), &pinned);
    let count = accepted.len();
    let next = accepted
        .iter()
        .enumerate()
        .map(|(position, &(at, rect))| {
            let candidate = &ranked[at];
            let alpha = if pinned.contains(&candidate.node) {
                1.0
            } else {
                fade(position, count)
            };
            LabelPlacement {
                node: candidate.node,
                kind: candidate.kind,
                at: rect.min,
                alpha,
            }
        })
        .collect();
    labels.set_if_neq(Labels(next));
}

/// What a slot of the pool shows and where.
type SlotView<'a> = (
    &'a mut LabelSlot,
    &'a mut Node,
    &'a mut Text,
    &'a mut TextFont,
    &'a mut TextColor,
    &'a mut TextShadow,
    &'a mut BackgroundColor,
);

/// Shows the chosen labels in the pool's text entities, adding entities
/// only when there are more labels than the pool holds.
fn show_labels(
    mut commands: Commands,
    labels: Res<Labels>,
    texts: Res<LabelTexts>,
    layer: Single<Entity, With<LabelLayer>>,
    mut slots: Query<SlotView>,
) {
    if !labels.is_changed() && !texts.is_changed() {
        return;
    }
    let mut pool: Vec<_> = slots.iter_mut().collect();
    pool.sort_by_key(|slot| slot.0.place);
    // After a graph switch an index names another node, so no slot keeps
    // what it showed.
    let current: Vec<Option<usize>> = pool
        .iter()
        .map(|slot| slot.0.node.filter(|_| !texts.is_changed()))
        .collect();
    let wanted: Vec<usize> = labels.0.iter().map(|label| label.node).collect();
    let next = assign_slots(&current, &wanted);
    let placement = |node: usize| labels.0.iter().find(|label| label.node == node);
    let words = |node: usize| texts.texts.get(node).cloned().unwrap_or_default();

    for (place, (mut slot, mut node, mut text, mut font, mut colour, mut shadow, mut plate)) in
        pool.into_iter().enumerate()
    {
        let shown = next[place].and_then(placement);
        let Some(label) = shown else {
            if slot.node.is_some() {
                slot.node = None;
            }
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        if slot.node != Some(label.node) || texts.is_changed() {
            slot.node = Some(label.node);
            text.0 = words(label.node);
            font.font_size = FontSize::Px(font_size(label.kind));
        }
        let (left, top) = (px(label.at.x), px(label.at.y));
        if node.display != Display::Flex || node.left != left || node.top != top {
            node.display = Display::Flex;
            node.left = left;
            node.top = top;
        }
        colour.set_if_neq(TextColor(label_colour(label.kind).with_alpha(label.alpha)));
        let wanted_shadow = shadow_colour(label.alpha);
        if shadow.color != wanted_shadow {
            shadow.color = wanted_shadow;
        }
        plate.set_if_neq(BackgroundColor(scene::BACKGROUND.with_alpha(PLATE_ALPHA)));
    }
    for (place, node) in next.iter().enumerate().skip(current.len()) {
        let Some(label) = node.and_then(placement) else {
            continue;
        };
        commands.spawn((
            LabelSlot {
                place,
                node: Some(label.node),
            },
            label_node(label.at),
            BackgroundColor(scene::BACKGROUND.with_alpha(PLATE_ALPHA)),
            Text::new(words(label.node)),
            TextFont {
                font_size: FontSize::Px(font_size(label.kind)),
                ..default()
            },
            TextColor(label_colour(label.kind).with_alpha(label.alpha)),
            TextShadow {
                offset: Vec2::splat(1.0),
                color: shadow_colour(label.alpha),
            },
            TextLayout::no_wrap(),
            Pickable::IGNORE,
            ChildOf(*layer),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn traits(kind: Kind, distance: f32) -> Traits {
        Traits {
            kind,
            distance,
            degree: 4,
            confidence: None,
            passes: true,
        }
    }

    /// A candidate whose label has one place, a 100 by 20 box at (x, y).
    fn at(node: usize, kind: Kind, score: f32, x: f32, y: f32) -> Candidate {
        Candidate {
            node,
            kind,
            score,
            places: [Some(Rect::new(x, y, x + 100.0, y + 20.0)), None, None, None],
        }
    }

    /// The positions chosen, without their rectangles.
    fn positions(chosen: &[(usize, Rect)]) -> Vec<usize> {
        chosen.iter().map(|&(at, _)| at).collect()
    }

    #[test]
    fn nearer_ranks_first_then_kind_then_degree_then_confidence() {
        let near = score(&traits(Kind::Fact, 0.05));
        let far = score(&traits(Kind::Fact, 0.6));
        assert!(near > far);
        assert!(score(&traits(Kind::Fact, 0.1)) > score(&traits(Kind::Fact, 0.2)));
        // At one distance: topics, then inferences, then facts.
        let [topic, inference, fact] =
            [Kind::Topic, Kind::Inference, Kind::Fact].map(|k| score(&traits(k, 0.3)));
        assert!(topic > inference && inference > fact);
        // A fact at the focus beats a topic far away.
        assert!(score(&traits(Kind::Fact, 0.0)) > score(&traits(Kind::Topic, 2.0)));
        let busy = Traits {
            degree: 60,
            ..traits(Kind::Fact, 0.3)
        };
        assert!(score(&busy) > fact);
        let sure = Traits {
            confidence: Some(0.95),
            ..traits(Kind::Fact, 0.3)
        };
        let doubtful = Traits {
            confidence: Some(0.1),
            ..traits(Kind::Fact, 0.3)
        };
        assert!(score(&sure) > fact && fact > score(&doubtful));
        assert!(
            score(&sure) - score(&doubtful) < score(&traits(Kind::Inference, 0.3)) - fact,
            "confidence weighs less than kind"
        );
        // A dimmed node ranks after every node that passes.
        let dimmed = Traits {
            passes: false,
            degree: 500,
            confidence: Some(1.0),
            ..traits(Kind::Topic, 0.0)
        };
        assert!(score(&dimmed) < score(&traits(Kind::Fact, 100.0)));
        assert!(score(&traits(Kind::Fact, f32::NAN)).is_finite());

        let ranked = rank(vec![
            at(3, Kind::Fact, 0.5, 0.0, 0.0),
            at(1, Kind::Fact, 0.9, 0.0, 0.0),
            at(2, Kind::Fact, 0.5, 0.0, 0.0),
        ]);
        let order: Vec<usize> = ranked.iter().map(|c| c.node).collect();
        assert_eq!(order, [1, 2, 3], "best first, a tie by node");
    }

    #[test]
    fn the_budget_follows_the_viewports_area_between_a_floor_and_a_ceiling() {
        assert_eq!(budget(Vec2::new(1280.0, 720.0)), 23);
        assert_eq!(budget(Vec2::new(1920.0, 1080.0)), 52);
        assert_eq!(budget(Vec2::new(200.0, 200.0)), MIN_LABELS);
        assert_eq!(budget(Vec2::ZERO), MIN_LABELS);
        assert_eq!(budget(Vec2::new(-5.0, 100.0)), MIN_LABELS);
        assert_eq!(budget(Vec2::new(8000.0, 8000.0)), MAX_LABELS);
        assert_eq!(budget(Vec2::new(f32::NAN, 720.0)), MIN_LABELS);
        assert_eq!(topic_reserve(23), 9);
        assert_eq!(topic_reserve(MIN_LABELS), 3);
    }

    #[test]
    fn a_label_overlapping_one_accepted_is_rejected() {
        let ranked = vec![
            at(0, Kind::Fact, 0.9, 0.0, 0.0),
            at(1, Kind::Fact, 0.8, 50.0, 10.0),  // over 0
            at(2, Kind::Fact, 0.7, 100.0, 0.0),  // touches 0's right edge
            at(3, Kind::Fact, 0.6, 0.0, 20.0),   // touches 0's bottom edge
            at(4, Kind::Fact, 0.5, 120.0, 15.0), // over 2
        ];
        let chosen = choose(&ranked, 10, 0, &[]);
        assert_eq!(positions(&chosen), [0, 2, 3]);
        for (i, (_, a)) in chosen.iter().enumerate() {
            for (_, b) in &chosen[i + 1..] {
                assert!(!overlaps(a, b));
            }
        }
        let rect = |i: usize| ranked[i].places[0].unwrap();
        assert!(overlaps(&rect(0), &rect(1)));
        assert!(!overlaps(&rect(0), &rect(2)));
        // The budget stops the walk.
        assert_eq!(positions(&choose(&ranked, 2, 0, &[])), [0, 2]);
        assert!(choose(&[], 5, 2, &[]).is_empty());
    }

    #[test]
    fn a_label_blocked_in_one_place_takes_the_next_that_is_free() {
        let size = Vec2::new(100.0, 20.0);
        let disc = Vec2::new(400.0, 300.0);
        let places = label_places(disc, size);
        let [right, left, above, below] = places;
        assert!(right.min.x > disc.x && left.max.x < disc.x);
        assert!(above.max.y < disc.y && below.min.y > disc.y);
        for (i, a) in places.iter().enumerate() {
            assert_eq!(a.size(), size);
            for b in &places[i + 1..] {
                assert!(!overlaps(a, b), "the four places are apart");
            }
        }
        let first = Candidate {
            node: 0,
            kind: Kind::Topic,
            score: 1.0,
            places: [Some(right), None, None, None],
        };
        let second = Candidate {
            node: 1,
            kind: Kind::Fact,
            score: 0.9,
            places: places.map(Some),
        };
        let chosen = choose(&[first, second], 5, 0, &[]);
        assert_eq!(chosen, [(0, right), (1, left)]);
        // A place not offered, as under the bar, is not taken.
        let second = Candidate {
            places: [Some(right), None, None, Some(below)],
            ..second
        };
        assert_eq!(
            choose(&[first, second], 5, 0, &[]),
            [(0, right), (1, below)]
        );
        let viewport = Rect::new(0.0, 0.0, 1280.0, 720.0);
        assert!(inside(&right, &viewport));
        assert!(!inside(&right.translate(Vec2::new(1000.0, 0.0)), &viewport));
    }

    #[test]
    fn topics_keep_their_share_of_the_budget() {
        // Six facts rank above three topics, every label apart.
        let mut ranked: Vec<Candidate> = (0..6)
            .map(|i| at(i, Kind::Fact, 1.0 - i as f32 * 0.01, 0.0, i as f32 * 30.0))
            .collect();
        ranked.extend((6..9).map(|i| at(i, Kind::Topic, 0.5, 300.0, i as f32 * 30.0)));
        let ranked = rank(ranked);
        let kinds = |chosen: &[(usize, Rect)]| -> (usize, usize) {
            let topics = chosen
                .iter()
                .filter(|&&(a, _)| ranked[a].kind == Kind::Topic)
                .count();
            (topics, chosen.len() - topics)
        };
        let chosen = choose(&ranked, 5, 2, &[]);
        assert_eq!(kinds(&chosen), (2, 3), "two of five kept for topics");
        assert_eq!(positions(&chosen), [0, 1, 2, 6, 7], "in rank order");
        assert_eq!(kinds(&choose(&ranked, 5, 0, &[])), (0, 5));
        assert_eq!(
            kinds(&choose(&ranked, 12, 4, &[])),
            (3, 6),
            "only three exist"
        );
        // A topic that overlaps a better-ranked label gives its place to the
        // next topic.
        let mut crowded = ranked.clone();
        let blocked = crowded.iter().position(|c| c.node == 6).unwrap();
        crowded[blocked].places = crowded[0].places;
        let chosen = choose(&crowded, 5, 2, &[]);
        assert!(!positions(&chosen).contains(&blocked));
        assert_eq!(kinds(&chosen), (2, 3));
    }

    #[test]
    fn the_focused_and_hovered_nodes_are_always_labelled() {
        let ranked = vec![
            at(0, Kind::Topic, 0.9, 0.0, 0.0),
            at(1, Kind::Fact, 0.8, 0.0, 40.0),
            at(2, Kind::Fact, 0.1, 10.0, 5.0), // over 0, ranked last
            at(3, Kind::Fact, 0.05, 20.0, 8.0), // over 0 and 2
        ];
        let chosen = positions(&choose(&ranked, 2, 1, &[3, 2]));
        assert_eq!(chosen, [3, 2], "first, whatever they overlap");
        let chosen = positions(&choose(&ranked, 3, 1, &[3, 2, 3]));
        assert_eq!(chosen, [3, 2, 1], "0 overlaps them; said twice is once");
        assert_eq!(
            positions(&choose(&ranked, 1, 0, &[9])),
            [0],
            "a node off the screen is no candidate"
        );
        assert_eq!(
            positions(&choose(&ranked, 0, 0, &[1, 0])),
            [1, 0],
            "beyond the budget"
        );
    }

    #[test]
    fn labels_fade_with_rank_to_the_floor() {
        assert_eq!(fade(0, 1), 1.0);
        assert_eq!(fade(0, 0), 1.0);
        assert_eq!(fade(0, 5), 1.0);
        assert!((fade(4, 5) - FADE_FLOOR).abs() < 1e-6);
        assert!((fade(2, 5) - (1.0 + FADE_FLOOR) / 2.0).abs() < 1e-6);
        assert!((fade(9, 5) - FADE_FLOOR).abs() < 1e-6, "past the end");
        let alphas: Vec<f32> = (0..23).map(|i| fade(i, 23)).collect();
        assert!(alphas.windows(2).all(|w| w[1] < w[0]));
    }

    #[test]
    fn a_label_is_a_name_or_the_start_of_a_claim() {
        assert_eq!(label_text("Swifts", "id"), "Swifts");
        assert_eq!(label_text("  two\n words ", "id"), "two words");
        assert_eq!(label_text("", "abcdef0123456789"), "abcdef01");
        let claim = "Common swifts spend most of their lives in the air, landing rarely.";
        let label = label_text(claim, "id");
        assert_eq!(label, "Common swifts spend most of their lives\u{2026}");
        assert_eq!(label.chars().count(), LABEL_CHARS);
        let exact: String = "é".repeat(LABEL_CHARS);
        assert_eq!(label_text(&exact, "id"), exact, "characters, not bytes");
        // A longer text is estimated wider, a capital wider than a small
        // letter, and the rectangle starts right of the disc.
        assert!(text_width("Mmmm", 12.0) > text_width("iiii", 12.0));
        assert!(text_width("ABC", 12.0) > text_width("abc", 12.0));
        let size = label_size("Swifts", Kind::Topic);
        assert!(size.x > text_width("Swifts", TOPIC_FONT));
        assert!(size.y > TOPIC_FONT);
        let [rect, ..] = label_places(Vec2::new(100.0, 50.0), size);
        assert!(rect.min.x > 100.0 && (rect.center().y - 50.0).abs() < 1e-4);
    }

    #[test]
    fn an_orthographic_view_measures_only_across_it() {
        let forward = Vec3::NEG_Y;
        let (a, b) = (Vec3::new(0.0, 100.0, 0.0), Vec3::new(3.0, 0.0, 4.0));
        assert!((view_distance(a, b, forward, true) - 5.0).abs() < 1e-4);
        assert!(
            (view_distance(a, b, forward, false) - (100.0f32.powi(2) + 25.0).sqrt()).abs() < 1e-3
        );
    }

    #[test]
    fn a_slot_keeps_its_node_and_a_freed_slot_takes_a_new_one() {
        assert_eq!(assign_slots(&[], &[4, 5]), [Some(4), Some(5)]);
        assert_eq!(
            assign_slots(&[Some(4), Some(5), None], &[6, 5]),
            [Some(6), Some(5), None]
        );
        assert_eq!(
            assign_slots(&[Some(1), None], &[2, 3, 1, 4]),
            [Some(1), Some(2), Some(3), Some(4)]
        );
        assert_eq!(assign_slots(&[Some(1), Some(2)], &[]), [None, None]);
    }

    /// The pool, with no window and no renderer.
    fn pool_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Labels>()
            .insert_resource(LabelTexts {
                texts: (0..10).map(|i| format!("node {i}")).collect(),
                sizes: vec![Vec2::new(60.0, 20.0); 10],
                made_from: Vec::new(),
            })
            .add_systems(Startup, spawn_layer)
            .add_systems(PostUpdate, show_labels);
        app.update();
        app
    }

    fn show(app: &mut App, nodes: &[usize]) {
        let labels = nodes
            .iter()
            .enumerate()
            .map(|(i, &node)| LabelPlacement {
                node,
                kind: Kind::Fact,
                at: Vec2::new(10.0 * i as f32, 30.0),
                alpha: fade(i, nodes.len()),
            })
            .collect();
        app.insert_resource(Labels(labels));
        app.update();
    }

    /// Every text entity, as (slot, text, shown).
    fn texts(app: &mut App) -> Vec<(Option<LabelSlot>, String, bool)> {
        let mut query = app
            .world_mut()
            .query::<(&Text, Option<&LabelSlot>, Option<&Node>, Option<&ChildOf>)>();
        let layer = app
            .world_mut()
            .query_filtered::<Entity, With<LabelLayer>>()
            .single(app.world())
            .unwrap();
        query
            .iter(app.world())
            .map(|(text, slot, node, parent)| {
                assert_eq!(parent.map(ChildOf::parent), Some(layer), "in the layer");
                let shown = node.is_some_and(|n| n.display != Display::None);
                (slot.copied(), text.0.clone(), shown)
            })
            .collect()
    }

    #[test]
    fn the_pool_reuses_its_entities_and_leaves_no_orphan_text() {
        let mut app = pool_app();
        show(&mut app, &[1, 2, 3]);
        assert_eq!(texts(&mut app).len(), 3);
        let place_of = |app: &mut App, node: usize| {
            texts(app)
                .into_iter()
                .find(|(slot, _, shown)| *shown && slot.unwrap().node == Some(node))
                .map(|(slot, text, _)| (slot.unwrap().place, text))
        };
        let two = place_of(&mut app, 2).unwrap();

        show(&mut app, &[2, 7]);
        let all = texts(&mut app);
        assert_eq!(all.len(), 3, "no entity spawned for 7, none despawned");
        assert_eq!(place_of(&mut app, 2), Some(two), "2 kept its entity");
        assert_eq!(
            place_of(&mut app, 7).map(|(_, t)| t).as_deref(),
            Some("node 7")
        );
        assert_eq!(all.iter().filter(|(_, _, shown)| *shown).count(), 2);

        show(&mut app, &[]);
        let all = texts(&mut app);
        assert_eq!(all.len(), 3);
        assert!(
            all.iter()
                .all(|(slot, _, shown)| !shown && slot.unwrap().node.is_none())
        );

        show(&mut app, &[0, 1, 2, 3, 4]);
        let all = texts(&mut app);
        assert_eq!(all.len(), 5, "grown to the most shown at once");
        let mut places: Vec<usize> = all.iter().map(|(s, _, _)| s.unwrap().place).collect();
        places.sort_unstable();
        assert_eq!(places, [0, 1, 2, 3, 4], "each place once");
        let mut nodes: Vec<usize> = all.iter().filter_map(|(s, _, _)| s.unwrap().node).collect();
        nodes.sort_unstable();
        assert_eq!(nodes, [0, 1, 2, 3, 4], "each node once");
        for (slot, text, shown) in &all {
            assert!(*shown);
            assert_eq!(*text, format!("node {}", slot.unwrap().node.unwrap()));
        }

        // After a graph switch every text is written again, even for a
        // node index that stays.
        app.insert_resource(LabelTexts {
            texts: (0..10).map(|i| format!("new {i}")).collect(),
            sizes: vec![Vec2::new(60.0, 20.0); 10],
            made_from: Vec::new(),
        });
        show(&mut app, &[0, 1]);
        let all = texts(&mut app);
        assert_eq!(all.len(), 5);
        let shown: Vec<&String> = all
            .iter()
            .filter(|(_, _, s)| *s)
            .map(|(_, t, _)| t)
            .collect();
        assert_eq!(shown.len(), 2);
        assert!(shown.iter().all(|t| t.starts_with("new ")));
    }
}
