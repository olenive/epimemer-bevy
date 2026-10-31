//! Search (DESIGN.md §10): a substring over the content and description of
//! the visible nodes, whose hits are focused in turn.
//!
//! The bar's search field (`bar.rs`) is a text field that takes the
//! keyboard only while it has the input focus: a click on it, or `/` from
//! the scene, gives it the keys, and Escape, or a press anywhere else, gives
//! them back, so the camera's keys work whenever it does not hold them.
//! While it holds them, Enter focuses the first hit and each further Enter
//! the next, Shift+Enter the one before, wrapping at either end. The camera
//! stays where it is: the hit focused is labelled and brightened as any
//! focus is, and the bar's Go button, or `G` from the scene, fits the
//! current hit's topic as `T` fits the focused node's. Clearing the field
//! clears the hits and leaves the focus where it is.
//!
//! [`Search`] holds what is typed and the hits [`find_hits`] finds: every
//! node the filter leaves visible whose content or description holds the
//! text, ignoring case, in layout order. A node the filter hides, one with
//! no place in the time mode drawn, and one that has left the active set
//! are never hits. The hits are found again whenever the text, the filter
//! or the graph changes; the hit walked to last is kept, by its id, while
//! it is still a hit, and a new text starts the walk again from the first.

use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;
use bevy::text::EditableText;

use crate::bar::{Collapsed, OpenPicker, Picker, style};
use crate::camera::ViewRequest;
use crate::filter::{RefreshShown, Shown, is_visible};
use crate::focus::Focus;
use crate::graph_index::GraphIndex;
use crate::keys::{CLEAR_FOCUS_KEY, GO_KEY, SEARCH_KEY, WALK_KEYS};
use crate::ui_focus::UiFocus;

/// What is searched for, and what it finds.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    /// What is typed into the search field.
    pub text: String,
    /// The text the hits were found for.
    pub found_for: String,
    /// The visible nodes whose content or description holds the text, in
    /// layout order.
    pub hits: Vec<usize>,
    /// The hit walked to last, by its node's id.
    pub walked: Option<String>,
}

/// The bar's search field.
#[derive(Component, Debug)]
pub struct SearchField;

/// The text after the search field that says how many hits there are and
/// which one is focused.
#[derive(Component, Debug)]
pub struct SearchCount;

/// What the bar's search buttons ask for.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchAction {
    /// Fit the current hit's topic.
    Go,
    /// Empty the field.
    Clear,
}

pub struct SearchPlugin;

impl Plugin for SearchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Search>()
            .init_resource::<Focus>()
            .init_resource::<UiFocus>()
            .init_resource::<Collapsed>()
            .init_resource::<OpenPicker>()
            .add_message::<SearchAction>()
            .add_message::<ViewRequest>()
            .add_systems(
                Update,
                (
                    read_field,
                    take_keyboard,
                    give_back_keyboard,
                    walk_on_enter,
                    clear_on_request,
                    go_on_request,
                    show_count,
                )
                    .chain(),
            )
            .add_systems(PostUpdate, refresh_hits.after(RefreshShown));
    }
}

/// Whether a text holds the search text, which is already in lower case.
pub fn holds(text: &str, wanted: &str) -> bool {
    text.to_lowercase().contains(wanted)
}

/// The text searched for, as it is matched: without the space around it,
/// in lower case.
pub fn wanted(text: &str) -> String {
    text.trim().to_lowercase()
}

/// Every visible node whose content or description holds `text`, ignoring
/// case, in layout order; none for a blank text.
pub fn find_hits(index: &GraphIndex, visible: &[bool], text: &str) -> Vec<usize> {
    let wanted = wanted(text);
    if wanted.is_empty() {
        return Vec::new();
    }
    (0..index.ids.len())
        .filter(|&i| is_visible(visible, i) && index.active.get(i).copied().unwrap_or(true))
        .filter(|&i| {
            let content = index.names.get(i).map_or("", String::as_str);
            let description = index.views.get(i).map_or("", |v| v.description.as_str());
            holds(content, &wanted) || holds(description, &wanted)
        })
        .collect()
}

/// The search after finding its hits again for its text: the hit walked to
/// last is kept when the text is the one it was walked for.
pub fn refound(search: &Search, index: &GraphIndex, visible: &[bool]) -> Search {
    let same_text = search.text == search.found_for;
    Search {
        text: search.text.clone(),
        found_for: search.text.clone(),
        hits: find_hits(index, visible, &search.text),
        walked: search.walked.clone().filter(|_| same_text),
    }
}

/// Where among the hits the hit walked to last is, while it is one.
pub fn position(search: &Search, index: &GraphIndex) -> Option<usize> {
    let walked = search.walked.as_deref()?;
    search
        .hits
        .iter()
        .position(|&i| index.ids.get(i).map(String::as_str) == Some(walked))
}

/// The next place among `count` hits from `from`, forward or back,
/// wrapping at either end; from no place, forward is the first and back the
/// last. `None` when there is no hit.
pub fn step(count: usize, from: Option<usize>, forward: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (from.filter(|&f| f < count), forward) {
        (None, true) => 0,
        (None, false) => count - 1,
        (Some(f), true) => (f + 1) % count,
        (Some(f), false) => (f + count - 1) % count,
    })
}

/// Walks to the next hit, or the one before, and gives the node to focus.
pub fn walk(search: &mut Search, index: &GraphIndex, forward: bool) -> Option<usize> {
    let at = step(search.hits.len(), position(search, index), forward)?;
    let node = search.hits[at];
    search.walked = index.ids.get(node).cloned();
    Some(node)
}

/// The current hit, or the first where none is walked to yet, which it
/// becomes the current one.
pub fn current(search: &mut Search, index: &GraphIndex) -> Option<usize> {
    match position(search, index) {
        Some(at) => Some(search.hits[at]),
        None => walk(search, index, true),
    }
}

/// What the bar says after the field: how many hits, and which is focused.
pub fn count_text(search: &Search, at: Option<usize>) -> String {
    if wanted(&search.text).is_empty() {
        return String::new();
    }
    match (search.hits.len(), at) {
        (0, _) => "no hits".to_owned(),
        (count, Some(at)) => format!("{} of {count}", at + 1),
        (1, None) => "1 hit".to_owned(),
        (count, None) => format!("{count} hits"),
    }
}

/// The search `--search` starts with: the text typed, its hits, and the
/// first of them walked to, with the node to focus.
pub fn start(index: &GraphIndex, visible: &[bool], text: &str) -> (Search, Option<usize>) {
    let mut search = Search {
        text: text.to_owned(),
        found_for: text.to_owned(),
        hits: find_hits(index, visible, text),
        walked: None,
    };
    let first = walk(&mut search, index, true);
    (search, first)
}

/// The field, if the bar has one.
fn the_field(fields: &Query<Entity, With<SearchField>>) -> Option<Entity> {
    fields.iter().next()
}

/// Whether the search field has the keyboard.
fn field_has_keys(fields: &Query<Entity, With<SearchField>>, input: Option<&InputFocus>) -> bool {
    the_field(fields).is_some_and(|field| input.and_then(InputFocus::get) == Some(field))
}

/// Copies what is typed into the field into [`Search`].
fn read_field(fields: Query<&EditableText, With<SearchField>>, mut search: ResMut<Search>) {
    let Some(field) = fields.iter().next() else {
        return;
    };
    let typed = field.value().to_string();
    if search.text != typed {
        search.text = typed;
    }
}

/// `/` from the scene gives the field the keyboard, opening the bar first
/// if it is collapsed. The key itself is not typed: the field reads the
/// keys sent to it from the next update.
fn take_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiFocus>,
    fields: Query<Entity, With<SearchField>>,
    input: Option<ResMut<InputFocus>>,
    mut collapsed: ResMut<Collapsed>,
) {
    if ui.typing || !keys.just_pressed(SEARCH_KEY.0) {
        return;
    }
    let (Some(field), Some(mut input)) = (the_field(&fields), input) else {
        return;
    };
    input.set(field, FocusCause::Navigated);
    collapsed.set_if_neq(Collapsed(false));
}

/// Escape, or a left press anywhere but on the field, gives the keyboard
/// back to the scene.
fn give_back_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    hover: Option<Res<HoverMap>>,
    fields: Query<Entity, With<SearchField>>,
    input: Option<ResMut<InputFocus>>,
) {
    let Some(mut input) = input else {
        return;
    };
    let Some(field) = the_field(&fields).filter(|&f| input.get() == Some(f)) else {
        return;
    };
    let on_field = hover
        .as_deref()
        .and_then(|hover| hover.get(&PointerId::Mouse))
        .is_some_and(|hits| hits.contains_key(&field));
    let pressed_elsewhere = buttons.is_some_and(|b| b.just_pressed(MouseButton::Left)) && !on_field;
    if keys.just_pressed(CLEAR_FOCUS_KEY.0) || pressed_elsewhere {
        input.clear();
    }
}

/// Enter in the field focuses the next hit, Shift+Enter the one before.
/// The topic picker's Enter is its own while its list is open.
fn walk_on_enter(
    keys: Res<ButtonInput<KeyCode>>,
    fields: Query<Entity, With<SearchField>>,
    input: Option<Res<InputFocus>>,
    open: Res<OpenPicker>,
    index: Option<Res<GraphIndex>>,
    mut search: ResMut<Search>,
    mut focus: ResMut<Focus>,
) {
    if !keys.any_just_pressed(WALK_KEYS)
        || open.0 == Some(Picker::Topic)
        || !field_has_keys(&fields, input.as_deref())
    {
        return;
    }
    let Some(index) = index else {
        return;
    };
    let forward = !keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if let Some(node) = walk(&mut search, &index, forward) {
        focus.set_if_neq(Focus(Some(node)));
    }
}

/// The Clear button empties the field, which clears the hits; the focus
/// stays.
fn clear_on_request(
    mut asked: MessageReader<SearchAction>,
    mut fields: Query<&mut EditableText, With<SearchField>>,
    mut search: ResMut<Search>,
) {
    if !asked.read().any(|a| *a == SearchAction::Clear) {
        return;
    }
    for mut field in &mut fields {
        field.clear();
    }
    if !search.text.is_empty() {
        search.text.clear();
    }
}

/// The Go button, or `G` from the scene, focuses the current hit, or the
/// first, and fits its topic as `T` does.
fn go_on_request(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiFocus>,
    mut asked: MessageReader<SearchAction>,
    index: Option<Res<GraphIndex>>,
    mut search: ResMut<Search>,
    mut focus: ResMut<Focus>,
    mut views: MessageWriter<ViewRequest>,
) {
    let clicked = asked.read().any(|a| *a == SearchAction::Go);
    if !(clicked || (!ui.typing && keys.just_pressed(GO_KEY.0))) {
        return;
    }
    let Some(index) = index else {
        return;
    };
    let Some(node) = current(&mut search, &index) else {
        info!("no search hit to go to");
        return;
    };
    focus.set_if_neq(Focus(Some(node)));
    views.write(ViewRequest::FocusedTopic);
}

/// Keeps the count after the field up to date, and the field's border lit
/// while it has the keyboard.
fn show_count(
    search: Res<Search>,
    index: Option<Res<GraphIndex>>,
    input: Option<Res<InputFocus>>,
    mut counts: Query<&mut Text, With<SearchCount>>,
    mut fields: Query<(Entity, &mut BorderColor), With<SearchField>>,
) {
    let at = index.as_deref().and_then(|index| position(&search, index));
    let words = count_text(&search, at);
    for mut text in &mut counts {
        if text.0 != words {
            text.0.clone_from(&words);
        }
    }
    let held = input.as_deref().and_then(InputFocus::get);
    for (field, mut border) in &mut fields {
        let colour = if held == Some(field) {
            style::ACCENT
        } else {
            style::PANEL_EDGE
        };
        border.set_if_neq(BorderColor::all(colour));
    }
}

/// Finds the hits again when the text, the filter or the graph changed.
fn refresh_hits(shown: Res<Shown>, index: Option<Res<GraphIndex>>, mut search: ResMut<Search>) {
    let Some(index) = index else {
        return;
    };
    if !(search.is_changed() || shown.is_changed() || index.is_changed()) {
        return;
    }
    let next = refound(&search, &index, &shown.visible);
    search.set_if_neq(next);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_index::tests::node;
    use crate::hub::NodeType;

    /// Five nodes: a topic whose description names swifts, three claims,
    /// and a claim that has left the active set.
    fn index() -> GraphIndex {
        let views = vec![
            crate::hub::NodeView {
                content: "Bird migration".into(),
                description: "Where Swifts and swallows go.".into(),
                ..node("t", NodeType::Topic, 1)
            },
            crate::hub::NodeView {
                content: "Common SWIFTS winter in Africa.".into(),
                ..node("f1", NodeType::Fact, 2)
            },
            crate::hub::NodeView {
                content: "Swallows nest in barns.".into(),
                ..node("f2", NodeType::Fact, 3)
            },
            crate::hub::NodeView {
                content: "Swifts arrive in late April.".into(),
                ..node("f3", NodeType::Fact, 4)
            },
            crate::hub::NodeView {
                content: "Swifts sleep on the wing.".into(),
                ..node("gone", NodeType::Fact, 5)
            },
        ];
        GraphIndex {
            ids: views.iter().map(|v| v.node_id.clone()).collect(),
            names: views.iter().map(|v| v.content.clone()).collect(),
            by_id: views
                .iter()
                .enumerate()
                .map(|(i, v)| (v.node_id.clone(), i))
                .collect(),
            active: vec![true, true, true, true, false],
            views,
            ..GraphIndex::default()
        }
    }

    #[test]
    fn a_hit_holds_the_text_in_its_content_or_description_ignoring_case() {
        let index = index();
        let all = vec![true; 5];
        assert_eq!(
            find_hits(&index, &all, "swift"),
            [0, 1, 3],
            "not the node gone"
        );
        assert_eq!(find_hits(&index, &all, "  SWALLOWS "), [0, 2]);
        assert_eq!(find_hits(&index, &all, "barns."), [2]);
        assert!(find_hits(&index, &all, "owls").is_empty());
        assert!(find_hits(&index, &all, "   ").is_empty(), "a blank text");
    }

    #[test]
    fn a_node_the_filter_hides_is_never_a_hit() {
        let index = index();
        let visible = [true, false, true, true, true];
        assert_eq!(find_hits(&index, &visible, "swift"), [0, 3]);
        assert!(find_hits(&index, &[false; 5], "swift").is_empty());
    }

    #[test]
    fn the_walk_wraps_at_either_end() {
        assert_eq!(step(3, None, true), Some(0));
        assert_eq!(
            step(3, None, false),
            Some(2),
            "back from nowhere is the last"
        );
        assert_eq!(step(3, Some(0), true), Some(1));
        assert_eq!(step(3, Some(2), true), Some(0));
        assert_eq!(step(3, Some(0), false), Some(2));
        assert_eq!(step(3, Some(9), true), Some(0), "a place past the end");
        assert_eq!(step(0, None, true), None);

        let index = index();
        let (mut search, first) = start(&index, &[true; 5], "swift");
        assert_eq!(first, Some(0));
        let walked: Vec<Option<usize>> = (0..4).map(|_| walk(&mut search, &index, true)).collect();
        assert_eq!(walked, [Some(1), Some(3), Some(0), Some(1)]);
        assert_eq!(walk(&mut search, &index, false), Some(0));
        assert_eq!(walk(&mut search, &index, false), Some(3));
        assert_eq!(count_text(&search, position(&search, &index)), "3 of 3");
    }

    #[test]
    fn hits_found_again_keep_the_hit_walked_to_while_the_text_is_the_same() {
        let index = index();
        let (mut search, _) = start(&index, &[true; 5], "swift");
        walk(&mut search, &index, true);
        assert_eq!(search.walked.as_deref(), Some("f1"));
        // The filter hides the topic: f1 is still a hit, now the first.
        let hidden = [false, true, true, true, true];
        let again = refound(&search, &index, &hidden);
        assert_eq!(again.hits, [1, 3]);
        assert_eq!(position(&again, &index), Some(0));
        // A new text starts the walk again.
        let typed = Search {
            text: "swifts".into(),
            ..again.clone()
        };
        let fresh = refound(&typed, &index, &hidden);
        assert_eq!((fresh.walked, fresh.found_for.as_str()), (None, "swifts"));
        // Cleared, nothing is a hit.
        let cleared = refound(
            &Search {
                text: String::new(),
                ..again
            },
            &index,
            &hidden,
        );
        assert!(cleared.hits.is_empty() && cleared.walked.is_none());
        assert_eq!(count_text(&cleared, None), "");
    }

    #[test]
    fn the_count_says_how_many_and_which() {
        let search = |text: &str, hits: Vec<usize>| Search {
            text: text.into(),
            hits,
            ..Search::default()
        };
        assert_eq!(count_text(&search("", vec![]), None), "");
        assert_eq!(count_text(&search("owl", vec![]), None), "no hits");
        assert_eq!(count_text(&search("owl", vec![4]), None), "1 hit");
        assert_eq!(count_text(&search("owl", vec![4, 5]), None), "2 hits");
        assert_eq!(count_text(&search("owl", vec![4, 5]), Some(1)), "2 of 2");
    }

    /// The search systems, with a field that has the keyboard.
    fn app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<InputFocus>()
            .insert_resource(index())
            .insert_resource(Shown {
                visible: vec![true; 5],
                passes: vec![true; 5],
                ..Shown::default()
            })
            .add_plugins(SearchPlugin);
        let field = app.world_mut().spawn(SearchField).id();
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(field, FocusCause::Navigated);
        app.update();
        (app, field)
    }

    fn tap(app: &mut App, keys: &[KeyCode]) {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        for &key in keys {
            input.press(key);
        }
        app.update();
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        for &key in keys {
            input.release(key);
        }
        input.clear();
    }

    fn focus(app: &App) -> Option<usize> {
        app.world().resource::<Focus>().0
    }

    #[test]
    fn enter_walks_the_hits_and_shift_enter_walks_back() {
        let (mut app, _) = app();
        app.world_mut().resource_mut::<Search>().text = "swift".into();
        app.update();
        assert_eq!(app.world().resource::<Search>().hits, [0, 1, 3]);
        assert_eq!(focus(&app), None, "typing moves nothing");
        tap(&mut app, &[KeyCode::Enter]);
        assert_eq!(focus(&app), Some(0));
        tap(&mut app, &[KeyCode::NumpadEnter]);
        assert_eq!(focus(&app), Some(1));
        tap(&mut app, &[KeyCode::ShiftLeft, KeyCode::Enter]);
        assert_eq!(focus(&app), Some(0));
        tap(&mut app, &[KeyCode::ShiftRight, KeyCode::Enter]);
        assert_eq!(focus(&app), Some(3), "wrapped to the last");
        tap(&mut app, &[KeyCode::Enter]);
        assert_eq!(focus(&app), Some(0), "and round to the first");
    }

    #[test]
    fn the_filter_changing_finds_the_hits_again_and_hidden_nodes_drop_out() {
        let (mut app, _) = app();
        app.world_mut().resource_mut::<Search>().text = "swift".into();
        app.update();
        app.world_mut().resource_mut::<Shown>().visible = vec![true, true, true, false, true];
        app.update();
        assert_eq!(app.world().resource::<Search>().hits, [0, 1]);
    }

    #[test]
    fn enter_does_nothing_while_the_field_does_not_have_the_keys() {
        let (mut app, _) = app();
        app.world_mut().resource_mut::<Search>().text = "swift".into();
        app.update();
        app.world_mut().resource_mut::<InputFocus>().clear();
        tap(&mut app, &[KeyCode::Enter]);
        assert_eq!(focus(&app), None);
    }

    #[test]
    fn escape_gives_the_keys_back_and_slash_takes_them_again() {
        let (mut app, field) = app();
        tap(&mut app, &[KeyCode::Escape]);
        assert_eq!(app.world().resource::<InputFocus>().get(), None);
        app.insert_resource(Collapsed(true));
        tap(&mut app, &[KeyCode::Slash]);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(field));
        assert!(!app.world().resource::<Collapsed>().0, "the bar opens");
    }

    #[test]
    fn clearing_the_field_clears_the_hits_and_leaves_the_focus() {
        let (mut app, _) = app();
        app.world_mut().resource_mut::<Search>().text = "swift".into();
        app.update();
        tap(&mut app, &[KeyCode::Enter]);
        assert_eq!(focus(&app), Some(0));
        app.world_mut().write_message(SearchAction::Clear);
        app.update();
        app.update();
        let search = app.world().resource::<Search>();
        assert!(search.text.is_empty() && search.hits.is_empty());
        assert_eq!(focus(&app), Some(0));
    }

    #[test]
    fn go_focuses_the_current_hit_and_asks_for_its_topic() {
        let (mut app, _) = app();
        app.world_mut().resource_mut::<Search>().text = "swift".into();
        app.update();
        app.world_mut().write_message(SearchAction::Go);
        app.update();
        assert_eq!(focus(&app), Some(0), "the first, with none walked to");
        let asked: Vec<ViewRequest> = app
            .world_mut()
            .resource_mut::<Messages<ViewRequest>>()
            .drain()
            .collect();
        assert_eq!(asked, [ViewRequest::FocusedTopic]);
        tap(&mut app, &[KeyCode::Enter]);
        assert_eq!(focus(&app), Some(1));
        app.insert_resource(Focus(Some(4)));
        app.world_mut().resource_mut::<InputFocus>().clear();
        tap(&mut app, &[GO_KEY.0]);
        assert_eq!(focus(&app), Some(1), "G from the scene, back to the hit");
    }
}
