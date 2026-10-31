//! Focus and hover (DESIGN.md §8): the node the user has chosen, and the
//! node under the pointer.
//!
//! A left click on a disc focuses its node. A left click on the scene where
//! no disc is clears the focus, and so does Escape, unless a list of the
//! bar is open, which Escape closes first, or a text field has the
//! keyboard. The disc under the pointer, the nearest where several are, is
//! hovered. Both are node indices in layout order, kept as the resources
//! [`Focus`] and [`Hovered`]: the scene brightens the two discs and fades
//! the rest with their distance from the focus (`scene.rs`), the edges of
//! both are drawn as lines and the nodes at their far ends brighten
//! (`edges.rs`), the labels always name both and rank the rest by their
//! distance from the focus (`labels.rs`), and `T` fits the focused node's
//! topic (`camera.rs`).
//!
//! Neither reaches through the bar or the side panel. Bevy's UI picking
//! ranks them above every disc and each of their nodes blocks what lies
//! beneath, and while the pointer is over either (`ui_focus.rs`) nothing
//! is hovered and a click changes no focus. A graph switch clears both
//! (`reload.rs`), since an index names a node of one graph.
//!
//! The focus can also be set from the side panel, by a click on a node it
//! names (`panel.rs`), and from the search field, by walking its hits
//! (`search.rs`). [`FocusHistory`] keeps the nodes focused before, by id,
//! and `B` or the panel's Back button returns to the one before, so that a
//! chain of supports or successions walked through the panel can be walked
//! back.

use std::collections::HashSet;

use bevy::input::InputSystems;
use bevy::math::Vec3;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;

use crate::bar::OpenPicker;
use crate::edge_index::{EdgeIndex, claim_neighbours};
use crate::graph_index::GraphIndex;
use crate::keys::{BACK_KEY, CLEAR_FOCUS_KEY};
use crate::scene::Disc;
use crate::ui_focus::UiFocus;

/// How many earlier foci [`FocusHistory`] keeps.
pub const HISTORY_LENGTH: usize = 32;

/// The share of its alpha a node far from the focus keeps: enough that the
/// shape of the whole graph stays in view.
pub const FAR_ALPHA: f32 = 0.35;
/// How far from the focus, in world units as [`focus_distance`] measures
/// them, a node has faded to [`FAR_ALPHA`]. Tuned on the development graph,
/// whose topics stood 3 units apart on the circle and now about 8 from
/// their nearest neighbour on the topic map, with claims reaching up to 13
/// units around a topic: this keeps a topic, its claims and its
/// neighbourhood on the map bright.
pub const FOCUS_REACH: f32 = 60.0;
/// How much a difference in time counts toward the distance from the
/// focus, against a difference on the map. Below one, so that a topic's
/// claims recorded on later days stay in its neighbourhood.
pub const TIME_WEIGHT: f32 = 0.5;

/// The focused node, by its index in the layout.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Focus(pub Option<usize>);

/// The node under the pointer, by its index in the layout.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hovered(pub Option<usize>);

/// The nodes focused before the current one, by id, the latest last, at
/// most [`HISTORY_LENGTH`] of them. An id names the same node after the
/// graph is read again, where an index may not, and an id that names no
/// drawn node is passed over on the way back.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusHistory {
    pub earlier: Vec<String>,
    /// The focus as last recorded, by id.
    pub current: Option<String>,
    /// The focus was set by going back, so its change is not a step to
    /// record.
    pub going_back: bool,
}

/// Asks for the focus before the current one, from `B` or the panel's Back
/// button.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoBack;

/// The history after the focus changed to `next`: the focus left is kept
/// unless it was empty, the same as the one before it, or left by going
/// back.
pub fn record(history: &FocusHistory, next: Option<String>) -> FocusHistory {
    let mut after = history.clone();
    if after.current == next {
        after.going_back = false;
        return after;
    }
    if let Some(left) = after.current.take()
        && !history.going_back
        && after.earlier.last() != Some(&left)
    {
        after.earlier.push(left);
        let over = after.earlier.len().saturating_sub(HISTORY_LENGTH);
        after.earlier.drain(..over);
    }
    after.current = next;
    after.going_back = false;
    after
}

/// The node an id names, while it is drawn.
fn drawn_node(index: &GraphIndex, id: &str) -> Option<usize> {
    index
        .by_id
        .get(id)
        .copied()
        .filter(|&i| index.active.get(i).copied().unwrap_or(false))
}

/// The node going back would focus: the latest earlier focus that is drawn
/// and is not the current one.
pub fn back_target(history: &FocusHistory, index: &GraphIndex) -> Option<usize> {
    history
        .earlier
        .iter()
        .rev()
        .filter(|id| Some(*id) != history.current.as_ref())
        .find_map(|id| drawn_node(index, id))
}

/// Goes back: takes the earlier foci off the history down to the latest
/// one that is drawn, and gives the node to focus.
pub fn go_back(history: &mut FocusHistory, index: &GraphIndex) -> Option<usize> {
    while let Some(id) = history.earlier.pop() {
        if Some(&id) == history.current.as_ref() {
            continue;
        }
        if let Some(node) = drawn_node(index, &id) {
            history.going_back = true;
            return Some(node);
        }
    }
    None
}

/// How a disc stands out from the rest, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Highlight {
    Plain,
    /// At the far end of one of the hovered node's claim-to-claim edges.
    NearHovered,
    /// At the far end of one of the focused node's claim-to-claim edges.
    NearFocused,
    Hovered,
    Focused,
}

/// The drawn nodes at the far ends of the focused node's claim-to-claim
/// edges and of the hovered node's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Near {
    pub focused: HashSet<usize>,
    pub hovered: HashSet<usize>,
}

/// The nodes near the focus and the hover along their claim-to-claim
/// edges, read from the two nodes' own lists of edges.
pub fn near(edges: &EdgeIndex, focus: Focus, hovered: Hovered) -> Near {
    let of = |node: Option<usize>| -> HashSet<usize> {
        node.map(|n| claim_neighbours(edges, n).into_iter().collect())
            .unwrap_or_default()
    };
    Near {
        focused: of(focus.0),
        hovered: of(hovered.0),
    }
}

/// Keeps [`Focus`] and [`Hovered`] up to date with the pointer and the keys.
/// The focus the app starts with, from `--focus`, is inserted before it.
pub struct FocusPlugin;

impl Plugin for FocusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Focus>()
            .init_resource::<Hovered>()
            .init_resource::<UiFocus>()
            .init_resource::<FocusHistory>()
            .add_message::<GoBack>()
            .add_observer(focus_on_click)
            // Before the bar's own systems in `Update`, which close an open
            // list on the same key.
            .add_systems(PreUpdate, clear_on_escape.after(InputSystems))
            .add_systems(Update, (track_hover, back_on_request))
            .add_systems(PostUpdate, record_focus);
    }
}

/// How a node stands out: focused, hovered, near the focus, near the
/// hover, or none of these. A node that is several shows as the first.
pub fn highlight(index: usize, focus: Focus, hovered: Hovered, near: &Near) -> Highlight {
    if focus.0 == Some(index) {
        Highlight::Focused
    } else if hovered.0 == Some(index) {
        Highlight::Hovered
    } else if near.focused.contains(&index) {
        Highlight::NearFocused
    } else if near.hovered.contains(&index) {
        Highlight::NearHovered
    } else {
        Highlight::Plain
    }
}

/// The distance between two points as the focus falloff measures it: the
/// distance on the map, with the difference in time weighed by
/// [`TIME_WEIGHT`]. Time is Bevy's y.
pub fn focus_distance(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    (d.x * d.x + d.z * d.z + (TIME_WEIGHT * d.y).powi(2)).sqrt()
}

/// The share of its alpha a node keeps at this distance from the focus:
/// all of it at the focus, falling along a smooth step to [`FAR_ALPHA`] at
/// [`FOCUS_REACH`], and that beyond. A distance that is not a number counts
/// as far.
pub fn focus_falloff(distance: f32) -> f32 {
    if distance.is_nan() {
        return FAR_ALPHA;
    }
    let t = (distance / FOCUS_REACH).clamp(0.0, 1.0);
    let step = t * t * (3.0 - 2.0 * t);
    1.0 - (1.0 - FAR_ALPHA) * step
}

/// A node's alpha, as a fraction of its kind's own: the filter's alpha,
/// times the falloff from the focus where one is set.
pub fn node_alpha(filter_alpha: f32, focus: Option<Vec3>, at: Vec3) -> f32 {
    filter_alpha * focus.map_or(1.0, |centre| focus_falloff(focus_distance(centre, at)))
}

/// The nearest of the discs under the pointer, from each hit's depth and
/// the disc's node index.
pub fn nearest_disc(hits: impl IntoIterator<Item = (f32, usize)>) -> Option<usize> {
    hits.into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, index)| index)
}

/// A left click on a disc focuses its node, and a left click that reached
/// the window, with no disc and no part of the bar under the pointer,
/// clears the focus. The observer sees each click once at every entity it
/// climbs through, and acts only at the entity first clicked.
fn focus_on_click(
    click: On<Pointer<Click>>,
    discs: Query<&Disc>,
    windows: Query<(), With<Window>>,
    ui: Res<UiFocus>,
    mut focus: ResMut<Focus>,
) {
    let target = click.original_event_target();
    if click.button != PointerButton::Primary || click.entity != target || ui.pointer_over_ui {
        return;
    }
    if let Ok(disc) = discs.get(target) {
        focus.set_if_neq(Focus(Some(disc.index)));
    } else if windows.contains(target) {
        focus.set_if_neq(Focus(None));
    }
}

/// Escape clears the focus, unless a list of the bar is open, which the
/// bar closes on it, or a text field has the keyboard.
fn clear_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiFocus>,
    open: Option<Res<OpenPicker>>,
    mut focus: ResMut<Focus>,
) {
    let list_open = open.is_some_and(|open| open.0.is_some());
    if keys.just_pressed(CLEAR_FOCUS_KEY.0) && !ui.typing && !list_open {
        focus.set_if_neq(Focus(None));
    }
}

/// `B`, unless a text field has the keyboard, or the panel's Back button
/// focuses the node focused before.
fn back_on_request(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiFocus>,
    mut asked: MessageReader<GoBack>,
    index: Option<Res<GraphIndex>>,
    mut history: ResMut<FocusHistory>,
    mut focus: ResMut<Focus>,
) {
    let pressed = !ui.typing && keys.just_pressed(BACK_KEY.0);
    if !(asked.read().count() > 0 || pressed) {
        return;
    }
    let Some(index) = index else {
        return;
    };
    match go_back(&mut history, &index) {
        Some(node) => {
            focus.set_if_neq(Focus(Some(node)));
        }
        None => info!("no earlier focus to go back to"),
    }
}

/// Records each change of focus in [`FocusHistory`], once the update's
/// changes are made.
fn record_focus(
    focus: Res<Focus>,
    index: Option<Res<GraphIndex>>,
    mut history: ResMut<FocusHistory>,
) {
    if !focus.is_changed() && !history.going_back {
        return;
    }
    let next = focus
        .0
        .and_then(|i| index.as_ref().and_then(|index| index.ids.get(i).cloned()));
    let after = record(&history, next);
    history.set_if_neq(after);
}

/// Keeps [`Hovered`] on the nearest disc under the mouse pointer, and on
/// none while the pointer is over the bar or the side panel.
fn track_hover(
    hover: Option<Res<HoverMap>>,
    ui: Res<UiFocus>,
    discs: Query<&Disc>,
    mut hovered: ResMut<Hovered>,
) {
    let next = if ui.pointer_over_ui {
        None
    } else {
        hover
            .as_deref()
            .and_then(|hover| hover.get(&PointerId::Mouse))
            .and_then(|hits| {
                nearest_disc(hits.iter().filter_map(|(entity, hit)| {
                    discs.get(*entity).ok().map(|disc| (hit.depth, disc.index))
                }))
            })
    };
    hovered.set_if_neq(Hovered(next));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::Location;

    use super::*;
    use crate::bar::Picker;
    use crate::layout::Kind;

    #[test]
    fn the_falloff_runs_from_whole_at_the_focus_to_far_alpha_at_its_reach() {
        assert_eq!(focus_falloff(0.0), 1.0);
        assert_eq!(focus_falloff(-5.0), 1.0, "a distance is never below zero");
        assert!((focus_falloff(FOCUS_REACH) - FAR_ALPHA).abs() < 1e-6);
        assert!((focus_falloff(FOCUS_REACH * 10.0) - FAR_ALPHA).abs() < 1e-6);
        assert!((focus_falloff(f32::INFINITY) - FAR_ALPHA).abs() < 1e-6);
        assert_eq!(focus_falloff(f32::NAN), FAR_ALPHA);
        let halfway = focus_falloff(FOCUS_REACH / 2.0);
        assert!(
            (halfway - (1.0 + FAR_ALPHA) / 2.0).abs() < 1e-6,
            "{halfway}"
        );
        // It never rises with distance, and stays between its ends.
        let samples: Vec<f32> = (0..=100)
            .map(|i| focus_falloff(i as f32 * FOCUS_REACH / 50.0))
            .collect();
        assert!(samples.windows(2).all(|w| w[1] <= w[0]));
        assert!(samples.iter().all(|&a| (FAR_ALPHA..=1.0).contains(&a)));
        // Flat at the focus, so the nearest neighbours keep nearly all.
        assert!(focus_falloff(FOCUS_REACH * 0.05) > 0.98);
    }

    #[test]
    fn time_counts_for_less_than_the_map_in_the_distance_from_the_focus() {
        let focus = Vec3::new(10.0, 50.0, -4.0);
        assert_eq!(focus_distance(focus, focus), 0.0);
        let across = focus_distance(focus, focus + Vec3::new(3.0, 0.0, 4.0));
        assert!((across - 5.0).abs() < 1e-5);
        let later = focus_distance(focus, focus + Vec3::Y * 10.0);
        assert!((later - 10.0 * TIME_WEIGHT).abs() < 1e-5);
        assert!(later < 10.0);
    }

    #[test]
    fn a_focus_fades_the_filters_alpha_and_none_leaves_it() {
        let at = Vec3::new(FOCUS_REACH * 2.0, 0.0, 0.0);
        assert_eq!(node_alpha(0.12, None, at), 0.12);
        assert_eq!(node_alpha(1.0, Some(at), at), 1.0);
        assert!((node_alpha(1.0, Some(Vec3::ZERO), at) - FAR_ALPHA).abs() < 1e-6);
        assert_eq!(node_alpha(0.0, Some(at), at), 0.0, "hidden stays hidden");
    }

    #[test]
    fn the_focused_node_outranks_the_hovered_one_and_both_their_neighbours() {
        let (focus, hovered) = (Focus(Some(1)), Hovered(Some(2)));
        let near = Near {
            focused: [2, 3, 4].into(),
            hovered: [1, 4, 5].into(),
        };
        let none = Near::default();
        assert_eq!(highlight(1, focus, hovered, &near), Highlight::Focused);
        assert_eq!(highlight(2, focus, hovered, &near), Highlight::Hovered);
        assert_eq!(highlight(3, focus, hovered, &none), Highlight::Plain);
        assert_eq!(highlight(3, focus, hovered, &near), Highlight::NearFocused);
        assert_eq!(
            highlight(4, focus, hovered, &near),
            Highlight::NearFocused,
            "near both"
        );
        assert_eq!(highlight(5, focus, hovered, &near), Highlight::NearHovered);
        assert_eq!(
            highlight(1, focus, Hovered(Some(1)), &none),
            Highlight::Focused,
            "both"
        );
        assert!(Highlight::NearHovered < Highlight::NearFocused);
        assert!(Highlight::NearFocused < Highlight::Hovered);
    }

    #[test]
    fn the_nodes_near_the_focus_and_the_hover_come_from_their_claim_edges() {
        use crate::graph_index::tests::edge;
        let by_id = ["f", "i", "g", "t"]
            .iter()
            .enumerate()
            .map(|(i, &n)| (n.to_owned(), i))
            .collect();
        let edges = crate::edge_index::build(
            &[
                edge("f", "i", "supports"),
                edge("g", "i", "supports"),
                edge("f", "t", "tagged_with_topic"),
                edge("old", "g", "superseded_by"),
            ],
            &by_id,
            4,
        );
        let found = near(&edges, Focus(Some(1)), Hovered(Some(0)));
        assert_eq!(found.focused, [0, 2].into());
        assert_eq!(found.hovered, [1].into(), "membership is not a neighbour");
        assert_eq!(near(&edges, Focus(None), Hovered(None)), Near::default());
    }

    fn ids(names: &[&str]) -> GraphIndex {
        GraphIndex {
            ids: names.iter().map(|&n| n.to_owned()).collect(),
            by_id: names
                .iter()
                .enumerate()
                .map(|(i, &n)| (n.to_owned(), i))
                .collect(),
            active: vec![true; names.len()],
            ..GraphIndex::default()
        }
    }

    fn walked(steps: &[Option<&str>]) -> FocusHistory {
        steps.iter().fold(FocusHistory::default(), |history, step| {
            record(&history, step.map(str::to_owned))
        })
    }

    #[test]
    fn the_history_keeps_each_focus_left_once_and_no_more_than_its_length() {
        let history = walked(&[Some("a"), Some("b"), None, Some("c"), Some("c")]);
        assert_eq!(history.earlier, ["a", "b"], "an empty focus is not a step");
        assert_eq!(history.current.as_deref(), Some("c"));
        let long: Vec<String> = (0..HISTORY_LENGTH + 5).map(|i| format!("n{i}")).collect();
        let steps: Vec<Option<&str>> = long.iter().map(|s| Some(s.as_str())).collect();
        let history = walked(&steps);
        assert_eq!(history.earlier.len(), HISTORY_LENGTH);
        assert_eq!(history.earlier[0], "n4", "the oldest go first");
        assert_eq!(
            history.earlier.last().map(String::as_str),
            Some(format!("n{}", HISTORY_LENGTH + 3).as_str())
        );
    }

    #[test]
    fn going_back_returns_along_the_chain_and_passes_over_nodes_not_drawn() {
        let mut index = ids(&["a", "b", "c", "d"]);
        let mut history = walked(&[Some("a"), Some("gone"), Some("b"), Some("c"), Some("d")]);
        index.active[2] = false; // c has left the active set.
        assert_eq!(back_target(&history, &index), Some(1));
        assert_eq!(go_back(&mut history, &index), Some(1), "past c to b");
        history = record(&history, Some("b".into()));
        assert_eq!(history.earlier, ["a", "gone"], "going back is not a step");
        assert_eq!(
            go_back(&mut history, &index),
            Some(0),
            "past the id not drawn"
        );
        history = record(&history, Some("a".into()));
        assert_eq!(back_target(&history, &index), None);
        assert_eq!(go_back(&mut history, &index), None);
        // A new step after going back starts the history from there.
        history = record(&history, Some("d".into()));
        assert_eq!(history.earlier, ["a"]);
    }

    #[test]
    fn b_and_the_back_message_focus_the_node_before() {
        let mut app = app();
        app.insert_resource(ids(&["a", "b", "c"]));
        for node in [0, 1, 2] {
            app.insert_resource(Focus(Some(node)));
            app.update();
        }
        app.world_mut().write_message(GoBack);
        app.update();
        assert_eq!(focus(&app), Some(1));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(BACK_KEY.0);
        app.update();
        assert_eq!(focus(&app), Some(0));
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(BACK_KEY.0);
        keys.clear();
        app.world_mut().resource_mut::<UiFocus>().typing = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(BACK_KEY.0);
        app.update();
        assert_eq!(
            focus(&app),
            Some(0),
            "B is typed while the field has the keys"
        );
    }

    #[test]
    fn the_nearest_disc_under_the_pointer_is_hovered() {
        assert_eq!(nearest_disc([]), None);
        assert_eq!(nearest_disc([(5.0, 1), (2.0, 7), (9.0, 3)]), Some(7));
        assert_eq!(nearest_disc([(2.0, 4), (2.0, 3)]), Some(3), "a tie");
    }

    /// The focus systems and observer, with no window on the screen.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<KeyCode>>()
            .add_plugins(FocusPlugin);
        app.world_mut().register_component::<Window>();
        app.update();
        app
    }

    /// A left click on an entity, as the picking backend sends it.
    fn click(app: &mut App, entity: Entity, button: PointerButton) {
        let location = Location {
            target: NormalizedRenderTarget::None {
                width: 1,
                height: 1,
            },
            position: Vec2::ZERO,
        };
        let event = Click {
            button,
            hit: HitData::new(entity, 0.0, None, None),
            duration: Duration::ZERO,
            count: 1,
        };
        app.world_mut()
            .trigger(Pointer::new(PointerId::Mouse, location, event, entity));
        app.update();
    }

    fn focus(app: &App) -> Option<usize> {
        app.world().resource::<Focus>().0
    }

    fn escape(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::Escape);
        keys.clear();
    }

    #[test]
    fn a_click_on_a_disc_focuses_it_and_one_on_the_empty_scene_clears_it() {
        let mut app = app();
        let disc = app
            .world_mut()
            .spawn(Disc {
                index: 4,
                kind: Kind::Fact,
            })
            .id();
        let window = app.world_mut().spawn(Window::default()).id();
        let bar = app.world_mut().spawn(Node::default()).id();

        click(&mut app, disc, PointerButton::Secondary);
        assert_eq!(focus(&app), None, "the right button looks around");
        click(&mut app, disc, PointerButton::Primary);
        assert_eq!(focus(&app), Some(4));
        click(&mut app, bar, PointerButton::Primary);
        assert_eq!(focus(&app), Some(4), "a click on the bar keeps it");
        click(&mut app, window, PointerButton::Primary);
        assert_eq!(focus(&app), None);

        click(&mut app, disc, PointerButton::Primary);
        app.world_mut().resource_mut::<UiFocus>().pointer_over_ui = true;
        click(&mut app, window, PointerButton::Primary);
        assert_eq!(focus(&app), Some(4), "not while the pointer is on the bar");
    }

    #[test]
    fn escape_clears_the_focus_once_no_list_is_open() {
        let mut app = app();
        app.insert_resource(Focus(Some(2)))
            .insert_resource(OpenPicker(Some(Picker::Topic)));
        escape(&mut app);
        assert_eq!(focus(&app), Some(2), "Escape closes the list first");
        app.insert_resource(OpenPicker(None));
        app.world_mut().resource_mut::<UiFocus>().typing = true;
        escape(&mut app);
        assert_eq!(focus(&app), Some(2), "the text field has the key");
        app.world_mut().resource_mut::<UiFocus>().typing = false;
        escape(&mut app);
        assert_eq!(focus(&app), None);
    }
}
