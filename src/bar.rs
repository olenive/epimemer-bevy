//! The bar along the top (DESIGN.md §10): the filters, the session and
//! graph pickers and the camera presets, in Bevy UI.
//!
//! The bar holds no state of its own about what is shown. Every filter
//! control writes the one [`Filter`] resource, and the scene, picking and
//! the camera follow it as they do any change of filter (`filter.rs`).
//! Every view button sends the camera the [`ViewRequest`] its key sends,
//! from the same key map (`keys.rs`). Choosing a graph sends a
//! [`LoadGraph`], and `reload.rs` reads it in the background and swaps it in.
//!
//! The controls, left to right: the session and the graph; how the event
//! stream stands (`live.rs`): connected with the count of what it has brought,
//! reconnecting with the last error, or reading the graph again; the time mode,
//! recorded time or claim time, which `C` switches too (`time_mode.rs`);
//! the metacontexts, the node kinds, the status (active only, as the
//! snapshot is); the topic subtree and the source document, each picked
//! from a list; the time window, a range slider over the time axis of the
//! mode drawn (`bar/slider.rs`); dim or hide; the edge toggle, which `L`
//! turns on and off too (`edges.rs`); the views; and Recalculate layout,
//! which `U` presses too, and which fits the topic map again from the
//! topics' embeddings and saves it (`relayout.rs`).
//! Hovering a control says what it does at the end of the bar, where load
//! errors are shown too, and what is drawn: the graph, its nodes, and the
//! topic map, or why there is none.
//!
//! A picker's list opens under its button and closes when an entry is
//! chosen, when the button is pressed again, on Escape, or on a press
//! anywhere else. The topic picker has a text field: while the pointer is
//! over the bar, typing narrows the list to the topics whose name starts
//! with what is typed, and Enter takes the first. While the pointer is over
//! the scene the keys go to the camera, so the field never holds W, A, S
//! and D away from it.
//!
//! The search field, after the edge toggle, works the other way
//! (`search.rs`): it takes the keyboard when it is clicked or `/` is
//! pressed, wherever the pointer is, and keeps it until Escape or a press
//! anywhere else, so that Enter can walk its hits; the count after it says
//! how many there are and which is focused, Go fits the current hit's topic
//! and Clear empties it.
//!
//! The bar collapses to a thin strip with one button that brings it back,
//! and `H` does both.
//!
//! The bar is UI, so Bevy's UI picking backend ranks it above every mesh
//! and each of its nodes blocks what is beneath: a click on the bar never
//! picks a disc. The wheel over the bar scrolls the bar's lists rather than
//! changing the camera's speed (`ui_focus.rs`). The bar reports how much of
//! the window it covers as [`BarCover`], and every view is fitted to the
//! part below it.

mod panels;
pub mod slider;
pub mod style;

use bevy::ecs::spawn::SpawnIter;
use bevy::ecs::system::SystemParam;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::PickingSystems;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui::{InteractionDisabled, UiSystems};

use crate::background::{Pending, Poll, in_background, poll};
use crate::camera::{BarCover, ViewRequest};
use crate::edges::AllEdges;
use crate::filter::{Filter, Kinds, Metacontexts, Mode, kind_passes};
use crate::graph_index::GraphIndex;
use crate::hub::{self, GraphList, SessionInfo};
use crate::keys::{BAR_KEY, EDGE_KEY, GO_KEY, RECALCULATE_KEY, SEARCH_KEY, VIEW_KEYS};
use crate::layout::Kind;
use crate::live::{self, LiveState};
use crate::pickers::{self, MetacontextEntry, Pickers, short_id};
use crate::relayout::{self, MapState, RecalculateLayout};
use crate::reload::{Catalogue, LoadGraph, LoadStatus};
use crate::scene::kind_colour;
use crate::search::{Search, SearchAction, SearchCount, SearchField};
use crate::time_axis::TimeMode;
use crate::ui_focus::{UiFocus, over_ui};

/// How wide the search field is, in logical pixels.
const SEARCH_WIDTH: f32 = 180.0;

/// A control that opens a list under its button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Picker {
    Session,
    Graph,
    Metacontext,
    Topic,
    Source,
}

/// How the bar starts, from `--ui-state`: open, collapsed, or open with a
/// picker's list open, and for the topic picker, something typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UiState {
    #[default]
    Open,
    Collapsed,
    Picker(Picker, String),
}

/// The names `--ui-state` takes.
pub const UI_STATES: &str =
    "open, collapsed, sessions, graphs, metacontexts, topics, topics:PREFIX or sources";

/// Reads a `--ui-state` value.
pub fn parse_ui_state(text: &str) -> Option<UiState> {
    let (name, typed) = text.split_once(':').unwrap_or((text, ""));
    let picker = |picker| Some(UiState::Picker(picker, String::new()));
    match (name, typed.is_empty()) {
        ("open", true) => Some(UiState::Open),
        ("collapsed", true) => Some(UiState::Collapsed),
        ("sessions", true) => picker(Picker::Session),
        ("graphs", true) => picker(Picker::Graph),
        ("metacontexts", true) => picker(Picker::Metacontext),
        ("sources", true) => picker(Picker::Source),
        ("topics", _) => Some(UiState::Picker(Picker::Topic, typed.to_owned())),
        _ => None,
    }
}

/// Whether the bar is collapsed to its strip.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Collapsed(pub bool);

/// The picker whose list is open, if any.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenPicker(pub Option<Picker>);

/// What is typed into the topic picker's field.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct TopicPrefix(pub String);

/// The hub reads the bar has started that have not answered yet: the
/// session list, and one session's graph list.
#[derive(Resource, Default)]
struct Fetches {
    sessions: Option<Pending<Result<Vec<SessionInfo>, String>>>,
    graphs: Option<(String, Pending<Result<GraphList, String>>)>,
}

/// The last failure of a list read, for the message line.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
struct Notice(Option<String>);

/// What pressing a control does.
#[derive(Component, Clone, Debug, PartialEq)]
pub enum Control {
    Collapse,
    Expand,
    Open(Picker),
    /// The recorded-time mode.
    RecordedTime,
    /// The claim-time mode.
    ClaimTime,
    Kind(Kind),
    Mode,
    /// The edge toggle: every claim-to-claim edge between visible nodes.
    AllEdges,
    View(ViewRequest),
    /// Recalculate layout: a new topic map.
    Recalculate,
    ClearTopic,
    ClearSource,
    ClearWindow,
    /// Fit the current search hit's topic.
    SearchGo,
    /// Empty the search field.
    SearchClear,
    AllMetacontexts,
    Metacontext(String),
    Session(String),
    Graph(String),
    Topic(String),
    Source(String),
}

/// A text the bar keeps up to date.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum BarText {
    Session,
    Graph,
    Metacontexts,
    Topic,
    Source,
    Mode,
    Window,
    /// The time window's caption, which names the time mode's axis.
    WindowCaption,
    /// How the event stream stands.
    Live,
    Message,
}

/// What hovering a control says in the message line.
#[derive(Component, Clone, Copy, Debug)]
struct Hint(&'static str);

/// The bar, open.
#[derive(Component)]
struct BarRoot;

/// The strip the bar collapses to.
#[derive(Component)]
struct Strip;

/// The bar or its strip: what covers the top of the viewport.
#[derive(Component)]
struct Covers;

/// The node a picker's list opens under.
#[derive(Component)]
struct Anchor(Picker);

/// An open picker's list.
#[derive(Component)]
struct Panel;

/// The topic picker's text field.
#[derive(Component)]
struct TopicField;

/// A row of the topic picker, by its place in `Pickers::topics`.
#[derive(Component)]
struct TopicRow(usize);

/// The topic picker's count of the topics listed.
#[derive(Component)]
struct TopicCount;

/// A row of a picker's list: clear at rest.
#[derive(Component)]
struct Row;

/// The inside of a metacontext's checkbox, lit while it passes.
#[derive(Component)]
struct CheckMark(String);

const CLAIM_TIME_HINT: &str = "Each claim over the period its timepoint gives, a claim with no timepoint at its recorded time; C switches.";
const RECALCULATE_HINT: &str = "Fits the topic map again from the topics' embeddings and saves it over the one saved; every topic moves, its claims with it. U does the same.";
const STATUS_HINT: &str = "The snapshot holds active nodes only, so every node is active.";
const RECORDED_HINT: &str = "Every node at the time it was recorded; C switches.";
const EDGES_HINT: &str = "Every claim-to-claim edge between visible nodes, as lines. Without it, a node's edges show while it is hovered or focused.";
const LIVE_HINT: &str = "The event stream from the hub: every change to the graph drawn appears as it is made, and the graph is read again after the stream drops or misses something.";
const SEARCH_HINT: &str = "Nodes whose text or description holds what is typed, ignoring case, among those drawn. / types here; Enter focuses the next hit, Shift+Enter the one before; Escape leaves the field.";

/// The bar, its strip and its pickers.
pub struct BarPlugin {
    pub start: UiState,
}

impl Plugin for BarPlugin {
    fn build(&self, app: &mut App) {
        let (collapsed, open, typed) = match &self.start {
            UiState::Open => (false, None, String::new()),
            UiState::Collapsed => (true, None, String::new()),
            UiState::Picker(picker, typed) => (false, Some(*picker), typed.clone()),
        };
        app.insert_resource(Collapsed(collapsed))
            .insert_resource(OpenPicker(open))
            .insert_resource(TopicPrefix(typed))
            .init_resource::<Fetches>()
            .init_resource::<Notice>()
            .init_resource::<UiFocus>()
            .init_resource::<AllEdges>()
            .init_resource::<TimeMode>()
            .add_message::<SearchAction>()
            .add_message::<RecalculateLayout>()
            .add_observer(on_click)
            .add_systems(Startup, spawn_bar)
            .add_systems(PreUpdate, track_focus.after(PickingSystems::Hover))
            .add_systems(PostUpdate, measure_cover.after(UiSystems::Layout))
            .add_systems(
                Update,
                (
                    toggle_on_key,
                    close_on_outside_press,
                    close_on_escape,
                    choose_first_topic,
                    start_fetches,
                    receive_fetches,
                    show_collapsed,
                    rebuild_panel,
                    read_topic_field,
                    narrow_topic_rows,
                    style_controls,
                    style_checkmarks,
                    update_texts,
                    slider::place_handles,
                )
                    .chain(),
            );
    }
}

/// The metacontexts that pass after a click on one of their checkboxes.
/// From all, the one clicked is left out; otherwise it is toggled, and a
/// set holding every metacontext listed is all again.
pub fn toggle_metacontext(
    current: &Metacontexts,
    id: &str,
    listed: &[MetacontextEntry],
) -> Metacontexts {
    let all: std::collections::BTreeSet<String> = listed.iter().map(|m| m.id.clone()).collect();
    let mut chosen = match current {
        Metacontexts::All => all.clone(),
        Metacontexts::Only(ids) => ids.clone(),
    };
    if !chosen.remove(id) {
        chosen.insert(id.to_owned());
    }
    if chosen == all {
        Metacontexts::All
    } else {
        Metacontexts::Only(chosen)
    }
}

/// Whether a metacontext's checkbox is ticked.
pub fn metacontext_ticked(current: &Metacontexts, id: &str) -> bool {
    match current {
        Metacontexts::All => true,
        Metacontexts::Only(ids) => ids.contains(id),
    }
}

/// The kinds after a click on one kind's toggle.
pub fn toggle_kind(kinds: Kinds, kind: Kind) -> Kinds {
    match kind {
        Kind::Topic => Kinds {
            topic: !kinds.topic,
            ..kinds
        },
        Kind::Fact => Kinds {
            fact: !kinds.fact,
            ..kinds
        },
        Kind::Inference => Kinds {
            inference: !kinds.inference,
            ..kinds
        },
    }
}

/// Whether a control shows as chosen.
fn is_selected(
    control: &Control,
    filter: &Filter,
    catalogue: &Catalogue,
    open: OpenPicker,
    edges: AllEdges,
    mode: TimeMode,
) -> bool {
    match control {
        Control::RecordedTime => mode == TimeMode::Recorded,
        Control::ClaimTime => mode == TimeMode::Claim,
        Control::Kind(kind) => kind_passes(&filter.kinds, *kind),
        Control::Mode => filter.mode == Mode::Hide,
        Control::AllEdges => edges.0,
        Control::Open(picker) => open.0 == Some(*picker),
        Control::AllMetacontexts => filter.metacontexts == Metacontexts::All,
        Control::Session(id) => *id == catalogue.session.session_id,
        Control::Graph(graph) => {
            *graph == catalogue.graph && catalogue.browsing == catalogue.session.session_id
        }
        Control::Topic(id) => filter.topic.as_deref() == Some(id.as_str()),
        Control::Source(id) => filter.source.as_deref() == Some(id.as_str()),
        _ => false,
    }
}

/// What the controls change.
#[derive(SystemParam)]
struct Act<'w> {
    filter: ResMut<'w, Filter>,
    open: ResMut<'w, OpenPicker>,
    collapsed: ResMut<'w, Collapsed>,
    edges: ResMut<'w, AllEdges>,
    mode: ResMut<'w, TimeMode>,
    catalogue: ResMut<'w, Catalogue>,
    pickers: Res<'w, Pickers>,
    views: MessageWriter<'w, ViewRequest>,
    loads: MessageWriter<'w, LoadGraph>,
    search: MessageWriter<'w, SearchAction>,
    recalculate: MessageWriter<'w, RecalculateLayout>,
}

/// Does what a control does, once per click. The observer sees every
/// click, as it climbs from the node clicked to its ancestors; the first
/// node on the way that is a control takes it.
fn on_click(
    mut click: On<Pointer<Click>>,
    controls: Query<(&Control, Has<InteractionDisabled>)>,
    mut act: Act,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok((control, disabled)) = controls.get(click.entity) else {
        return;
    };
    click.propagate(false);
    if !disabled {
        perform(control, &mut act);
    }
}

fn perform(control: &Control, act: &mut Act) {
    match control {
        Control::Collapse => act.collapsed.0 = true,
        Control::Expand => act.collapsed.0 = false,
        Control::Open(picker) => {
            act.open.0 = (act.open.0 != Some(*picker)).then_some(*picker);
        }
        Control::RecordedTime => {
            act.mode.set_if_neq(TimeMode::Recorded);
        }
        Control::ClaimTime => {
            act.mode.set_if_neq(TimeMode::Claim);
        }
        Control::Recalculate => {
            act.recalculate.write(RecalculateLayout {
                reason: "asked for, by the bar's button".to_owned(),
            });
        }
        Control::Kind(kind) => act.filter.kinds = toggle_kind(act.filter.kinds, *kind),
        Control::Mode => {
            act.filter.mode = match act.filter.mode {
                Mode::Dim => Mode::Hide,
                Mode::Hide => Mode::Dim,
            }
        }
        Control::AllEdges => act.edges.0 = !act.edges.0,
        Control::View(request) => {
            act.views.write(*request);
        }
        Control::ClearTopic => {
            if act.filter.topic.is_some() {
                act.filter.topic = None;
            }
        }
        Control::ClearSource => {
            if act.filter.source.is_some() {
                act.filter.source = None;
            }
        }
        Control::ClearWindow => {
            if act.filter.window.is_some() {
                act.filter.window = None;
            }
        }
        Control::SearchGo => {
            act.search.write(SearchAction::Go);
        }
        Control::SearchClear => {
            act.search.write(SearchAction::Clear);
        }
        Control::AllMetacontexts => {
            if act.filter.metacontexts != Metacontexts::All {
                act.filter.metacontexts = Metacontexts::All;
            }
        }
        Control::Metacontext(id) => {
            act.filter.metacontexts =
                toggle_metacontext(&act.filter.metacontexts, id, &act.pickers.metacontexts);
        }
        Control::Session(id) => {
            if act.catalogue.browsing != *id {
                act.catalogue.browsing = id.clone();
                act.catalogue.graphs = None;
            }
            act.open.0 = Some(Picker::Graph);
        }
        Control::Graph(graph) => {
            act.loads.write(LoadGraph {
                session: act.catalogue.browsing.clone(),
                graph: graph.clone(),
            });
            act.open.0 = None;
        }
        Control::Topic(id) => {
            act.filter.topic = Some(id.clone());
            act.open.0 = None;
        }
        Control::Source(id) => {
            act.filter.source = Some(id.clone());
            act.open.0 = None;
        }
    }
}

/// The entity and every ancestor of it.
fn lineage(entity: Entity, parents: &Query<&ChildOf>) -> impl Iterator<Item = Entity> {
    std::iter::once(entity).chain(parents.iter_ancestors(entity))
}

/// The entities under the mouse pointer.
fn hovered(hover: Option<&HoverMap>) -> Vec<Entity> {
    hover
        .and_then(|hover| hover.get(&PointerId::Mouse))
        .map(|hits| hits.keys().copied().collect())
        .unwrap_or_default()
}

/// The fraction of a viewport `viewport` pixels high that a bar `bar`
/// pixels high covers, or `None` before either has been laid out.
pub fn cover_fraction(bar: f32, viewport: f32) -> Option<f32> {
    (bar > 0.0 && viewport > 0.0).then(|| (bar / viewport).min(1.0))
}

/// Tells the camera how much of the viewport the bar, or its strip, covers,
/// so that every view is fitted below it. Both heights are in the target's
/// pixels, so the window's density does not enter.
fn measure_cover(
    bars: Query<&ComputedNode, With<Covers>>,
    cameras: Query<&Camera, With<IsDefaultUiCamera>>,
    mut cover: ResMut<BarCover>,
) {
    let bar = bars.iter().map(|node| node.size().y).fold(0.0, f32::max);
    let viewport = cameras
        .iter()
        .find_map(|camera| camera.physical_viewport_size())
        .map_or(0.0, |size| size.y as f32);
    if let Some(fraction) = cover_fraction(bar, viewport) {
        cover.set_if_neq(BarCover(Some(fraction)));
    }
}

/// What the input focus becomes, from who holds it now, the topic field
/// where its list is open, and whether the pointer is over the UI:
/// `Some` of the new holder where it changes. The topic field takes the
/// keyboard while the pointer is over the UI and lets it go when the
/// pointer moves to the scene; a holder that is no longer a UI node lets it
/// go; anything else keeps it, the search field among them.
pub fn next_input_focus(
    held: Option<Entity>,
    topic_field: Option<Entity>,
    pointer_over_ui: bool,
    is_node: impl Fn(Entity) -> bool,
) -> Option<Option<Entity>> {
    match (topic_field, pointer_over_ui) {
        (Some(field), true) if held != Some(field) => Some(Some(field)),
        (Some(field), false) if held == Some(field) => Some(None),
        _ if held.is_some_and(|e| !is_node(e)) => Some(None),
        _ => None,
    }
}

/// Works out who has the pointer and the keyboard, by
/// [`next_input_focus`]: the pointer is the UI's while it is over the bar,
/// one of its lists or the side panel, and the keyboard is a text field's
/// while one holds the input focus.
fn track_focus(
    hover: Option<Res<HoverMap>>,
    nodes: Query<(), With<ComputedNode>>,
    fields: Query<Entity, With<TopicField>>,
    editable: Query<(), With<EditableText>>,
    input_focus: Option<ResMut<InputFocus>>,
    mut focus: ResMut<UiFocus>,
) {
    let pointer_over_ui = over_ui(hovered(hover.as_deref()), |e| nodes.contains(e));
    let mut typing = false;
    if let Some(mut input_focus) = input_focus {
        let field = fields.iter().next();
        let next = next_input_focus(input_focus.get(), field, pointer_over_ui, |e| {
            nodes.contains(e)
        });
        match next {
            Some(Some(holder)) => input_focus.set(holder, FocusCause::Navigated),
            Some(None) => input_focus.clear(),
            None => {}
        }
        typing = input_focus.get().is_some_and(|e| editable.contains(e));
    }
    focus.set_if_neq(UiFocus {
        pointer_over_ui,
        typing,
    });
}

fn toggle_on_key(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<UiFocus>,
    mut collapsed: ResMut<Collapsed>,
) {
    if !focus.typing && keys.just_pressed(BAR_KEY.0) {
        collapsed.0 = !collapsed.0;
    }
}

/// A left press anywhere but on the open list or its own button closes it.
fn close_on_outside_press(
    buttons: Res<ButtonInput<MouseButton>>,
    hover: Option<Res<HoverMap>>,
    parents: Query<&ChildOf>,
    panels: Query<(), With<Panel>>,
    controls: Query<&Control>,
    mut open: ResMut<OpenPicker>,
) {
    let Some(picker) = open.0 else {
        return;
    };
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let inside = hovered(hover.as_deref()).into_iter().any(|e| {
        lineage(e, &parents)
            .any(|a| panels.contains(a) || controls.get(a) == Ok(&Control::Open(picker)))
    });
    if !inside {
        open.0 = None;
    }
}

fn close_on_escape(keys: Res<ButtonInput<KeyCode>>, mut open: ResMut<OpenPicker>) {
    if open.0.is_some() && keys.just_pressed(KeyCode::Escape) {
        open.0 = None;
    }
}

/// Enter in the topic picker takes the first topic listed.
fn choose_first_topic(
    keys: Res<ButtonInput<KeyCode>>,
    prefix: Res<TopicPrefix>,
    pickers: Res<Pickers>,
    mut open: ResMut<OpenPicker>,
    mut filter: ResMut<Filter>,
) {
    if open.0 != Some(Picker::Topic)
        || !keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter])
    {
        return;
    }
    if let Some(first) = pickers::matching(&pickers.topics, &prefix.0).first() {
        filter.topic = Some(first.id.clone());
        open.0 = None;
    }
}

/// Opening the session picker reads the hub's sessions afresh, and opening
/// the graph picker reads the chosen session's graphs, each in the background.
/// Closing the topic picker forgets what was typed into it.
fn start_fetches(
    open: Res<OpenPicker>,
    catalogue: Res<Catalogue>,
    mut fetches: ResMut<Fetches>,
    mut prefix: ResMut<TopicPrefix>,
    mut notice: ResMut<Notice>,
) {
    if !open.is_changed() {
        return;
    }
    if open.0 != Some(Picker::Topic) && !prefix.0.is_empty() {
        prefix.0.clear();
    }
    let hub = catalogue.hub.clone();
    let started = match open.0 {
        Some(Picker::Session) => in_background("read the sessions", move || async move {
            hub::sessions(&hub).await.map_err(|e| e.to_string())
        })
        .map(|pending| fetches.sessions = Some(pending)),
        Some(Picker::Graph) => {
            let session = catalogue.browsing.clone();
            let asked = session.clone();
            in_background("read the graphs", move || async move {
                hub::graphs(&hub, &asked).await.map_err(|e| e.to_string())
            })
            .map(|pending| fetches.graphs = Some((session, pending)))
        }
        _ => Ok(()),
    };
    if let Err(message) = started {
        notice.0 = Some(message);
    }
}

/// What a list read that ended without an answer says.
const LOST: &str = "the read stopped without an answer";

/// Takes the answers of the list reads that have come.
fn receive_fetches(
    mut fetches: ResMut<Fetches>,
    mut catalogue: ResMut<Catalogue>,
    mut notice: ResMut<Notice>,
) {
    if let Some(pending) = fetches.sessions.as_ref() {
        let answer = match poll(pending) {
            Poll::Waiting => None,
            Poll::Ready(answer) => Some(answer),
            Poll::Lost => Some(Err(LOST.to_owned())),
        };
        if let Some(answer) = answer {
            fetches.sessions = None;
            match answer {
                Ok(sessions) => {
                    if catalogue.sessions != sessions {
                        catalogue.sessions = sessions;
                    }
                    notice.0 = None;
                }
                Err(message) => notice.0 = Some(format!("Could not list the sessions: {message}")),
            }
        }
    }
    if let Some((session, pending)) = fetches.graphs.as_ref() {
        let answer = match poll(pending) {
            Poll::Waiting => None,
            Poll::Ready(answer) => Some(answer),
            Poll::Lost => Some(Err(LOST.to_owned())),
        };
        if let Some(answer) = answer {
            let session = session.clone();
            fetches.graphs = None;
            match answer {
                Ok(graphs) if session == catalogue.browsing => {
                    catalogue.graphs = Some(graphs);
                    notice.0 = None;
                }
                Ok(_) => {}
                Err(message) => {
                    notice.0 = Some(format!(
                        "Could not list the graphs of session {}: {message}",
                        short_id(&session)
                    ));
                }
            }
        }
    }
}

fn show_collapsed(
    collapsed: Res<Collapsed>,
    mut open: ResMut<OpenPicker>,
    mut bar: Query<&mut Node, (With<BarRoot>, Without<Strip>)>,
    mut strip: Query<&mut Node, (With<Strip>, Without<BarRoot>)>,
) {
    if !collapsed.is_changed() {
        return;
    }
    let (shown, hidden) = (Display::Flex, Display::None);
    for mut node in &mut bar {
        node.display = if collapsed.0 { hidden } else { shown };
    }
    for mut node in &mut strip {
        node.display = if collapsed.0 { shown } else { hidden };
    }
    if collapsed.0 && open.0.is_some() {
        open.0 = None;
    }
}

/// What a picker's list is made from.
#[derive(SystemParam)]
struct PanelData<'w> {
    open: Res<'w, OpenPicker>,
    catalogue: Res<'w, Catalogue>,
    pickers: Res<'w, Pickers>,
    prefix: Res<'w, TopicPrefix>,
    fetches: Res<'w, Fetches>,
    notice: Res<'w, Notice>,
}

/// Rebuilds the open picker's list when the picker opens, or when what it
/// lists changes: a session or graph list read, or a new graph drawn.
fn rebuild_panel(
    mut commands: Commands,
    data: PanelData,
    panels: Query<Entity, With<Panel>>,
    anchors: Query<(Entity, &Anchor)>,
) {
    let lists_hub = matches!(data.open.0, Some(Picker::Session | Picker::Graph));
    let changed = data.open.is_changed()
        || data.pickers.is_changed()
        || (lists_hub && (data.catalogue.is_changed() || data.fetches.is_changed()));
    if !changed {
        return;
    }
    for panel in &panels {
        commands.entity(panel).despawn();
    }
    let Some(picker) = data.open.0 else {
        return;
    };
    let Some((anchor, _)) = anchors.iter().find(|(_, a)| a.0 == picker) else {
        return;
    };
    let content = panels::Content {
        catalogue: &data.catalogue,
        pickers: &data.pickers,
        prefix: &data.prefix.0,
        reading_sessions: data.fetches.sessions.is_some(),
        reading_graphs: data.fetches.graphs.is_some(),
        notice: data.notice.0.as_deref(),
    };
    panels::spawn(&mut commands, anchor, picker, &content);
}

/// Copies what is typed into the topic field into [`TopicPrefix`].
fn read_topic_field(
    fields: Query<&EditableText, With<TopicField>>,
    mut prefix: ResMut<TopicPrefix>,
) {
    let Some(field) = fields.iter().next() else {
        return;
    };
    let typed = field.value().to_string();
    if prefix.0 != typed {
        prefix.0 = typed;
    }
}

/// Shows the topic rows that match what is typed, and the count of them.
fn narrow_topic_rows(
    prefix: Res<TopicPrefix>,
    pickers: Res<Pickers>,
    mut rows: Query<(&TopicRow, &mut Node)>,
    added: Query<(), Added<TopicRow>>,
    mut count: Query<&mut Text, With<TopicCount>>,
) {
    if !prefix.is_changed() && added.is_empty() {
        return;
    }
    let mut shown = 0;
    for (row, mut node) in &mut rows {
        let matches = pickers
            .topics
            .get(row.0)
            .is_some_and(|t| pickers::matches(&t.label, &prefix.0));
        shown += usize::from(matches);
        let display = if matches {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for mut text in &mut count {
        let words = panels::topic_count(shown, pickers.topics.len(), &prefix.0);
        if text.0 != words {
            text.0 = words;
        }
    }
}

/// What styling a control reads and writes.
type StyledControl = (
    &'static Control,
    &'static Interaction,
    &'static mut BackgroundColor,
    Has<InteractionDisabled>,
    Has<Row>,
);

/// Colours every control by whether it is chosen, under the pointer, or
/// unable to do anything yet.
fn style_controls(
    filter: Res<Filter>,
    catalogue: Res<Catalogue>,
    open: Res<OpenPicker>,
    edges: Res<AllEdges>,
    mode: Res<TimeMode>,
    mut controls: Query<StyledControl>,
) {
    for (control, interaction, mut background, disabled, row) in &mut controls {
        let rest = if row { style::ROW } else { style::BUTTON };
        let colour = style::button_colour(
            rest,
            is_selected(control, &filter, &catalogue, *open, *edges, *mode),
            *interaction != Interaction::None,
            disabled,
        );
        background.set_if_neq(BackgroundColor(colour));
    }
}

fn style_checkmarks(filter: Res<Filter>, mut marks: Query<(&CheckMark, &mut BackgroundColor)>) {
    for (mark, mut background) in &mut marks {
        let colour = if metacontext_ticked(&filter.metacontexts, &mark.0) {
            style::ACCENT
        } else {
            Color::NONE
        };
        background.set_if_neq(BackgroundColor(colour));
    }
}

/// What the bar's texts read from.
#[derive(SystemParam)]
struct TextData<'w, 's> {
    filter: Res<'w, Filter>,
    catalogue: Res<'w, Catalogue>,
    pickers: Res<'w, Pickers>,
    index: Res<'w, GraphIndex>,
    mode: Res<'w, TimeMode>,
    status: Res<'w, LoadStatus>,
    notice: Res<'w, Notice>,
    map: Option<Res<'w, MapState>>,
    live: Option<Res<'w, LiveState>>,
    hints: Query<'w, 's, (&'static Interaction, &'static Hint)>,
}

/// Keeps every text the bar shows up to date, writing only those that
/// change.
fn update_texts(data: TextData, mut texts: Query<(&BarText, &mut Text, &mut TextColor)>) {
    let hint = data
        .hints
        .iter()
        .find(|(interaction, _)| **interaction != Interaction::None)
        .map(|(_, hint)| hint.0);
    for (which, mut text, mut colour) in &mut texts {
        let (words, tone) = match which {
            BarText::Session => (
                format!(
                    "{} {}",
                    short_id(&data.catalogue.session.session_id),
                    data.catalogue.session.backend
                ),
                style::TEXT,
            ),
            BarText::Graph => (data.catalogue.graph.clone(), style::TEXT),
            BarText::Metacontexts => (
                metacontexts_label(&data.filter.metacontexts, &data.pickers.metacontexts),
                style::TEXT,
            ),
            BarText::Topic => (
                topic_label(data.filter.topic.as_deref(), &data.pickers),
                style::TEXT,
            ),
            BarText::Source => (
                data.filter
                    .source
                    .as_deref()
                    .map_or_else(|| "any".to_owned(), |id| short_id(id).to_owned()),
                style::TEXT,
            ),
            BarText::Mode => (
                match data.filter.mode {
                    Mode::Dim => "dim",
                    Mode::Hide => "hide",
                }
                .to_owned(),
                style::TEXT,
            ),
            BarText::Window => (
                slider::range_text(data.filter.window.as_ref(), &data.index.time),
                style::TEXT,
            ),
            BarText::WindowCaption => (
                match *data.mode {
                    TimeMode::Recorded => "Recorded",
                    TimeMode::Claim => "Claim time",
                }
                .to_owned(),
                style::CAPTION,
            ),
            BarText::Live => {
                let (words, error) = data.live.as_deref().map_or_else(
                    || ("off".to_owned(), false),
                    |state| live::status_text(state, data.catalogue.session.connected),
                );
                (words, if error { style::ERROR } else { style::TEXT })
            }
            BarText::Message => {
                let map = data.map.as_deref().map(relayout::bar_note);
                let map = map.as_ref().map(|(words, error)| (words.as_str(), *error));
                let (words, error) = message(hint, &data.status, data.notice.0.as_deref(), map);
                (words, if error { style::ERROR } else { style::CAPTION })
            }
        };
        if text.0 != words {
            text.0 = words;
        }
        colour.set_if_neq(TextColor(tone));
    }
}

/// The message line: a hovered control's hint first, then a load under
/// way or failed, then a list read that failed, then what is drawn, with
/// what the topic map says, which may be an error. The flag says whether
/// it is an error.
pub fn message(
    hint: Option<&str>,
    status: &LoadStatus,
    notice: Option<&str>,
    map: Option<(&str, bool)>,
) -> (String, bool) {
    if let Some(hint) = hint {
        return (hint.to_owned(), false);
    }
    match (status, notice) {
        (LoadStatus::Loaded { graph, nodes }, None) if map.is_some() => {
            let (words, error) = map.unwrap_or_default();
            (format!("{graph}: {nodes} nodes; {words}"), error)
        }
        (LoadStatus::Loading { graph, session }, _) => (
            format!(
                "Loading graph {graph} from session {}...",
                short_id(session)
            ),
            false,
        ),
        (LoadStatus::Failed(message), _) => (format!("Could not load the graph: {message}"), true),
        (_, Some(notice)) => (notice.to_owned(), true),
        (LoadStatus::Loaded { graph, nodes }, None) => (format!("{graph}: {nodes} nodes"), false),
        (LoadStatus::Idle, None) => (String::new(), false),
    }
}

/// The metacontext picker's button: all, one by name, or how many of how
/// many.
pub fn metacontexts_label(current: &Metacontexts, listed: &[MetacontextEntry]) -> String {
    match current {
        Metacontexts::All => "all".to_owned(),
        Metacontexts::Only(ids) if ids.is_empty() => "none".to_owned(),
        Metacontexts::Only(ids) if ids.len() == 1 => {
            let id = ids.iter().next().map_or("", String::as_str);
            listed
                .iter()
                .find(|m| m.id == id)
                .map_or(id, |m| m.name.as_str())
                .to_owned()
        }
        Metacontexts::Only(ids) => format!("{} of {}", ids.len(), listed.len()),
    }
}

/// The topic picker's button: the topic chosen, shortened, or any.
pub fn topic_label(topic: Option<&str>, pickers: &Pickers) -> String {
    const MOST: usize = 28;
    let Some(id) = topic else {
        return "any".to_owned();
    };
    let label = pickers
        .topics
        .iter()
        .find(|t| t.id == id)
        .map_or_else(|| short_id(id).to_owned(), |t| t.label.clone());
    if label.chars().count() > MOST {
        format!("{}...", label.chars().take(MOST - 3).collect::<String>())
    } else {
        label
    }
}

/// A caption before a group of controls.
fn caption(words: &str) -> impl Bundle {
    style::text(words, style::CAPTION)
}

/// A button with fixed words.
fn button(words: &str, control: Control) -> impl Bundle {
    (
        control,
        Button,
        style::button_node(),
        BackgroundColor(style::BUTTON),
        children![style::text(words, style::TEXT)],
    )
}

/// A button whose words the bar keeps up to date.
fn live_button(text: BarText, control: Control) -> impl Bundle {
    (
        control,
        Button,
        style::button_node(),
        BackgroundColor(style::BUTTON),
        children![(text, style::text("", style::TEXT))],
    )
}

/// A button with fixed words that says what it does when hovered.
fn hinted_button(words: &str, control: Control, hint: &'static str) -> impl Bundle {
    (
        control,
        Button,
        Hint(hint),
        style::button_node(),
        BackgroundColor(style::BUTTON),
        children![style::text(words, style::TEXT)],
    )
}

/// A picker's button, with the node its list opens under.
fn opener(picker: Picker, text: BarText) -> impl Bundle {
    (
        Anchor(picker),
        Node::default(),
        children![live_button(text, Control::Open(picker))],
    )
}

fn kind_toggle(kind: Kind, words: &str) -> impl Bundle {
    (
        Control::Kind(kind),
        Button,
        style::button_node(),
        BackgroundColor(style::BUTTON),
        children![
            style::swatch(kind_colour(kind).with_alpha(1.0)),
            style::text(words, style::TEXT)
        ],
    )
}

/// The search field, with what `--search` typed into it.
fn search_field(typed: &str) -> impl Bundle {
    (
        SearchField,
        Node {
            width: px(SEARCH_WIDTH),
            padding: UiRect::axes(px(5), px(3)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(3)),
            ..default()
        },
        EditableText::new(typed),
        TextFont {
            font_size: FontSize::Px(style::FONT_SIZE),
            ..default()
        },
        TextColor(style::TEXT),
        TextLayout::no_wrap(),
        TextCursorStyle {
            color: style::TEXT,
            ..default()
        },
        BackgroundColor(style::BUTTON),
        BorderColor::all(style::PANEL_EDGE),
    )
}

fn spawn_bar(mut commands: Commands, collapsed: Res<Collapsed>, search: Option<Res<Search>>) {
    let display = |shown: bool| if shown { Display::Flex } else { Display::None };
    let typed = search.map(|s| s.text.clone()).unwrap_or_default();
    commands.spawn((
        BarRoot,
        Covers,
        Node {
            display: display(!collapsed.0),
            position_type: PositionType::Absolute,
            top: px(0),
            left: px(0),
            right: px(0),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center,
            column_gap: px(16),
            row_gap: px(6),
            padding: UiRect::axes(px(10), px(6)),
            ..default()
        },
        BackgroundColor(style::BAR),
        GlobalZIndex(10),
        children![
            (
                style::group_node(),
                children![
                    caption("Session"),
                    opener(Picker::Session, BarText::Session)
                ],
            ),
            (
                style::group_node(),
                children![caption("Graph"), opener(Picker::Graph, BarText::Graph)],
            ),
            (
                style::group_node(),
                Interaction::default(),
                Hint(LIVE_HINT),
                children![
                    caption("Stream"),
                    (BarText::Live, style::text("", style::TEXT))
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Time"),
                    (
                        button("Recorded", Control::RecordedTime),
                        Hint(RECORDED_HINT)
                    ),
                    (button("Claim", Control::ClaimTime), Hint(CLAIM_TIME_HINT)),
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Metacontexts"),
                    opener(Picker::Metacontext, BarText::Metacontexts),
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Kinds"),
                    kind_toggle(Kind::Topic, "Topics"),
                    kind_toggle(Kind::Fact, "Facts"),
                    kind_toggle(Kind::Inference, "Inferences"),
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Status"),
                    (
                        Button,
                        Hint(STATUS_HINT),
                        style::button_node(),
                        BackgroundColor(style::DISABLED),
                        children![style::text("Active", style::TEXT)],
                    ),
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Topic"),
                    opener(Picker::Topic, BarText::Topic),
                    button("Clear", Control::ClearTopic),
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Source"),
                    opener(Picker::Source, BarText::Source),
                    button("Clear", Control::ClearSource),
                ],
            ),
            (
                style::group_node(),
                children![
                    (
                        BarText::WindowCaption,
                        style::text("Recorded", style::CAPTION)
                    ),
                    slider::slider(),
                    (BarText::Window, style::text("", style::TEXT)),
                    button("Clear", Control::ClearWindow),
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Filtered out"),
                    live_button(BarText::Mode, Control::Mode)
                ],
            ),
            (
                style::group_node(),
                children![
                    caption("Edges"),
                    (
                        button(&format!("All ({})", EDGE_KEY.1), Control::AllEdges),
                        Hint(EDGES_HINT)
                    ),
                ],
            ),
            (
                style::group_node(),
                Interaction::default(),
                Hint(SEARCH_HINT),
                children![
                    caption(&format!("Search ({})", SEARCH_KEY.1)),
                    search_field(&typed),
                    (SearchCount, style::text("", style::CAPTION)),
                    button(&format!("Go ({})", GO_KEY.1), Control::SearchGo),
                    button("Clear", Control::SearchClear),
                ],
            ),
            (
                style::group_node(),
                Children::spawn((
                    Spawn(caption("View")),
                    SpawnIter(
                        VIEW_KEYS
                            .iter()
                            .map(|key| button(key.button, Control::View(key.request))),
                    ),
                )),
            ),
            hinted_button(
                &format!("Recalculate layout ({})", RECALCULATE_KEY.1),
                Control::Recalculate,
                RECALCULATE_HINT,
            ),
            button("Hide bar (H)", Control::Collapse),
            (BarText::Message, style::text("", style::CAPTION)),
        ],
    ));
    commands.spawn((
        Strip,
        Covers,
        Node {
            display: display(collapsed.0),
            position_type: PositionType::Absolute,
            top: px(0),
            left: px(0),
            right: px(0),
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(10), px(2)),
            ..default()
        },
        BackgroundColor(style::BAR),
        GlobalZIndex(10),
        children![button("Show bar (H)", Control::Expand)],
    ));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::Location;

    use super::*;
    use crate::pickers::MetacontextEntry;
    use crate::presets::Preset;

    fn session(id: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            pid: 1,
            backend: "surrealdb".into(),
            active_graph: "memory".into(),
            started_at: "2026-10-20T10:00:00Z".into(),
            connected: true,
            last_event_at: None,
            schema_version: Some(1),
        }
    }

    /// The bar's controls and the click observer, with no window and no
    /// renderer.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<ViewRequest>()
            .add_message::<LoadGraph>()
            .add_message::<SearchAction>()
            .add_message::<RecalculateLayout>()
            .init_resource::<Filter>()
            .init_resource::<OpenPicker>()
            .init_resource::<Collapsed>()
            .init_resource::<AllEdges>()
            .init_resource::<TimeMode>()
            .insert_resource(Pickers {
                metacontexts: listed(&["a", "b"]),
                ..Pickers::default()
            })
            .insert_resource(Catalogue {
                hub: "127.0.0.1:1".into(),
                sessions: vec![session("s1"), session("s2")],
                session: session("s1"),
                graph: "memory".into(),
                browsing: "s1".into(),
                graphs: None,
            })
            .add_observer(on_click)
            .add_systems(Startup, spawn_bar);
        // A click climbs through `PointerTraversal`, which looks for a
        // window too, so the component must be known even with no window.
        app.world_mut().register_component::<Window>();
        app.update();
        app
    }

    /// A left click on an entity, as the picking backend sends it.
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

    /// The bar's control that does this, which must be the only one.
    fn control(app: &mut App, wanted: &Control) -> Entity {
        let mut query = app.world_mut().query::<(Entity, &Control)>();
        let found: Vec<Entity> = query
            .iter(app.world())
            .filter(|(_, c)| *c == wanted)
            .map(|(e, _)| e)
            .collect();
        assert_eq!(found.len(), 1, "{wanted:?}");
        found[0]
    }

    fn spawn(app: &mut App, control: Control) -> Entity {
        app.world_mut().spawn(control).id()
    }

    #[test]
    fn a_view_button_sends_what_its_key_sends() {
        let mut app = app();
        for key in &VIEW_KEYS {
            let button = control(&mut app, &Control::View(key.request));
            // The click lands on the button's text and climbs to the button.
            let text = app.world().get::<Children>(button).unwrap()[0];
            click(&mut app, text);
            let sent: Vec<ViewRequest> = app
                .world_mut()
                .resource_mut::<Messages<ViewRequest>>()
                .drain()
                .collect();
            assert_eq!(sent, [key.request], "{}", key.button);
        }
        assert!(
            VIEW_KEYS
                .iter()
                .any(|k| k.request == ViewRequest::Preset(Preset::Now))
        );
    }

    #[test]
    fn the_filter_controls_change_the_filter() {
        let mut app = app();
        let facts = control(&mut app, &Control::Kind(Kind::Fact));
        click(&mut app, facts);
        assert!(!app.world().resource::<Filter>().kinds.fact);
        let mode = control(&mut app, &Control::Mode);
        click(&mut app, mode);
        assert_eq!(app.world().resource::<Filter>().mode, Mode::Hide);
        let edges = control(&mut app, &Control::AllEdges);
        let words = app.world().get::<Children>(edges).unwrap()[0];
        assert_eq!(
            app.world().get::<Text>(words).unwrap().0,
            format!("All ({})", EDGE_KEY.1)
        );
        click(&mut app, edges);
        assert!(app.world().resource::<AllEdges>().0, "the edge toggle on");
        click(&mut app, edges);
        assert!(!app.world().resource::<AllEdges>().0, "and off");

        let row = spawn(&mut app, Control::Metacontext("a".into()));
        click(&mut app, row);
        assert_eq!(app.world().resource::<Filter>().metacontexts, only(&["b"]));
        let all = spawn(&mut app, Control::AllMetacontexts);
        click(&mut app, all);
        assert_eq!(
            app.world().resource::<Filter>().metacontexts,
            Metacontexts::All
        );

        app.world_mut().resource_mut::<OpenPicker>().0 = Some(Picker::Topic);
        let topic = spawn(&mut app, Control::Topic("t1".into()));
        click(&mut app, topic);
        assert_eq!(
            app.world().resource::<Filter>().topic.as_deref(),
            Some("t1")
        );
        assert_eq!(
            app.world().resource::<OpenPicker>().0,
            None,
            "chosen, closed"
        );
        let clear = control(&mut app, &Control::ClearTopic);
        click(&mut app, clear);
        assert_eq!(app.world().resource::<Filter>().topic, None);

        let source = spawn(&mut app, Control::Source("doc".into()));
        click(&mut app, source);
        assert_eq!(
            app.world().resource::<Filter>().source.as_deref(),
            Some("doc")
        );
        let clear = control(&mut app, &Control::ClearSource);
        click(&mut app, clear);
        assert_eq!(app.world().resource::<Filter>().source, None);
    }

    #[test]
    fn a_session_opens_its_graphs_and_a_graph_asks_for_a_load() {
        let mut app = app();
        let opener = control(&mut app, &Control::Open(Picker::Session));
        click(&mut app, opener);
        assert_eq!(
            app.world().resource::<OpenPicker>().0,
            Some(Picker::Session)
        );
        click(&mut app, opener);
        assert_eq!(
            app.world().resource::<OpenPicker>().0,
            None,
            "pressed again"
        );

        let other = spawn(&mut app, Control::Session("s2".into()));
        click(&mut app, other);
        assert_eq!(app.world().resource::<OpenPicker>().0, Some(Picker::Graph));
        assert_eq!(app.world().resource::<Catalogue>().browsing, "s2");
        let graph = spawn(&mut app, Control::Graph("games".into()));
        click(&mut app, graph);
        let asked: Vec<LoadGraph> = app
            .world_mut()
            .resource_mut::<Messages<LoadGraph>>()
            .drain()
            .collect();
        assert_eq!(
            asked,
            [LoadGraph {
                session: "s2".into(),
                graph: "games".into()
            }]
        );
        assert_eq!(app.world().resource::<OpenPicker>().0, None);
    }

    #[test]
    fn recalculate_asks_for_a_new_map_and_the_bar_collapses_and_returns() {
        let mut app = app();
        let recalculate = control(&mut app, &Control::Recalculate);
        assert!(
            app.world()
                .get::<InteractionDisabled>(recalculate)
                .is_none()
        );
        let before = app.world().resource::<Filter>().clone();
        click(&mut app, recalculate);
        assert_eq!(*app.world().resource::<Filter>(), before);
        let asked: Vec<RecalculateLayout> = app
            .world_mut()
            .resource_mut::<Messages<RecalculateLayout>>()
            .drain()
            .collect();
        assert_eq!(asked.len(), 1);

        let collapse = control(&mut app, &Control::Collapse);
        click(&mut app, collapse);
        assert!(app.world().resource::<Collapsed>().0);
        let expand = control(&mut app, &Control::Expand);
        click(&mut app, expand);
        assert!(!app.world().resource::<Collapsed>().0);
    }

    #[test]
    fn the_time_buttons_switch_the_mode_and_show_which_is_drawn() {
        let mut app = app();
        let claim = control(&mut app, &Control::ClaimTime);
        let recorded = control(&mut app, &Control::RecordedTime);
        assert!(app.world().get::<InteractionDisabled>(claim).is_none());
        assert_eq!(*app.world().resource::<TimeMode>(), TimeMode::Recorded);
        click(&mut app, claim);
        assert_eq!(*app.world().resource::<TimeMode>(), TimeMode::Claim);
        let (filter, catalogue) = (
            Filter::default(),
            app.world().resource::<Catalogue>().clone(),
        );
        let selected = |control: &Control, mode| {
            is_selected(
                control,
                &filter,
                &catalogue,
                OpenPicker(None),
                AllEdges(false),
                mode,
            )
        };
        assert!(selected(&Control::ClaimTime, TimeMode::Claim));
        assert!(!selected(&Control::RecordedTime, TimeMode::Claim));
        click(&mut app, recorded);
        assert_eq!(*app.world().resource::<TimeMode>(), TimeMode::Recorded);
        assert!(selected(&Control::RecordedTime, TimeMode::Recorded));
    }

    fn listed(ids: &[&str]) -> Vec<MetacontextEntry> {
        ids.iter()
            .map(|&id| MetacontextEntry {
                id: id.into(),
                name: id.to_uppercase(),
                nodes: 1,
            })
            .collect()
    }

    fn only(ids: &[&str]) -> Metacontexts {
        Metacontexts::Only(ids.iter().map(|&s| s.to_owned()).collect())
    }

    #[test]
    fn a_metacontext_box_leaves_all_ticks_back_and_every_box_is_all() {
        let listed = listed(&["a", "b", "c"]);
        let all = Metacontexts::All;
        assert_eq!(toggle_metacontext(&all, "b", &listed), only(&["a", "c"]));
        assert_eq!(
            toggle_metacontext(&only(&["a", "c"]), "a", &listed),
            only(&["c"])
        );
        assert_eq!(toggle_metacontext(&only(&["c"]), "c", &listed), only(&[]));
        assert_eq!(toggle_metacontext(&only(&["a", "c"]), "b", &listed), all);
        assert!(metacontext_ticked(&all, "z"));
        assert!(metacontext_ticked(&only(&["a"]), "a") && !metacontext_ticked(&only(&["a"]), "b"));
    }

    #[test]
    fn the_metacontext_button_names_what_passes() {
        let listed = listed(&["a", "b", "c"]);
        assert_eq!(metacontexts_label(&Metacontexts::All, &listed), "all");
        assert_eq!(metacontexts_label(&only(&["b"]), &listed), "B");
        assert_eq!(metacontexts_label(&only(&["gone"]), &listed), "gone");
        assert_eq!(metacontexts_label(&only(&["a", "c"]), &listed), "2 of 3");
        assert_eq!(metacontexts_label(&only(&[]), &listed), "none");
    }

    #[test]
    fn a_kind_toggle_flips_that_kind_only() {
        let kinds = toggle_kind(Kinds::default(), Kind::Fact);
        assert_eq!(
            kinds,
            Kinds {
                topic: true,
                fact: false,
                inference: true
            }
        );
        assert_eq!(toggle_kind(kinds, Kind::Fact), Kinds::default());
    }

    #[test]
    fn the_message_line_puts_a_hint_first_and_an_error_before_what_is_drawn() {
        let loaded = LoadStatus::Loaded {
            graph: "memory".into(),
            nodes: 2097,
        };
        assert_eq!(
            message(None, &loaded, None, None),
            ("memory: 2097 nodes".into(), false)
        );
        assert_eq!(
            message(Some("why"), &loaded, Some("bad"), None),
            ("why".into(), false)
        );
        assert_eq!(
            message(None, &loaded, Some("bad"), None),
            ("bad".into(), true)
        );
        assert_eq!(
            message(None, &loaded, None, Some(("No topic map: old hub.", true))),
            ("memory: 2097 nodes; No topic map: old hub.".into(), true)
        );
        assert_eq!(
            message(
                None,
                &loaded,
                None,
                Some(("topic map of 433 topics", false))
            ),
            ("memory: 2097 nodes; topic map of 433 topics".into(), false)
        );
        let loading = LoadStatus::Loading {
            graph: "games".into(),
            session: "6af12772cc2f".into(),
        };
        assert_eq!(
            message(None, &loading, None, Some(("map", false))).0,
            "Loading graph games from session 6af12772..."
        );
        let failed = LoadStatus::Failed("no hub".into());
        assert_eq!(
            message(None, &failed, None, None),
            ("Could not load the graph: no hub".into(), true)
        );
    }

    #[test]
    fn the_topic_button_shows_the_topic_shortened() {
        let pickers = Pickers {
            topics: vec![
                pickers::TopicEntry {
                    index: 0,
                    id: "t1".into(),
                    label: "visualisation".into(),
                },
                pickers::TopicEntry {
                    index: 1,
                    id: "t2".into(),
                    label: "Visualisation contract work for a second client".into(),
                },
            ],
            ..Pickers::default()
        };
        assert_eq!(topic_label(None, &pickers), "any");
        assert_eq!(topic_label(Some("t1"), &pickers), "visualisation");
        assert_eq!(
            topic_label(Some("t2"), &pickers),
            "Visualisation contract wo..."
        );
        assert_eq!(topic_label(Some("0123456789ab"), &pickers), "01234567");
    }

    #[test]
    fn the_search_buttons_ask_the_search_for_what_they_do() {
        let mut app = app();
        for (wanted, action) in [
            (Control::SearchGo, SearchAction::Go),
            (Control::SearchClear, SearchAction::Clear),
        ] {
            let button = control(&mut app, &wanted);
            click(&mut app, button);
            let sent: Vec<SearchAction> = app
                .world_mut()
                .resource_mut::<Messages<SearchAction>>()
                .drain()
                .collect();
            assert_eq!(sent, [action]);
        }
        let mut fields = app
            .world_mut()
            .query_filtered::<&EditableText, With<SearchField>>();
        assert_eq!(fields.iter(app.world()).count(), 1, "one search field");
    }

    #[test]
    fn the_topic_field_takes_the_keys_over_the_bar_and_the_search_field_keeps_them() {
        let world = &mut World::new();
        let (topic, search, gone) = (
            world.spawn_empty().id(),
            world.spawn_empty().id(),
            world.spawn_empty().id(),
        );
        let is_node = |e: Entity| e != gone;
        assert_eq!(
            next_input_focus(None, Some(topic), true, is_node),
            Some(Some(topic))
        );
        assert_eq!(
            next_input_focus(Some(topic), Some(topic), true, is_node),
            None
        );
        assert_eq!(
            next_input_focus(Some(topic), Some(topic), false, is_node),
            Some(None),
            "the pointer left the bar"
        );
        assert_eq!(
            next_input_focus(Some(search), None, false, is_node),
            None,
            "the search field keeps the keys over the scene"
        );
        assert_eq!(next_input_focus(Some(search), None, true, is_node), None);
        assert_eq!(
            next_input_focus(Some(gone), None, false, is_node),
            Some(None),
            "a holder that is no longer a node"
        );
        assert_eq!(next_input_focus(None, None, true, is_node), None);
    }

    #[test]
    fn the_cover_is_the_bars_share_of_the_viewport_once_both_are_laid_out() {
        assert_eq!(cover_fraction(144.0, 1440.0), Some(0.1));
        assert_eq!(cover_fraction(0.0, 1440.0), None, "not laid out yet");
        assert_eq!(cover_fraction(50.0, 0.0), None);
        assert_eq!(cover_fraction(900.0, 600.0), Some(1.0));
    }

    #[test]
    fn ui_states_read_as_written() {
        assert_eq!(parse_ui_state("open"), Some(UiState::Open));
        assert_eq!(parse_ui_state("collapsed"), Some(UiState::Collapsed));
        assert_eq!(
            parse_ui_state("topics:vis"),
            Some(UiState::Picker(Picker::Topic, "vis".into()))
        );
        assert_eq!(
            parse_ui_state("topics"),
            Some(UiState::Picker(Picker::Topic, String::new()))
        );
        assert_eq!(
            parse_ui_state("sources"),
            Some(UiState::Picker(Picker::Source, String::new()))
        );
        assert_eq!(parse_ui_state("sources:x"), None, "only topics take text");
        assert_eq!(parse_ui_state("sideways"), None);
    }
}
