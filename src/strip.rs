//! The pipeline strip (DESIGN.md §10): the session's pipelines along the
//! bottom of the window, each a small picture lit by what it is doing.
//!
//! A tile is made for each pipeline the session followed starts, in the
//! order each first started, from the topology its `pipeline_started`
//! carries. Petritype knows a place or a transition by a number, which here
//! is its position in the event's `place_names` or `transition_names`
//! ([`net_of`]); an edge runs from a place to a transition or from a
//! transition to a place, told by which list each end's name is in, and an
//! edge that is neither is logged and left out. The picture is
//! `petritype-activity`'s thumbnail: its model ([`Activity`]) at its default
//! settings, a grid of one cell for each place and transition set out by
//! rank from left to right by `petritype-layout`, and the colours of its
//! dark palette ([`PALETTE`]). This client only feeds the model and draws
//! the colours.
//!
//! `live.rs` hands every pipeline event of the session followed to
//! [`receive_event`], whether the strip is folded or not, and says when
//! events may have been lost ([`events_lost`]): a gap in `seq`, the
//! client's own reconnect, or the followed session's return from a
//! disconnection. What an event does to its tile is a pure function
//! ([`receive`], [`apply`]): a start makes the tile, keeps its picture when
//! the topology is the one it was made from and makes it again when it is
//! not, and either way leaves nothing in flight; a fire launches its
//! transition and takes a token from each input place; a completion lands
//! one in each output place; `tokens_updated` sets every count it names,
//! the only count the strip trusts; a run's end or failure ends every
//! flight and writes the tile's second line ([`status_text`]). An event
//! for a pipeline with no tile is left to the log. Before an event is
//! applied, the tile's model is brought forward to the moment it came
//! ([`bring_forward`]), on Bevy's real clock, the time source the browser
//! build can read.
//!
//! The strip is Bevy UI, under the camera that draws the bar and the side
//! panel. Each tile's picture is an [`Image`] of one pixel a cell, sampled
//! nearest and scaled up whole by an [`ImageNode`], whose bytes are written
//! again only on a frame where a cell's colour changed. Every tile has one
//! cell size ([`cell_size`]). Folded ([`StripFolded`]), by `K` or the bar's
//! Pipelines button, the strip's nodes do not exist and each of its systems
//! returns at once, painting and advancing nothing; unfolding draws every
//! model brought forward to the present. The
//! strip takes no room until a pipeline has started, and while it is open
//! it reports how much of the window's height it covers ([`StripCover`]),
//! so that every view is fitted and every label placed above it.
//!
//! Hovering a cell names its place or transition in the bar's message line,
//! with a place's count ([`StripHover`], [`cell_words`]); a click on a tile
//! opens the log on its Pipelines filter; the wheel over the strip scrolls
//! it sideways where the tiles are wider than the room. A switch to another
//! session empties the strip; a switch of graph does not, since a pipeline
//! event names no graph.

use std::collections::HashMap;
use std::time::Duration;

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::SystemParam;
use bevy::image::ImageSampler;
use bevy::input::mouse::MouseScrollUnit;
use bevy::picking::events::Scroll;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::time::Real;
use bevy::ui::{RelativeCursorPosition, UiSystems};
use petritype::style::Rgb;
use petritype::{EdgeInfo, EdgeKind, PlaceId, TransitionId};
use petritype_activity::{Activity, ActivitySettings, Grid, Palette, Thumbnail};
use petritype_layout::{Direction, Layout, LayoutNode};

use crate::bar::style;
use crate::camera::{PanelCover, StripCover};
use crate::cheatsheet::{HelpOpen, PanelChoice, PanelShows};
use crate::event_log::{LogKind, LogOpen, duration_text};
use crate::keys::STRIP_KEY;
use crate::panel::PanelCollapsed;
use crate::reload::Catalogue;
use crate::stream::{Pipeline, TopologyEdge};
use crate::ui_focus::UiFocus;

/// The palette the pictures are drawn in: Petritype's dark one, which
/// suits the scene's dark background.
pub const PALETTE: Palette = Palette::DARK;
/// The box every pipeline's grid should fit, in logical pixels.
pub const BOX_WIDTH: u32 = 200;
pub const BOX_HEIGHT: u32 = 40;
/// The smallest and the largest size of a cell, in logical pixels.
pub const MIN_CELL: u32 = 2;
pub const MAX_CELL: u32 = 12;
/// The size of a tile's status line; its name is in the bar's size.
const STATUS_FONT_SIZE: f32 = 12.0;

/// What hovering a tile, away from its cells, says in the bar's message
/// line.
pub const TILE_HINT: &str = "A pipeline of the session: a cell for each place and transition, lit as the pipeline runs. A click opens the log on its pipeline runs; K folds the strip away.";

/// A pipeline's net as its start describes it, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topology {
    pub places: Vec<String>,
    pub transitions: Vec<String>,
    pub edges: Vec<TopologyEdge>,
}

/// A pipeline's net in Petritype's terms: each place and transition by
/// the id its position gives, and the edges between them.
#[derive(Clone, Debug, PartialEq)]
pub struct Net {
    pub places: Vec<(PlaceId, String)>,
    pub transitions: Vec<(TransitionId, String)>,
    pub edges: Vec<EdgeInfo>,
    /// Edges that run neither from a place to a transition nor from a
    /// transition to a place, left out.
    pub left_out: Vec<TopologyEdge>,
    place_ids: HashMap<String, PlaceId>,
    transition_ids: HashMap<String, TransitionId>,
}

impl Net {
    /// A place by its name: the first of that name.
    pub fn place(&self, name: &str) -> Option<PlaceId> {
        self.place_ids.get(name).copied()
    }

    /// A transition by its name: the first of that name.
    pub fn transition(&self, name: &str) -> Option<TransitionId> {
        self.transition_ids.get(name).copied()
    }
}

/// Each name with the id its position in the list gives.
fn numbered<Id>(names: &[String], id: impl Fn(u32) -> Id) -> Vec<(Id, String)> {
    names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| Some((id(u32::try_from(index).ok()?), name.clone())))
        .collect()
}

/// The first id of each name.
fn by_name<Id: Copy>(numbered: &[(Id, String)]) -> HashMap<String, Id> {
    let mut ids = HashMap::new();
    for (id, name) in numbered {
        ids.entry(name.clone()).or_insert(*id);
    }
    ids
}

/// The net a topology describes. An edge from a place to a transition is
/// an input that takes a token; one from a transition to a place is an
/// output, numbered among that transition's outputs in the order given;
/// any other edge, between two places, two transitions or a name neither
/// list holds, is left out.
pub fn net_of(topology: &Topology) -> Net {
    let places = numbered(&topology.places, PlaceId::new);
    let transitions = numbered(&topology.transitions, TransitionId::new);
    let place_ids = by_name(&places);
    let transition_ids = by_name(&transitions);
    let mut slots: HashMap<TransitionId, usize> = HashMap::new();
    let mut edges = Vec::new();
    let mut left_out = Vec::new();
    for edge in &topology.edges {
        let input = place_ids
            .get(&edge.source)
            .zip(transition_ids.get(&edge.target));
        let output = transition_ids
            .get(&edge.source)
            .zip(place_ids.get(&edge.target));
        match (input, output) {
            (Some((&place, &transition)), _) => edges.push(EdgeInfo::Input {
                place,
                transition,
                kind: EdgeKind::Take,
                filter: None,
            }),
            (None, Some((&transition, &place))) => {
                let slot = slots.entry(transition).or_default();
                edges.push(EdgeInfo::Output {
                    transition,
                    place,
                    slot: *slot,
                });
                *slot += 1;
            }
            (None, None) => left_out.push(edge.clone()),
        }
    }
    Net {
        places,
        transitions,
        edges,
        left_out,
        place_ids,
        transition_ids,
    }
}

/// What a tile's latest run is doing or did.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Running,
    Done {
        duration_ms: f64,
    },
    Failed,
    /// The stream may have missed the run's end.
    Unknown,
}

/// A tile's second line.
pub fn status_text(status: &Status) -> String {
    match status {
        Status::Running => "running".to_owned(),
        Status::Done { duration_ms } => format!("done in {}", duration_text(*duration_ms)),
        Status::Failed => "failed".to_owned(),
        Status::Unknown => "unknown".to_owned(),
    }
}

/// The colour a tile's second line is written in.
fn status_colour(status: &Status) -> Color {
    match status {
        Status::Running => style::TEXT,
        Status::Done { .. } => style::CAPTION,
        Status::Failed => style::ERROR,
        Status::Unknown => style::FAINT,
    }
}

/// One pipeline's tile: its net, the grid of its picture, and the model
/// that lights it.
#[derive(Clone, Debug)]
pub struct Tile {
    pub name: String,
    /// The topology the picture was made from.
    pub topology: Topology,
    pub net: Net,
    pub grid: Grid,
    pub activity: Activity,
    pub status: Status,
    /// The moment, on the strip's clock, the model was last brought to.
    pub at: Duration,
    /// Which picture this is: a picture made again is a new one.
    pub picture: u64,
}

/// A new tile for a pipeline that has just started, its model at rest.
pub fn new_tile(name: &str, topology: Topology, at: Duration, picture: u64) -> Tile {
    let net = net_of(&topology);
    let mut layout = Layout::default().with_direction(Direction::LeftRight);
    layout.compute_named(
        net.places.clone(),
        net.transitions.clone(),
        net.edges.clone(),
    );
    let grid = Grid::from_layout_named(&layout, net.places.clone(), net.transitions.clone());
    let activity = Activity::named(
        net.places.clone(),
        net.transitions.clone(),
        ActivitySettings::default(),
    );
    Tile {
        name: name.to_owned(),
        topology,
        net,
        grid,
        activity,
        status: Status::Running,
        at,
        picture,
    }
}

/// Advances a tile's model to `now`, on the strip's clock. A moment before
/// the one it stands at leaves it where it is.
pub fn bring_forward(tile: &mut Tile, now: Duration) {
    if let Some(elapsed) = now.checked_sub(tile.at)
        && !elapsed.is_zero()
    {
        tile.activity.tick(elapsed);
        tile.at = now;
    }
}

/// Ends every flight of a tile's model: no transition stays lit as running
/// for a run whose end was not heard.
pub fn end_flights(tile: &mut Tile) {
    for &(transition, _) in &tile.net.transitions {
        tile.activity.set_in_flight(transition, false);
    }
}

/// The pipeline an event is about.
pub fn pipeline_name(event: &Pipeline) -> &str {
    match event {
        Pipeline::Started { pipeline_name, .. }
        | Pipeline::TransitionFired { pipeline_name, .. }
        | Pipeline::TransitionCompleted { pipeline_name, .. }
        | Pipeline::TokensUpdated { pipeline_name, .. }
        | Pipeline::Completed { pipeline_name, .. }
        | Pipeline::Failed { pipeline_name, .. } => pipeline_name,
    }
}

/// Applies an event to its pipeline's tile, whose model has been brought
/// to the moment it came. A name the topology does not hold is passed
/// over. A start is the strip's to handle ([`receive`]), and changes
/// nothing here.
pub fn apply(tile: &mut Tile, event: &Pipeline) {
    match event {
        Pipeline::Started { .. } => {}
        Pipeline::TransitionFired {
            transition_name,
            input_places,
            ..
        } => {
            if let Some(transition) = tile.net.transition(transition_name) {
                tile.activity.launched(transition);
            }
            for place in input_places.iter().filter_map(|p| tile.net.place(p)) {
                tile.activity.taken(place, 1);
            }
            tile.status = Status::Running;
        }
        Pipeline::TransitionCompleted {
            transition_name,
            output_places,
            ..
        } => {
            if let Some(transition) = tile.net.transition(transition_name) {
                tile.activity.distributed(transition);
            }
            for place in output_places.iter().filter_map(|p| tile.net.place(p)) {
                tile.activity.added(place, 1);
            }
        }
        Pipeline::TokensUpdated {
            place_token_counts, ..
        } => {
            for (name, &count) in place_token_counts {
                if let Some(place) = tile.net.place(name) {
                    tile.activity
                        .set_count(place, usize::try_from(count).unwrap_or(0));
                }
            }
        }
        Pipeline::Completed { duration_ms, .. } => {
            end_flights(tile);
            tile.status = Status::Done {
                duration_ms: *duration_ms,
            };
        }
        Pipeline::Failed {
            transition_name, ..
        } => {
            if let Some(transition) = transition_name
                .as_deref()
                .and_then(|name| tile.net.transition(name))
            {
                tile.activity.failed(transition);
            }
            end_flights(tile);
            tile.status = Status::Failed;
        }
    }
}

/// The session's pipelines, each with its tile, in the order each first
/// started.
#[derive(Resource, Clone, Debug, Default)]
pub struct Strip {
    /// The session whose pipelines these are.
    pub session: String,
    pub tiles: Vec<Tile>,
    /// How many pictures have been made, which numbers the next.
    pub made: u64,
}

/// Empties the strip when it holds another session's pipelines; whether it
/// did.
pub fn for_session(strip: &mut Strip, session: &str) -> bool {
    if strip.session == session {
        return false;
    }
    strip.session = session.to_owned();
    strip.tiles.clear();
    true
}

/// What an event did to the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Took {
    /// A start made the pipeline's tile.
    Made,
    /// A start of the topology the picture was made from kept it.
    Kept,
    /// A start of another topology made the picture again.
    MadeAgain,
    /// The event lit the pipeline's tile.
    Applied,
    /// The pipeline has no tile, having started before the client came.
    NoTile,
}

/// What an event did, in a few words for `--log-events`.
pub fn took_text(took: Took) -> &'static str {
    match took {
        Took::Made => "its tile made",
        Took::Kept => "its tile kept",
        Took::MadeAgain => "its tile made again for a new topology",
        Took::Applied => "drawn in its tile",
        Took::NoTile => "no tile, since its start came before the client",
    }
}

/// Takes a pipeline event of `session`, which came at `now` on the strip's
/// clock: the tile's model is brought to that moment first.
pub fn receive(strip: &mut Strip, session: &str, event: &Pipeline, now: Duration) -> Took {
    for_session(strip, session);
    let name = pipeline_name(event);
    let found = strip.tiles.iter().position(|tile| tile.name == name);
    match (event, found) {
        (
            Pipeline::Started {
                place_names,
                transition_names,
                edges,
                ..
            },
            found,
        ) => {
            let topology = Topology {
                places: place_names.clone(),
                transitions: transition_names.clone(),
                edges: edges.clone(),
            };
            match found {
                Some(i) if strip.tiles[i].topology == topology => {
                    let tile = &mut strip.tiles[i];
                    bring_forward(tile, now);
                    end_flights(tile);
                    tile.status = Status::Running;
                    Took::Kept
                }
                Some(i) => {
                    strip.made += 1;
                    strip.tiles[i] = new_tile(name, topology, now, strip.made);
                    Took::MadeAgain
                }
                None => {
                    strip.made += 1;
                    strip.tiles.push(new_tile(name, topology, now, strip.made));
                    Took::Made
                }
            }
        }
        (_, None) => Took::NoTile,
        (_, Some(i)) => {
            let tile = &mut strip.tiles[i];
            bring_forward(tile, now);
            apply(tile, event);
            Took::Applied
        }
    }
}

/// Events may have been lost, at `now`: every flight ends, since its end
/// may have been missed, and a running tile is marked unknown.
pub fn lose(strip: &mut Strip, now: Duration) {
    for tile in &mut strip.tiles {
        bring_forward(tile, now);
        end_flights(tile);
        if tile.status == Status::Running {
            tile.status = Status::Unknown;
        }
    }
}

/// The strip's clock: Bevy's real time since the start, which no frame's
/// length clamps.
fn clock(world: &World) -> Duration {
    world
        .get_resource::<Time<Real>>()
        .map_or(Duration::ZERO, Time::elapsed)
}

/// Hands a pipeline event of the session followed to the strip, logging
/// the edges a new picture leaves out; what it did, or `None` without a
/// strip.
pub fn receive_event(world: &mut World, session: &str, event: &Pipeline) -> Option<Took> {
    let now = clock(world);
    let mut strip = world.get_resource_mut::<Strip>()?;
    let took = receive(&mut strip, session, event, now);
    if matches!(took, Took::Made | Took::MadeAgain)
        && let Some(tile) = strip.tiles.iter().find(|t| t.name == pipeline_name(event))
    {
        for edge in &tile.net.left_out {
            warn!(
                "strip: pipeline {} has an edge from {} to {}, neither a place to a transition nor a transition to a place; it is left out",
                tile.name, edge.source, edge.target
            );
        }
    }
    Some(took)
}

/// Tells the strip that events may have been lost ([`lose`]).
pub fn events_lost(world: &mut World) {
    let now = clock(world);
    if let Some(mut strip) = world.get_resource_mut::<Strip>() {
        lose(&mut strip, now);
    }
}

/// The one cell size of every tile: the largest whole number of pixels from
/// [`MIN_CELL`] to [`MAX_CELL`] at which every grid, as columns and rows,
/// fits the box; [`MIN_CELL`] where no size does.
pub fn cell_size(grids: impl IntoIterator<Item = (usize, usize)> + Clone) -> u32 {
    let fits = |cell: u32| {
        grids.clone().into_iter().all(|(columns, rows)| {
            columns.saturating_mul(cell as usize) <= BOX_WIDTH as usize
                && rows.saturating_mul(cell as usize) <= BOX_HEIGHT as usize
        })
    };
    (MIN_CELL..=MAX_CELL)
        .rev()
        .find(|&cell| fits(cell))
        .unwrap_or(MIN_CELL)
}

/// A picture's size in cells: the grid's, and at least one cell, for a net
/// with nothing in it.
pub fn picture_cells(grid: &Grid) -> (u32, u32) {
    let side = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).max(1);
    (side(grid.columns()), side(grid.rows()))
}

/// A picture's size and its box's, in logical pixels, at a cell size: the
/// box is [`BOX_WIDTH`] by [`BOX_HEIGHT`], or the picture where that is
/// larger.
pub fn picture_size(grid: &Grid, cell: u32) -> (UVec2, UVec2) {
    let (columns, rows) = picture_cells(grid);
    let picture = UVec2::new(columns.saturating_mul(cell), rows.saturating_mul(cell));
    (picture, picture.max(UVec2::new(BOX_WIDTH, BOX_HEIGHT)))
}

/// A palette colour as Bevy's.
pub fn colour(rgb: Rgb) -> Color {
    Color::srgb_u8(rgb.0, rgb.1, rgb.2)
}

/// A tile's picture now: four bytes a cell, row by row from the top left,
/// in the palette's colours; one cell of the page's colour for a net with
/// nothing in it.
pub fn pixels(tile: &Tile) -> Vec<u8> {
    let thumbnail = Thumbnail::new(
        &tile.activity,
        &tile.grid,
        tile.activity.settings().hue,
        &PALETTE,
    );
    let mut bytes: Vec<u8> = thumbnail
        .cells()
        .flat_map(|(_, _, Rgb(r, g, b))| [r, g, b, 255])
        .collect();
    if bytes.is_empty() {
        let Rgb(r, g, b) = PALETTE.page;
        bytes = vec![r, g, b, 255];
    }
    bytes
}

/// A picture of `columns` by `rows` pixels in the page's colour, sampled
/// nearest so that each pixel scales up as a square.
fn blank_image(columns: u32, rows: u32) -> Image {
    let Rgb(r, g, b) = PALETTE.page;
    let mut image = Image::new_fill(
        Extent3d {
            width: columns,
            height: rows,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[r, g, b, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// The cell under a point of a picture, given as the UI gives it: from
/// (-0.5, -0.5) at the top left to (0.5, 0.5) at the bottom right.
pub fn cell_at(normalized: Vec2, columns: usize, rows: usize) -> Option<(usize, usize)> {
    let at = normalized + Vec2::splat(0.5);
    if !(0.0..1.0).contains(&at.x) || !(0.0..1.0).contains(&at.y) {
        return None;
    }
    let column = (at.x * columns as f32).floor() as usize;
    let row = (at.y * rows as f32).floor() as usize;
    (column < columns && row < rows).then_some((column, row))
}

/// What hovering a cell says: the pipeline, and the place with its count or
/// the transition; nothing for a position with no cell.
pub fn cell_words(tile: &Tile, column: usize, row: usize) -> Option<String> {
    let cell = tile.grid.at(column, row)?;
    Some(match cell.node {
        LayoutNode::Place(place) => {
            let count = tile.activity.place(place).map_or(0, |p| p.count);
            let tokens = if count == 1 { "token" } else { "tokens" };
            format!("{}: place {}, {count} {tokens}", tile.name, cell.name)
        }
        LayoutNode::Transition(_) => format!("{}: transition {}", tile.name, cell.name),
    })
}

/// Whether the strip is folded away.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StripFolded(pub bool);

/// What the pointer over the strip says in the bar's message line, if
/// anything.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct StripHover(pub Option<String>);

/// How the strip starts, from `--strip`.
pub const STRIP_STATES: &str = "open or folded";

/// Reads a `--strip` value: whether the strip starts folded.
pub fn parse_strip(text: &str) -> Option<bool> {
    match text {
        "open" => Some(false),
        "folded" => Some(true),
        _ => None,
    }
}

/// The strip's outer node, along the bottom of the window.
#[derive(Component)]
struct StripRoot;

/// The row of tiles, which scrolls sideways.
#[derive(Component)]
struct StripRow;

/// A tile, by its pipeline and the picture it shows.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
struct TileView {
    name: String,
    picture: u64,
}

/// A tile's picture: the image its bytes are written into.
#[derive(Component)]
struct Picture {
    name: String,
    image: Handle<Image>,
}

/// A tile's second line.
#[derive(Component)]
struct TileStatus(String);

/// The strip: tiles from the stream, drawn along the bottom unless folded.
pub struct StripPlugin {
    pub folded: bool,
}

impl Plugin for StripPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(StripFolded(self.folded))
            .init_resource::<Strip>()
            .init_resource::<StripHover>()
            .init_resource::<StripCover>()
            .init_resource::<PanelCover>()
            .init_resource::<UiFocus>()
            .init_resource::<LogOpen>()
            .init_resource::<HelpOpen>()
            .init_resource::<PanelCollapsed>()
            .init_resource::<LogKind>()
            .add_observer(open_log_on_click)
            .add_observer(scroll_sideways)
            .add_systems(
                Update,
                (
                    follow_session,
                    fold_on_key,
                    show_strip,
                    draw_strip,
                    hover_strip,
                    place_beside_panel,
                )
                    .chain(),
            )
            .add_systems(PostUpdate, measure_cover.after(UiSystems::Layout));
    }
}

/// Empties the strip on a switch to another session, which the catalogue
/// names once the switch is drawn.
fn follow_session(
    catalogue: Option<Res<Catalogue>>,
    mut last: Local<Option<String>>,
    mut strip: ResMut<Strip>,
) {
    let Some(catalogue) = catalogue else {
        return;
    };
    let session = &catalogue.session.session_id;
    if last.as_ref() == Some(session) {
        return;
    }
    *last = Some(session.clone());
    if strip.session != *session {
        for_session(&mut strip, session);
    }
}

/// `K` folds the strip away or brings it back, unless a text field has the
/// keyboard.
fn fold_on_key(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<UiFocus>,
    mut folded: ResMut<StripFolded>,
) {
    if !focus.typing && keys.just_pressed(STRIP_KEY.0) {
        folded.0 = !folded.0;
    }
}

/// The strip's nodes as they stand: its outer node, its row and the tiles
/// in it.
#[derive(SystemParam)]
struct StripNodes<'w, 's> {
    roots: Query<'w, 's, Entity, With<StripRoot>>,
    rows: Query<'w, 's, (Entity, Option<&'static Children>), With<StripRow>>,
    views: Query<'w, 's, &'static TileView>,
}

/// Makes the strip's nodes when it is open and has a tile, and makes the
/// tiles again when a pipeline's tile is made or made again; takes them
/// away when it is folded or has none.
fn show_strip(
    mut commands: Commands,
    folded: Res<StripFolded>,
    strip: Res<Strip>,
    panel: Res<PanelCover>,
    nodes: StripNodes,
    mut hover: ResMut<StripHover>,
    mut images: ResMut<Assets<Image>>,
) {
    if folded.0 || strip.tiles.is_empty() {
        for root in &nodes.roots {
            commands.entity(root).despawn();
        }
        if !nodes.roots.is_empty() {
            hover.set_if_neq(StripHover(None));
        }
        return;
    }
    let row = match nodes.rows.iter().next() {
        Some((row, children)) => {
            let shown = children
                .into_iter()
                .flatten()
                .filter_map(|&child| nodes.views.get(child).ok())
                .map(|view| (view.name.as_str(), view.picture));
            let wanted = strip.tiles.iter().map(|t| (t.name.as_str(), t.picture));
            if shown.eq(wanted) {
                return;
            }
            for &child in children.into_iter().flatten() {
                commands.entity(child).despawn();
            }
            row
        }
        None => spawn_root(&mut commands, panel.0),
    };
    let cell = cell_size(
        strip
            .tiles
            .iter()
            .map(|tile| (tile.grid.columns(), tile.grid.rows())),
    );
    for tile in &strip.tiles {
        spawn_tile(&mut commands, row, tile, cell, &mut images);
    }
}

/// The strip's outer node and its row, along the bottom of the window to
/// the left of the side panel; the row, which the tiles go in.
fn spawn_root(commands: &mut Commands, panel: f32) -> Entity {
    let row = commands
        .spawn((
            StripRow,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: px(12),
                width: percent(100),
                overflow: Overflow {
                    x: OverflowAxis::Scroll,
                    y: OverflowAxis::Clip,
                },
                ..default()
            },
            ScrollPosition::default(),
        ))
        .id();
    commands
        .spawn((
            StripRoot,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                bottom: px(0),
                right: percent(100.0 * panel),
                padding: UiRect::axes(px(10), px(6)),
                ..default()
            },
            BackgroundColor(style::BAR),
            GlobalZIndex(5),
        ))
        .add_child(row);
    row
}

/// One tile under the row: its picture in a box, its name, and its second
/// line.
fn spawn_tile(
    commands: &mut Commands,
    row: Entity,
    tile: &Tile,
    cell: u32,
    images: &mut Assets<Image>,
) {
    let (columns, rows) = picture_cells(&tile.grid);
    let image = images.add(blank_image(columns, rows));
    let (picture, frame) = picture_size(&tile.grid, cell);
    let Rgb(r, g, b) = PALETTE.page;
    commands.spawn((
        TileView {
            name: tile.name.clone(),
            picture: tile.picture,
        },
        Button,
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            padding: UiRect::all(px(4)),
            width: px(frame.x + 8),
            flex_shrink: 0.0,
            overflow: Overflow::clip(),
            border_radius: BorderRadius::all(px(4)),
            ..default()
        },
        BackgroundColor(style::ROW),
        ChildOf(row),
        children![
            (
                Node {
                    width: px(frame.x),
                    height: px(frame.y),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    flex_shrink: 0.0,
                    border_radius: BorderRadius::all(px(3)),
                    ..default()
                },
                BackgroundColor(Color::srgb_u8(r, g, b)),
                children![(
                    Picture {
                        name: tile.name.clone(),
                        image: image.clone(),
                    },
                    ImageNode::new(image),
                    Node {
                        width: px(picture.x),
                        height: px(picture.y),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    RelativeCursorPosition::default(),
                )],
            ),
            style::text(tile.name.clone(), style::TEXT),
            (
                TileStatus(tile.name.clone()),
                Text::new(status_text(&tile.status)),
                TextFont {
                    font_size: FontSize::Px(STATUS_FONT_SIZE),
                    ..default()
                },
                TextColor(status_colour(&tile.status)),
                TextLayout::no_wrap(),
            ),
        ],
    ));
}

/// Brings every model to the present and writes each picture whose colours
/// changed, and each second line that did; nothing while the strip is
/// folded.
fn draw_strip(
    folded: Res<StripFolded>,
    time: Res<Time<Real>>,
    mut strip: ResMut<Strip>,
    pictures: Query<&Picture>,
    mut lines: Query<(&TileStatus, &mut Text, &mut TextColor)>,
    mut images: ResMut<Assets<Image>>,
) {
    if folded.0 || strip.tiles.is_empty() {
        return;
    }
    let now = time.elapsed();
    let strip = strip.bypass_change_detection();
    for tile in &mut strip.tiles {
        bring_forward(tile, now);
    }
    let tiles: HashMap<&str, &Tile> = strip.tiles.iter().map(|t| (t.name.as_str(), t)).collect();
    for picture in &pictures {
        let Some(tile) = tiles.get(picture.name.as_str()) else {
            continue;
        };
        let bytes = pixels(tile);
        let Some(held) = images
            .get(&picture.image)
            .and_then(|image| image.data.as_deref())
        else {
            continue;
        };
        // A picture of another size is one about to be made again.
        if held.len() != bytes.len() || held == bytes.as_slice() {
            continue;
        }
        if let Some(mut image) = images.get_mut(&picture.image) {
            image.data = Some(bytes);
        }
    }
    for (line, mut text, mut colour) in &mut lines {
        let Some(tile) = tiles.get(line.0.as_str()) else {
            continue;
        };
        let words = status_text(&tile.status);
        if text.0 != words {
            text.0 = words;
        }
        colour.set_if_neq(TextColor(status_colour(&tile.status)));
    }
}

/// What the pointer over the strip says: a cell's words over a cell, the
/// tile's hint elsewhere over a tile, else nothing.
fn hover_strip(
    folded: Res<StripFolded>,
    strip: Res<Strip>,
    tiles: Query<&Interaction, With<TileView>>,
    pictures: Query<(&Picture, &RelativeCursorPosition)>,
    mut hover: ResMut<StripHover>,
) {
    if folded.0 || strip.tiles.is_empty() {
        return;
    }
    let over_cell = pictures
        .iter()
        .filter(|(_, cursor)| cursor.cursor_over)
        .find_map(|(picture, cursor)| {
            let tile = strip.tiles.iter().find(|t| t.name == picture.name)?;
            let (column, row) = cell_at(cursor.normalized?, tile.grid.columns(), tile.grid.rows())?;
            cell_words(tile, column, row)
        });
    let over_tile = || {
        tiles
            .iter()
            .any(|interaction| *interaction != Interaction::None)
            .then(|| TILE_HINT.to_owned())
    };
    hover.set_if_neq(StripHover(over_cell.or_else(over_tile)));
}

/// Keeps the strip to the left of the side panel as the panel opens and
/// closes.
fn place_beside_panel(panel: Res<PanelCover>, mut roots: Query<&mut Node, With<StripRoot>>) {
    let right = percent(100.0 * panel.0);
    for mut node in &mut roots {
        if node.right != right {
            node.right = right;
        }
    }
}

/// A click on a tile opens the log in the side panel on its Pipelines
/// filter. The click climbs from the part clicked to the tile.
fn open_log_on_click(
    mut click: On<Pointer<Click>>,
    tiles: Query<(), With<TileView>>,
    mut shows: PanelShows,
    mut kind: ResMut<LogKind>,
) {
    if click.button != PointerButton::Primary || !tiles.contains(click.entity) {
        return;
    }
    click.propagate(false);
    shows.set(PanelChoice {
        log: true,
        help: false,
        collapsed: false,
    });
    kind.set_if_neq(LogKind::Pipelines);
}

/// How far a turn of the wheel moves the strip sideways, in logical
/// pixels: the larger of a sideways scroll and the wheel's turn, so that a
/// trackpad's swipe moves the strip whichever way it mostly goes, and a
/// line of it as a hundred pixels, as Bevy's scroll areas count it.
pub fn sideways(x: f32, y: f32, unit: MouseScrollUnit) -> f32 {
    let turn = if x.abs() > y.abs() { x } else { y };
    match unit {
        MouseScrollUnit::Line => turn * MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
        MouseScrollUnit::Pixel => turn,
    }
}

/// The wheel over the strip scrolls its row sideways, within the tiles.
fn scroll_sideways(
    mut scroll: On<Pointer<Scroll>>,
    mut rows: Query<(&ComputedNode, &mut ScrollPosition), With<StripRow>>,
) {
    let Ok((node, mut position)) = rows.get_mut(scroll.entity) else {
        return;
    };
    scroll.propagate(false);
    let room = (node.content_size().x - node.size().x) * node.inverse_scale_factor;
    let next = (position.x - sideways(scroll.x, scroll.y, scroll.unit)).clamp(0.0, room.max(0.0));
    if position.x != next {
        position.x = next;
    }
}

/// Tells the camera and the labels how much of the window's height the
/// open strip covers: none while it is folded or has no tile.
fn measure_cover(
    roots: Query<&ComputedNode, With<StripRoot>>,
    cameras: Query<&Camera, With<IsDefaultUiCamera>>,
    mut cover: ResMut<StripCover>,
) {
    if roots.is_empty() {
        cover.set_if_neq(StripCover(0.0));
        return;
    }
    let height = roots.iter().map(|node| node.size().y).fold(0.0, f32::max);
    let viewport = cameras
        .iter()
        .find_map(|camera| camera.physical_viewport_size())
        .map_or(0.0, |size| size.y as f32);
    let fraction = crate::bar::cover_fraction(height, viewport).unwrap_or(0.0);
    cover.set_if_neq(StripCover(fraction));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use bevy::asset::AssetEvent;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::{Location, PointerId};
    use bevy::time::TimeUpdateStrategy;
    use petritype_activity::HueMode;

    use super::*;
    use crate::hub::SessionInfo;

    const SESSION: &str = "s";

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    fn edge(source: &str, target: &str) -> TopologyEdge {
        TopologyEdge {
            source: source.into(),
            target: target.into(),
            label: None,
        }
    }

    /// A chain of two steps, as Epimemer declares a phased pipeline:
    /// start, a, after_a, b, after_b.
    fn chain() -> Topology {
        Topology {
            places: names(&["start", "after_a", "after_b"]),
            transitions: names(&["a", "b"]),
            edges: vec![
                edge("start", "a"),
                edge("a", "after_a"),
                edge("after_a", "b"),
                edge("b", "after_b"),
            ],
        }
    }

    fn started(name: &str, topology: &Topology) -> Pipeline {
        Pipeline::Started {
            pipeline_name: name.into(),
            place_names: topology.places.clone(),
            transition_names: topology.transitions.clone(),
            edges: topology.edges.clone(),
        }
    }

    fn fired(name: &str, transition: &str, inputs: &[&str]) -> Pipeline {
        Pipeline::TransitionFired {
            pipeline_name: name.into(),
            transition_name: transition.into(),
            input_places: names(inputs),
        }
    }

    fn landed(name: &str, transition: &str, outputs: &[&str]) -> Pipeline {
        Pipeline::TransitionCompleted {
            pipeline_name: name.into(),
            transition_name: transition.into(),
            output_places: names(outputs),
            duration_ms: 12.0,
        }
    }

    fn tokens(name: &str, counts: &[(&str, i64)]) -> Pipeline {
        Pipeline::TokensUpdated {
            pipeline_name: name.into(),
            place_token_counts: counts
                .iter()
                .map(|&(place, n)| (place.to_owned(), n))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn completed(name: &str, ms: f64) -> Pipeline {
        Pipeline::Completed {
            pipeline_name: name.into(),
            transitions_fired: 2,
            duration_ms: ms,
        }
    }

    fn failed(name: &str, transition: Option<&str>) -> Pipeline {
        Pipeline::Failed {
            pipeline_name: name.into(),
            error: "the embedder did not answer".into(),
            transition_name: transition.map(str::to_owned),
            transitions_fired: 1,
            duration_ms: 30.0,
        }
    }

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn tile_of<'a>(strip: &'a Strip, name: &str) -> &'a Tile {
        strip
            .tiles
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no tile {name}"))
    }

    fn transition(tile: &Tile, name: &str) -> petritype_activity::TransitionActivity {
        tile.activity
            .transition(tile.net.transition(name).unwrap())
            .unwrap()
    }

    fn place(tile: &Tile, name: &str) -> petritype_activity::PlaceActivity {
        tile.activity.place(tile.net.place(name).unwrap()).unwrap()
    }

    fn in_flight(tile: &Tile) -> Vec<&str> {
        tile.net
            .transitions
            .iter()
            .filter(|(id, _)| tile.activity.transition(*id).unwrap().in_flight)
            .map(|(_, name)| name.as_str())
            .collect()
    }

    #[test]
    fn a_topology_numbers_its_nodes_by_position_and_leaves_out_edges_of_neither_kind() {
        let topology = Topology {
            places: names(&["start", "done", "start"]),
            transitions: names(&["go", "check"]),
            edges: vec![
                edge("start", "go"),
                edge("go", "done"),
                edge("go", "start"),
                edge("start", "done"),
                edge("go", "check"),
                edge("nowhere", "go"),
            ],
        };
        let net = net_of(&topology);
        assert_eq!(
            net.places,
            [
                (PlaceId::new(0), "start".to_owned()),
                (PlaceId::new(1), "done".to_owned()),
                (PlaceId::new(2), "start".to_owned()),
            ]
        );
        assert_eq!(net.transition("check"), Some(TransitionId::new(1)));
        assert_eq!(net.place("start"), Some(PlaceId::new(0)), "the first");
        assert_eq!(net.place("go"), None);
        let (start, done, go) = (PlaceId::new(0), PlaceId::new(1), TransitionId::new(0));
        assert_eq!(
            net.edges,
            [
                EdgeInfo::Input {
                    place: start,
                    transition: go,
                    kind: EdgeKind::Take,
                    filter: None,
                },
                EdgeInfo::Output {
                    transition: go,
                    place: done,
                    slot: 0,
                },
                EdgeInfo::Output {
                    transition: go,
                    place: start,
                    slot: 1,
                },
            ]
        );
        assert_eq!(
            net.left_out,
            [
                edge("start", "done"),
                edge("go", "check"),
                edge("nowhere", "go")
            ]
        );
    }

    #[test]
    fn a_pipeline_is_laid_out_by_rank_from_left_to_right() {
        let tile = new_tile("reflect", chain(), Duration::ZERO, 1);
        assert_eq!((tile.grid.columns(), tile.grid.rows()), (5, 1));
        let order: Vec<&str> = (0..5)
            .map(|column| tile.grid.at(column, 0).unwrap().name.as_str())
            .collect();
        assert_eq!(order, ["start", "a", "after_a", "b", "after_b"]);
        assert_eq!(tile.status, Status::Running);
        // At rest, the picture is the net's icon.
        let still = pixels(&tile);
        assert_eq!(still.len(), 5 * 4);
        let icon: Vec<u8> = Thumbnail::icon(&tile.grid, &PALETTE)
            .cells()
            .flat_map(|(_, _, Rgb(r, g, b))| [r, g, b, 255])
            .collect();
        assert_eq!(still, icon);
    }

    #[test]
    fn a_run_lights_its_cells_and_its_end_writes_the_second_line() {
        let mut strip = Strip::default();
        let topology = chain();
        assert_eq!(
            receive(&mut strip, SESSION, &started("reflect", &topology), at(0)),
            Took::Made
        );
        receive(
            &mut strip,
            SESSION,
            &tokens("reflect", &[("start", 2)]),
            at(1),
        );
        receive(
            &mut strip,
            SESSION,
            &fired("reflect", "a", &["start"]),
            at(2),
        );
        let tile = tile_of(&strip, "reflect");
        assert_eq!(in_flight(tile), ["a"]);
        assert_eq!(transition(tile, "a").pulse, 1.0);
        assert_eq!(place(tile, "start").count, 1, "a token taken");
        assert!(place(tile, "start").pulse > 0.9, "lit as it leaves");
        let a = (0..5)
            .find(|&c| tile.grid.at(c, 0).unwrap().name == "a")
            .unwrap();
        assert_eq!(
            &pixels(tile)[a * 4..a * 4 + 3],
            &[PALETTE.running.0, PALETTE.running.1, PALETTE.running.2],
            "in flight, in the running colour"
        );

        receive(
            &mut strip,
            SESSION,
            &landed("reflect", "a", &["after_a"]),
            at(500),
        );
        receive(
            &mut strip,
            SESSION,
            &tokens("reflect", &[("start", 1), ("after_a", 4)]),
            at(500),
        );
        let tile = tile_of(&strip, "reflect");
        assert!(in_flight(tile).is_empty());
        assert_eq!(
            place(tile, "after_a").count,
            4,
            "the hub's count, not the strip's"
        );
        assert!(place(tile, "after_a").pulse > 0.9);
        assert_eq!(tile.at, at(500), "brought to the moment the event came");
        assert_eq!(status_text(&tile.status), "running");

        receive(&mut strip, SESSION, &completed("reflect", 1520.5), at(600));
        let tile = tile_of(&strip, "reflect");
        assert_eq!(
            tile.status,
            Status::Done {
                duration_ms: 1520.5
            }
        );
        assert_eq!(status_text(&tile.status), "done in 1.5 s");
    }

    #[test]
    fn a_failure_marks_its_transition_failed_and_ends_every_flight() {
        let mut strip = Strip::default();
        receive(&mut strip, SESSION, &started("store", &chain()), at(0));
        receive(&mut strip, SESSION, &fired("store", "a", &["start"]), at(1));
        receive(
            &mut strip,
            SESSION,
            &fired("store", "b", &["after_a"]),
            at(2),
        );
        receive(&mut strip, SESSION, &failed("store", Some("b")), at(3));
        let tile = tile_of(&strip, "store");
        assert!(in_flight(tile).is_empty());
        assert_eq!(transition(tile, "b").failure, 1.0);
        assert_eq!(transition(tile, "a").failure, 0.0);
        assert_eq!(status_text(&tile.status), "failed");
        // A failure outside every transition marks none.
        receive(&mut strip, SESSION, &started("early", &chain()), at(4));
        receive(&mut strip, SESSION, &failed("early", None), at(5));
        let tile = tile_of(&strip, "early");
        assert_eq!(tile.status, Status::Failed);
        assert!(
            tile.net.transitions.iter().all(|(id, _)| tile
                .activity
                .transition(*id)
                .unwrap()
                .failure
                == 0.0)
        );
    }

    #[test]
    fn the_same_topology_keeps_the_picture_and_another_makes_it_again() {
        let mut strip = Strip::default();
        receive(&mut strip, SESSION, &started("store", &chain()), at(0));
        receive(&mut strip, SESSION, &started("retrieval", &chain()), at(0));
        receive(
            &mut strip,
            SESSION,
            &fired("store", "a", &["start"]),
            at(10),
        );
        receive(&mut strip, SESSION, &completed("store", 20.0), at(20));
        receive(&mut strip, SESSION, &fired("store", "b", &[]), at(30));
        let before = tile_of(&strip, "store").clone();

        // The next run, of the same net, keeps the picture, its trace among
        // it, and leaves nothing in flight from the run that never ended.
        assert_eq!(
            receive(&mut strip, SESSION, &started("store", &chain()), at(40)),
            Took::Kept
        );
        let kept = tile_of(&strip, "store");
        assert_eq!(kept.picture, before.picture);
        assert!(in_flight(kept).is_empty());
        assert!(transition(kept, "a").ember > 0.0, "the trace stays");
        assert_eq!(kept.status, Status::Running);

        // A run of another net makes the picture again, in the same place.
        let mut wider = chain();
        wider.transitions.push("c".into());
        wider.places.push("after_c".into());
        wider.edges.push(edge("after_b", "c"));
        wider.edges.push(edge("c", "after_c"));
        assert_eq!(
            receive(&mut strip, SESSION, &started("store", &wider), at(50)),
            Took::MadeAgain
        );
        let again = tile_of(&strip, "store");
        assert!(again.picture > before.picture);
        assert_eq!((again.grid.columns(), again.grid.rows()), (7, 1));
        assert_eq!(transition(again, "a").ember, 0.0, "a new model");
        let order: Vec<&str> = strip.tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            order,
            ["store", "retrieval"],
            "in the order each first started"
        );
    }

    #[test]
    fn an_event_for_a_pipeline_with_no_tile_makes_none() {
        let mut strip = Strip::default();
        for event in [
            fired("segmentation:semantic", "split", &["start"]),
            landed("segmentation:semantic", "split", &["after_split"]),
            tokens("segmentation:semantic", &[("start", 1)]),
            completed("segmentation:semantic", 5.0),
            failed("segmentation:semantic", Some("split")),
        ] {
            assert_eq!(receive(&mut strip, SESSION, &event, at(1)), Took::NoTile);
        }
        assert!(strip.tiles.is_empty());
        // Its next start makes it.
        assert_eq!(
            receive(
                &mut strip,
                SESSION,
                &started("segmentation:semantic", &chain()),
                at(2)
            ),
            Took::Made
        );
        assert_eq!(strip.tiles.len(), 1);
    }

    #[test]
    fn lost_events_end_every_flight_and_mark_a_running_tile_unknown() {
        let mut strip = Strip::default();
        receive(&mut strip, SESSION, &started("running", &chain()), at(0));
        receive(&mut strip, SESSION, &fired("running", "a", &[]), at(1));
        receive(&mut strip, SESSION, &started("done", &chain()), at(0));
        receive(&mut strip, SESSION, &completed("done", 3.0), at(2));
        receive(&mut strip, SESSION, &started("broke", &chain()), at(0));
        receive(&mut strip, SESSION, &failed("broke", None), at(2));
        lose(&mut strip, at(100));
        let status = |name| status_text(&tile_of(&strip, name).status);
        assert_eq!(
            [status("running"), status("done"), status("broke")],
            ["unknown", "done in 3.0 ms", "failed"]
        );
        assert!(strip.tiles.iter().all(|t| in_flight(t).is_empty()));
        assert!(strip.tiles.iter().all(|t| t.at == at(100)));
        // The pipeline's next fire says it runs.
        receive(&mut strip, SESSION, &fired("running", "b", &[]), at(200));
        assert_eq!(tile_of(&strip, "running").status, Status::Running);
    }

    #[test]
    fn another_sessions_event_starts_the_strip_afresh() {
        let mut strip = Strip::default();
        receive(&mut strip, SESSION, &started("store", &chain()), at(0));
        assert!(!for_session(&mut strip, SESSION));
        assert_eq!(
            receive(&mut strip, "other", &fired("store", "a", &[]), at(1)),
            Took::NoTile
        );
        assert_eq!(strip.session, "other");
        assert!(strip.tiles.is_empty());
    }

    /// Two levels agree to within float rounding.
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Whether two tiles' models stand at the same levels.
    fn same_levels(a: &Tile, b: &Tile) -> bool {
        a.net.places.iter().all(|&(id, _)| {
            let (p, q) = (a.activity.place(id).unwrap(), b.activity.place(id).unwrap());
            close(p.pulse, q.pulse)
                && close(p.ember, q.ember)
                && close(p.flow, q.flow)
                && p.count == q.count
        }) && a.net.transitions.iter().all(|&(id, _)| {
            let (s, t) = (
                a.activity.transition(id).unwrap(),
                b.activity.transition(id).unwrap(),
            );
            close(s.pulse, t.pulse)
                && close(s.ember, t.ember)
                && close(s.failure, t.failure)
                && s.in_flight == t.in_flight
        })
    }

    #[test]
    fn a_model_brought_forward_once_equals_one_advanced_frame_by_frame() {
        // Each event comes in the frame of its moment, 16 ms a frame.
        let script = [
            (0, started("store", &chain())),
            (16, fired("store", "a", &["start"])),
            (304, landed("store", "a", &["after_a"])),
            (304, tokens("store", &[("after_a", 3)])),
            (320, fired("store", "b", &["after_a"])),
            (896, failed("store", Some("b"))),
        ];
        // Folded: brought forward only as each event comes, then once to
        // the present.
        let mut once = Strip::default();
        for (ms, event) in &script {
            receive(&mut once, SESSION, event, at(*ms));
        }
        bring_forward(&mut once.tiles[0], at(2000));
        // Open: brought forward every frame as well.
        let mut frames = Strip::default();
        let mut events = script.iter().peekable();
        for frame in 0..=125 {
            let now = at(frame * 16);
            while let Some((_, event)) = events.next_if(|(ms, _)| at(*ms) == now) {
                receive(&mut frames, SESSION, event, now);
            }
            bring_forward(&mut frames.tiles[0], now);
        }
        let (once, frames) = (&once.tiles[0], &frames.tiles[0]);
        assert_eq!((once.at, frames.at), (at(2000), at(2000)));
        assert!(same_levels(once, frames));
        assert_eq!(pixels(once), pixels(frames));
        assert!(
            !same_levels(once, &new_tile("store", chain(), at(0), 9)),
            "and not at rest, which would agree trivially"
        );
    }

    #[test]
    fn every_tile_has_the_largest_cell_at_which_every_grid_fits_the_box() {
        assert_eq!(cell_size([]), MAX_CELL, "nothing to fit");
        assert_eq!(cell_size([(5, 1)]), 12, "a short chain, at the most");
        assert_eq!(
            cell_size([(5, 1), (17, 1)]),
            11,
            "17 cells across fit 200 at 11"
        );
        assert_eq!(cell_size([(5, 4)]), 10, "4 rows fit 40 at 10");
        assert_eq!(cell_size([(100, 1)]), 2);
        assert_eq!(
            cell_size([(5, 1), (150, 1)]),
            MIN_CELL,
            "a net too large for the box shows whole at the smallest"
        );
        let tile = new_tile("store", chain(), Duration::ZERO, 1);
        assert_eq!(
            picture_size(&tile.grid, 12),
            (UVec2::new(60, 12), UVec2::new(200, 40))
        );
        let mut long = Topology {
            places: Vec::new(),
            transitions: Vec::new(),
            edges: Vec::new(),
        };
        for i in 0..80 {
            long.places.push(format!("p{i}"));
            long.transitions.push(format!("t{i}"));
            long.edges.push(edge(&format!("p{i}"), &format!("t{i}")));
            if i + 1 < 80 {
                long.edges
                    .push(edge(&format!("t{i}"), &format!("p{}", i + 1)));
            }
        }
        let long = new_tile("long", long, Duration::ZERO, 2);
        assert_eq!(long.grid.columns(), 160);
        assert_eq!(
            picture_size(&long.grid, 2),
            (UVec2::new(320, 2), UVec2::new(320, 40)),
            "its tile grows"
        );
        let empty = Topology {
            places: Vec::new(),
            transitions: Vec::new(),
            edges: Vec::new(),
        };
        let empty = new_tile("empty", empty, Duration::ZERO, 3);
        assert_eq!(picture_cells(&empty.grid), (1, 1));
        assert_eq!(pixels(&empty).len(), 4);
    }

    #[test]
    fn the_pointer_over_a_picture_is_over_the_cell_beneath_it() {
        assert_eq!(cell_at(Vec2::new(-0.5, -0.5), 5, 1), Some((0, 0)));
        assert_eq!(cell_at(Vec2::new(0.0, 0.0), 5, 1), Some((2, 0)));
        assert_eq!(cell_at(Vec2::new(0.49, 0.49), 5, 2), Some((4, 1)));
        assert_eq!(cell_at(Vec2::new(0.5, 0.0), 5, 1), None, "past the edge");
        assert_eq!(cell_at(Vec2::new(0.0, -0.6), 5, 1), None);
        assert_eq!(cell_at(Vec2::ZERO, 0, 0), None);
    }

    #[test]
    fn a_cell_is_named_with_a_places_count() {
        let mut strip = Strip::default();
        receive(&mut strip, SESSION, &started("store", &chain()), at(0));
        receive(
            &mut strip,
            SESSION,
            &tokens("store", &[("start", 3), ("after_a", 1)]),
            at(1),
        );
        let tile = tile_of(&strip, "store");
        let words: Vec<Option<String>> = (0..6).map(|c| cell_words(tile, c, 0)).collect();
        assert_eq!(
            words,
            [
                Some("store: place start, 3 tokens".to_owned()),
                Some("store: transition a".to_owned()),
                Some("store: place after_a, 1 token".to_owned()),
                Some("store: transition b".to_owned()),
                Some("store: place after_b, 0 tokens".to_owned()),
                None,
            ]
        );
    }

    #[test]
    fn the_wheel_scrolls_sideways_by_its_turn_or_a_sideways_scroll() {
        assert_eq!(sideways(0.0, -1.0, MouseScrollUnit::Line), -100.0);
        assert_eq!(sideways(0.0, 30.0, MouseScrollUnit::Pixel), 30.0);
        assert_eq!(sideways(-12.0, 30.0, MouseScrollUnit::Pixel), 30.0);
        assert_eq!(sideways(-40.0, 3.0, MouseScrollUnit::Pixel), -40.0);

        // The wheel over a tile scrolls the row, within the tiles: a row 300
        // wide holding 500 of tiles scrolls up to 200.
        let mut app = app(false, Duration::from_millis(16));
        feed(&mut app, &started("store", &chain()));
        app.update();
        let row = {
            let mut rows = app.world_mut().query_filtered::<Entity, With<StripRow>>();
            rows.single(app.world()).unwrap()
        };
        app.world_mut().entity_mut(row).insert(ComputedNode {
            size: Vec2::new(300.0, 90.0),
            content_size: Vec2::new(500.0, 90.0),
            inverse_scale_factor: 1.0,
            ..default()
        });
        let picture = {
            let mut pictures = app.world_mut().query_filtered::<Entity, With<Picture>>();
            pictures.single(app.world()).unwrap()
        };
        let wheel = |app: &mut App, y: f32| {
            let location = Location {
                target: NormalizedRenderTarget::None {
                    width: 1,
                    height: 1,
                },
                position: Vec2::ZERO,
            };
            let scroll = Scroll {
                unit: MouseScrollUnit::Line,
                x: 0.0,
                y,
                hit: HitData::new(picture, 0.0, None, None),
                phase: bevy::input::touch::TouchPhase::Moved,
            };
            app.world_mut()
                .trigger(Pointer::new(PointerId::Mouse, location, scroll, picture));
            app.world().get::<ScrollPosition>(row).unwrap().x
        };
        assert_eq!(wheel(&mut app, -1.0), 100.0, "a notch down moves right");
        assert_eq!(wheel(&mut app, -5.0), 200.0, "no further than the tiles");
        assert_eq!(wheel(&mut app, 3.0), 0.0, "and back");
    }

    #[test]
    fn the_strip_starts_open_or_folded() {
        assert_eq!(parse_strip("open"), Some(false));
        assert_eq!(parse_strip("folded"), Some(true));
        assert_eq!(parse_strip("closed"), None);
        assert_eq!(
            status_text(&Status::Unknown),
            "unknown",
            "a run whose end may have been missed"
        );
        assert_eq!(colour(PALETTE.running), Color::srgb_u8(183, 121, 31));
        assert_eq!(
            PALETTE.place(0.0, 0.0, HueMode::Kind),
            PALETTE.place_dim,
            "a still place is its dim colour"
        );
    }

    /// An app with the strip and no window: every update advances the real
    /// clock by `frame`.
    fn app(folded: bool, frame: Duration) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(frame))
            .add_plugins(StripPlugin { folded });
        // A click climbs through `PointerTraversal`, which looks for a
        // window too, so the component must be known even with no window.
        app.world_mut().register_component::<Window>();
        app.update();
        app
    }

    /// Hands the app's strip an event of the session, as `live.rs` does.
    fn feed(app: &mut App, event: &Pipeline) -> Option<Took> {
        receive_event(app.world_mut(), SESSION, event)
    }

    fn count<C: Component>(app: &mut App) -> usize {
        let mut query = app.world_mut().query_filtered::<(), With<C>>();
        query.iter(app.world()).count()
    }

    /// The image events since the last call: how many pictures were made,
    /// and how many written.
    fn image_events(app: &mut App) -> (usize, usize) {
        let events: Vec<AssetEvent<Image>> = app
            .world_mut()
            .resource_mut::<Messages<AssetEvent<Image>>>()
            .drain()
            .collect();
        let added = events
            .iter()
            .filter(|e| matches!(e, AssetEvent::Added { .. }))
            .count();
        let written = events
            .iter()
            .filter(|e| matches!(e, AssetEvent::Modified { .. }))
            .count();
        (added, written)
    }

    fn picture_bytes(app: &mut App, name: &str) -> Vec<u8> {
        let mut pictures = app.world_mut().query::<&Picture>();
        let handle = pictures
            .iter(app.world())
            .find(|p| p.name == name)
            .map(|p| p.image.clone())
            .unwrap();
        let images = app.world().resource::<Assets<Image>>();
        images.get(&handle).unwrap().data.clone().unwrap()
    }

    #[test]
    fn the_open_strip_shows_a_tile_per_pipeline_and_writes_a_picture_only_when_it_changes() {
        let mut app = app(false, Duration::from_secs(60));
        assert_eq!(count::<StripRoot>(&mut app), 0, "no room until a start");
        assert_eq!(
            feed(&mut app, &started("store", &chain())),
            Some(Took::Made)
        );
        assert_eq!(
            feed(&mut app, &fired("store", "a", &["start"])),
            Some(Took::Applied)
        );
        app.update();
        assert_eq!(count::<StripRoot>(&mut app), 1);
        assert_eq!(count::<TileView>(&mut app), 1);
        let (added, written) = image_events(&mut app);
        assert_eq!((added, written), (1, 1), "made, then drawn");
        let running = [PALETTE.running.0, PALETTE.running.1, PALETTE.running.2];
        assert_eq!(&picture_bytes(&mut app, "store")[4..7], &running);

        // A second pipeline makes the tiles again; the first keeps its
        // picture's number.
        feed(&mut app, &started("retrieval", &chain()));
        feed(&mut app, &completed("store", 40.0));
        app.update();
        assert_eq!(count::<TileView>(&mut app), 2);
        let mut lines = app.world_mut().query::<(&TileStatus, &Text)>();
        let mut words: Vec<(String, String)> = lines
            .iter(app.world())
            .map(|(line, text)| (line.0.clone(), text.0.clone()))
            .collect();
        words.sort();
        assert_eq!(
            words,
            [
                ("retrieval".to_owned(), "running".to_owned()),
                ("store".to_owned(), "done in 40 ms".to_owned())
            ]
        );
        image_events(&mut app);

        // A minute a frame: within a few frames every light has faded and
        // no picture is written again.
        for _ in 0..4 {
            app.update();
        }
        image_events(&mut app);
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            image_events(&mut app),
            (0, 0),
            "nothing changed, nothing written"
        );
        // An event lights a cell again, and only its picture is written.
        feed(&mut app, &fired("retrieval", "b", &[]));
        app.update();
        assert_eq!(image_events(&mut app), (0, 1));
    }

    #[test]
    fn a_folded_strip_writes_no_image_and_unfolding_draws_what_an_open_one_would() {
        let frame = Duration::from_millis(16);
        let (mut open, mut folded) = (app(false, frame), app(true, frame));
        let script = [
            (0, started("store", &chain())),
            (2, fired("store", "a", &["start"])),
            (20, landed("store", "a", &["after_a"])),
            (20, tokens("store", &[("after_a", 2)])),
            (22, fired("store", "b", &["after_a"])),
            (60, failed("store", Some("b"))),
        ];
        let mut events = script.iter().peekable();
        image_events(&mut folded);
        for frame in 0..150 {
            while let Some((_, event)) = events.next_if(|(at, _)| *at == frame) {
                feed(&mut open, event);
                feed(&mut folded, event);
            }
            open.update();
            folded.update();
            assert_eq!(count::<StripRoot>(&mut folded), 0, "no nodes while folded");
            assert_eq!(
                image_events(&mut folded),
                (0, 0),
                "no image while folded, frame {frame}"
            );
        }
        // Folded, the model stands where the last event left it.
        let behind = folded.world().resource::<Strip>().tiles[0].at;
        let ahead = open.world().resource::<Strip>().tiles[0].at;
        assert!(behind < ahead, "{behind:?} {ahead:?}");

        folded.insert_resource(StripFolded(false));
        open.update();
        folded.update();
        let (a, b) = (
            &open.world().resource::<Strip>().tiles[0],
            &folded.world().resource::<Strip>().tiles[0],
        );
        assert_eq!(a.at, b.at, "brought to the present");
        assert!(same_levels(a, b), "brought forward once, as frame by frame");
        let (drawn_open, drawn_folded) = (
            picture_bytes(&mut open, "store"),
            picture_bytes(&mut folded, "store"),
        );
        assert!(
            drawn_open
                .iter()
                .zip(&drawn_folded)
                .all(|(x, y)| x.abs_diff(*y) <= 1),
            "{drawn_open:?} {drawn_folded:?}"
        );
        assert_eq!(
            image_events(&mut folded).0,
            1,
            "the picture made on unfolding"
        );
    }

    #[test]
    fn a_start_of_the_same_net_keeps_the_drawn_picture_and_another_net_makes_it_again() {
        let mut app = app(false, Duration::from_millis(16));
        let picture = |app: &mut App| {
            let mut pictures = app.world_mut().query::<(Entity, &Picture)>();
            let (entity, picture) = pictures.single(app.world()).unwrap();
            let image = app.world().resource::<Assets<Image>>();
            (entity, image.get(&picture.image).unwrap().size())
        };
        feed(&mut app, &started("store", &chain()));
        feed(&mut app, &fired("store", "a", &["start"]));
        app.update();
        let (first, size) = picture(&mut app);
        assert_eq!(size, UVec2::new(5, 1), "a pixel a cell");
        assert_eq!(
            feed(&mut app, &started("store", &chain())),
            Some(Took::Kept)
        );
        app.update();
        assert_eq!(picture(&mut app).0, first, "the same picture");
        let tile = &app.world().resource::<Strip>().tiles[0];
        assert!(in_flight(tile).is_empty());

        let mut wider = chain();
        wider.transitions.push("c".into());
        wider.places.push("after_c".into());
        wider.edges.push(edge("after_b", "c"));
        wider.edges.push(edge("c", "after_c"));
        assert_eq!(
            feed(&mut app, &started("store", &wider)),
            Some(Took::MadeAgain)
        );
        app.update();
        let (again, size) = picture(&mut app);
        assert_ne!(again, first, "a picture made again");
        assert_eq!(size, UVec2::new(7, 1));
        assert_eq!(count::<TileView>(&mut app), 1);
    }

    #[test]
    fn k_folds_the_strip_and_brings_it_back() {
        let mut app = app(false, Duration::from_millis(16));
        feed(&mut app, &started("store", &chain()));
        app.update();
        assert_eq!(count::<StripRoot>(&mut app), 1);
        let press = |app: &mut App| {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(STRIP_KEY.0);
            app.update();
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(STRIP_KEY.0);
            keys.clear();
        };
        press(&mut app);
        assert!(app.world().resource::<StripFolded>().0);
        assert_eq!(count::<StripRoot>(&mut app), 0);
        assert_eq!(count::<Picture>(&mut app), 0);
        press(&mut app);
        assert!(!app.world().resource::<StripFolded>().0);
        assert_eq!(count::<StripRoot>(&mut app), 1);
        // Not while a text field takes the keyboard.
        app.insert_resource(UiFocus {
            pointer_over_ui: false,
            typing: true,
        });
        press(&mut app);
        assert!(!app.world().resource::<StripFolded>().0);
    }

    fn session(id: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            pid: 1,
            backend: "surrealdb".into(),
            active_graph: "memory".into(),
            started_at: "2026-11-17T10:00:00Z".into(),
            connected: true,
            last_event_at: None,
            schema_version: Some(1),
        }
    }

    fn catalogue(session_id: &str, graph: &str) -> Catalogue {
        Catalogue {
            hub: "127.0.0.1:1".into(),
            sessions: vec![session(session_id)],
            session: session(session_id),
            graph: graph.into(),
            browsing: session_id.into(),
            graphs: None,
        }
    }

    #[test]
    fn a_session_switch_empties_the_strip_and_a_graph_switch_does_not() {
        let mut app = app(false, Duration::from_millis(16));
        app.insert_resource(catalogue(SESSION, "memory"));
        app.update();
        feed(&mut app, &started("store", &chain()));
        app.update();
        app.insert_resource(catalogue(SESSION, "games"));
        app.update();
        assert_eq!(
            app.world().resource::<Strip>().tiles.len(),
            1,
            "a graph switch"
        );
        assert_eq!(count::<TileView>(&mut app), 1);
        app.insert_resource(catalogue("another", "games"));
        app.update();
        let strip = app.world().resource::<Strip>();
        assert!(strip.tiles.is_empty(), "a session switch");
        assert_eq!(strip.session, "another");
        assert_eq!(count::<StripRoot>(&mut app), 0, "and it takes no room");
    }

    #[test]
    fn hovering_a_cell_names_it_and_a_click_on_a_tile_opens_the_log_on_its_pipelines() {
        let mut app = app(false, Duration::from_millis(16));
        feed(&mut app, &started("store", &chain()));
        feed(&mut app, &tokens("store", &[("start", 2)]));
        app.update();
        let picture = {
            let mut pictures = app.world_mut().query_filtered::<Entity, With<Picture>>();
            pictures.single(app.world()).unwrap()
        };
        // Over the first of five cells.
        app.world_mut()
            .entity_mut(picture)
            .insert(RelativeCursorPosition {
                cursor_over: true,
                normalized: Some(Vec2::new(-0.45, 0.0)),
            });
        app.update();
        assert_eq!(
            app.world().resource::<StripHover>().0.as_deref(),
            Some("store: place start, 2 tokens")
        );
        // Over the tile, off its picture.
        app.world_mut()
            .entity_mut(picture)
            .insert(RelativeCursorPosition::default());
        let tile = {
            let mut tiles = app.world_mut().query_filtered::<Entity, With<TileView>>();
            tiles.single(app.world()).unwrap()
        };
        app.world_mut()
            .entity_mut(tile)
            .insert(Interaction::Hovered);
        app.update();
        assert_eq!(
            app.world().resource::<StripHover>().0.as_deref(),
            Some(TILE_HINT)
        );
        app.world_mut().entity_mut(tile).insert(Interaction::None);
        app.update();
        assert_eq!(app.world().resource::<StripHover>().0, None);

        // A click lands on the picture and climbs to its tile.
        app.insert_resource(PanelCollapsed(true))
            .insert_resource(HelpOpen(true));
        let location = Location {
            target: NormalizedRenderTarget::None {
                width: 1,
                height: 1,
            },
            position: Vec2::ZERO,
        };
        let click = Click {
            button: PointerButton::Primary,
            hit: HitData::new(picture, 0.0, None, None),
            duration: Duration::ZERO,
            count: 1,
        };
        app.world_mut()
            .trigger(Pointer::new(PointerId::Mouse, location, click, picture));
        app.update();
        assert!(app.world().resource::<LogOpen>().0);
        assert!(!app.world().resource::<HelpOpen>().0);
        assert!(!app.world().resource::<PanelCollapsed>().0);
        assert_eq!(*app.world().resource::<LogKind>(), LogKind::Pipelines);

        // Folding clears what the strip said.
        app.world_mut()
            .entity_mut(tile)
            .insert(Interaction::Hovered);
        app.update();
        assert!(app.world().resource::<StripHover>().0.is_some());
        app.insert_resource(StripFolded(true));
        app.update();
        assert_eq!(app.world().resource::<StripHover>().0, None);
    }
}
