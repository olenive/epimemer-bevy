//! epimemer-bevy: a Bevy visualiser for Epimemer graphs.
//!
//! One 3D scene in which the knowledge graph stands on a topic map and extends
//! along a time axis; the graph view, the timeline view and the free 3D view
//! are three camera presets of it. `DESIGN.md` is the design and its order of
//! work. The hub it reads from, and the shapes it reads, are specified in the
//! Epimemer repository's `dev-docs/VIZ_CONTRACT.md`.
//!
//! This is slices 1 to 10, 12 and 13 of that order: it reads one graph's snapshot from
//! the hub and draws every node on a time axis, topics on a map fitted from
//! their embeddings and saved (DESIGN.md §7) and claims around them, in
//! recorded time as a disc at the moment it was recorded and in claim time
//! as the column, smear or disc of DESIGN.md §4, with the drawn axis, its
//! ticks and "now", the tray of what claim time cannot place, and validity
//! brackets on focus; the camera presets of DESIGN.md §6, a free camera
//! between them, the filters of DESIGN.md §10, set from the command line and
//! from the bar along the top, where another session's or graph's snapshot
//! can be loaded too, focus and hover on every node, the label budget of
//! DESIGN.md §9, and the edges of DESIGN.md §5: claim-to-claim lines on
//! hover, on focus and with the edge toggle, and succession connectors
//! always; the side panel, with the focused node in full, and the search
//! field of DESIGN.md §10; it follows the graph's event stream, applying
//! each event as it comes (DESIGN.md §11), and keeps the session's retrieval
//! records, one of which the bar can choose to see what a tool call handed
//! the agent (`retrieval.rs`), and its acts, advisories and pipeline runs,
//! which the side panel shows as a log (`event_log.rs`), with the graph's
//! reflect counter in the bar (`reflect_counter.rs`). It builds for the browser as well
//! as natively, from the same code: the reads and the stream run in the
//! background over a transport each platform implements (`background.rs`,
//! `hub.rs`, `stream.rs`), and in a browser the options come from the page's
//! address (`browser.rs`). This file reads the options, subscribes to the
//! stream, fetches and places the first graph, and assembles the app.

mod apply;
mod axis;
mod background;
mod bar;
mod brackets;
#[cfg(any(test, target_arch = "wasm32"))]
mod browser;
mod camera;
mod clip;
#[cfg(not(target_arch = "wasm32"))]
mod dev;
mod edge_index;
mod edges;
mod event_log;
mod filter;
mod focus;
mod fonts;
mod graph_index;
mod hub;
mod keys;
mod labels;
mod layout;
mod live;
mod panel;
mod persist;
mod pickers;
mod picking;
mod presets;
mod projection;
mod reflect_counter;
mod relax;
mod relayout;
mod reload;
mod retrieval;
mod scene;
mod search;
mod stream;
mod time_axis;
mod time_mode;
mod topic_map;
mod tray;
mod ui_focus;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use bevy::app::PluginGroupBuilder;
#[cfg(target_arch = "wasm32")]
use bevy::log::LogPlugin;
use bevy::prelude::*;
use chrono::NaiveDate;

use crate::axis::AxisPlugin;
use crate::bar::{BarPlugin, UI_STATES, UiState};
use crate::brackets::BracketPlugin;
use crate::camera::CameraPlugin;
#[cfg(not(target_arch = "wasm32"))]
use crate::dev::{DumpLayoutPlugin, FpsLogPlugin, ScreenshotPlugin};
use crate::edges::{AllEdges, EdgePlugin};
use crate::filter::{Filter, FilterPlugin, Kinds, Metacontexts, Mode};
use crate::focus::{Focus, FocusPlugin};
use crate::fonts::FontPlugin;
use crate::graph_index::GraphIndex;
use crate::labels::LabelPlugin;
use crate::layout::Kind;
use crate::live::{Connection, LivePlugin, LiveState};
use crate::panel::PanelPlugin;
use crate::persist::{MapKey, Store};
use crate::picking::PickPlugin;
use crate::presets::{PRESET_NAMES, Preset};
use crate::reflect_counter::ReflectPlugin;
use crate::relayout::{MapState, RelayoutPlugin};
use crate::reload::{Arrival, Catalogue, LoadStatus, Prepared, ReloadPlugin};
use crate::retrieval::RetrievalPlugin;
use crate::scene::{Nodes, ScenePlugin};
use crate::search::{Search, SearchPlugin};
use crate::stream::{Stream, Subscription};
use crate::time_axis::{MODE_NAMES, TimeMode};
use crate::time_mode::TimeModePlugin;
use crate::topic_map::Purpose;
use crate::tray::TrayPlugin;

/// How long the start waits for the event stream's first connection before
/// reading the snapshot without it.
const FIRST_CONNECT_WAIT: Duration = Duration::from_secs(2);

const USAGE: &str = "\
epimemer-bevy: draws an Epimemer graph on a time axis, as it grew or over
the periods its claims are about.

Usage: epimemer-bevy [options]

  --hub HOST:PORT     the visualisation hub (default 127.0.0.1:8765)
  --graph NAME        the graph to draw (default memory)
  --session ID        the session to read from (default: the connected
                      session whose active graph is NAME, else the most
                      recently active connected session)
  --preset NAME       the view to start at: graph, timeline, 3d or now
                      (default 3d)
  --focus NODE        focus a node from the start, by its id or, for a
                      topic, its exact name
  --search TEXT       type TEXT into the search field and focus its first
                      hit, the first node drawn whose text or description
                      holds it, ignoring case
  --time MODE         the time axis to start in: recorded, every node at
                      the time it was recorded (the default), or claim,
                      each claim over the period its timepoint gives
  --all-edges         start with the edge toggle on: every claim-to-claim
                      edge between visible nodes drawn as a line
  --recalculate       fit a new topic map from the topics' embeddings once
                      they are read, and save it over the one saved, as
                      Recalculate layout does
  --no-persist        read no saved topic map and save none: the map is
                      fitted afresh and kept only while the app runs

Filters, which dim the nodes that fail them:
  --kinds LIST        only these kinds of node, from topic, fact and
                      inference, separated by commas
  --metacontext LIST  only the nodes in these metacontexts, by id,
                      separated by commas
  --topic TOPIC       only this topic, by id or exact name, the topics below
                      it, and the claims filed under or tagged with any of
                      them
  --window FROM..TO   only the nodes recorded from FROM to TO, each an ISO
                      date or timestamp in UTC; a date as TO takes in that
                      whole day
  --source ID         only the nodes read from this document, by id
  --retrieval RECORD  only the nodes this retrieval record retrieved, by its
                      number, or latest for the graph's newest; chosen once
                      the session's records are read
  --hide              hide the nodes that fail instead of dimming them:
                      they cannot be clicked and the views leave them out

  --ui-state STATE    how the bar starts: open (the default), collapsed,
                      or with a picker's list open: sessions, graphs,
                      metacontexts, topics, topics:PREFIX with PREFIX
                      typed, sources, or retrievals; or log, open with the
                      log in the side panel
  --screenshot PATH   save a PNG of the view and the bar once the scene is
                      drawn, then exit
  --screenshot-after SECONDS
                      with --screenshot, take it this long after the start
                      rather than as soon as the scene is drawn, so that a
                      change arriving on the event stream is in it
  --log-events        print every event applied from the stream to stderr,
                      one line each, with the counts after it
  --fit-topic         once the view is fitted below the bar, fit the
                      focused node's topic, as T does, with --focus or
                      --search
  --layout-dir DIR    keep the topic maps under DIR rather than the
                      platform's data directory
                      (~/Library/Application Support/epimemer-bevy on macOS,
                      the XDG data directory elsewhere)
  --dump-layout PATH  once the topic map has settled, write every topic's
                      id and place on the map to PATH as JSON, then exit
  --fps               log the frame rate once a second
  --orbit             circle the graph by itself, to measure while moving,
                      in perspective whatever --preset says; the view keys
                      do nothing meanwhile
  --help              this text";

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    request: hub::Request,
    preset: Preset,
    /// The time mode to start in.
    time: TimeMode,
    /// The node to focus from the start, by id or topic name, checked
    /// against the graph by [`resolve_focus`].
    focus: Option<String>,
    /// What to type into the search field from the start; its first hit is
    /// focused.
    search: Option<String>,
    /// Start with the edge toggle on.
    all_edges: bool,
    /// Fit a new topic map once the embeddings are read.
    recalculate: bool,
    /// Read and save no topic map.
    no_persist: bool,
    /// Where the topic maps are kept, in place of the data directory.
    layout_dir: Option<PathBuf>,
    /// Write the topics' places here once the map settles, then exit.
    dump_layout: Option<PathBuf>,
    filter: FilterArgs,
    ui_state: UiState,
    screenshot: Option<PathBuf>,
    /// How long after the start to take the screenshot.
    screenshot_after: Option<Duration>,
    /// Print every event applied.
    log_events: bool,
    /// Fit the focused node's topic at the start, as `T` does.
    fit_topic: bool,
    fps: bool,
    orbit: bool,
}

/// The filters the command line asked for, as far as they can be read
/// before the graph is: the ids and the topic are checked against it by
/// [`resolve_filter`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FilterArgs {
    kinds: Option<Kinds>,
    metacontexts: Option<Vec<String>>,
    topic: Option<String>,
    /// From and to, in microseconds since the Unix epoch.
    window: Option<(i64, i64)>,
    source: Option<String>,
    /// A retrieval record's number, or `latest`, chosen once the records
    /// are read (`retrieval.rs`).
    retrieval: Option<String>,
    hide: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Run(Box<Args>),
    Help,
}

/// Reads the arguments, without the program name. Each option with a value
/// takes it as the next argument or after `=`.
fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = Args {
        request: hub::Request {
            hub: "127.0.0.1:8765".into(),
            session: None,
            graph: "memory".into(),
        },
        preset: Preset::Free,
        time: TimeMode::Recorded,
        focus: None,
        search: None,
        all_edges: false,
        recalculate: false,
        no_persist: false,
        layout_dir: None,
        dump_layout: None,
        filter: FilterArgs::default(),
        ui_state: UiState::Open,
        screenshot: None,
        screenshot_after: None,
        log_events: false,
        fit_topic: false,
        fps: false,
        orbit: false,
    };
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name.to_owned(), Some(value.into())),
            _ => (argument, None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| arguments.next())
                .filter(|v: &String| !v.is_empty())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match name.as_str() {
            "--hub" => {
                let hub = value()?;
                if hub.contains('/') {
                    return Err(format!("--hub takes HOST:PORT, not a URL: {hub}"));
                }
                args.request.hub = hub;
            }
            "--graph" => args.request.graph = value()?,
            "--session" => args.request.session = Some(value()?),
            "--preset" => {
                let name = value()?;
                args.preset = presets::parse_preset(&name).ok_or_else(|| {
                    let names: Vec<&str> = PRESET_NAMES.iter().map(|&(n, _)| n).collect();
                    format!("--preset takes {}, not {name}", names.join(", "))
                })?;
            }
            "--focus" => args.focus = Some(value()?),
            "--search" => args.search = Some(value()?),
            "--time" => {
                let name = value()?;
                args.time = time_axis::parse_mode(&name).ok_or_else(|| {
                    let names: Vec<&str> = MODE_NAMES.iter().map(|&(n, _)| n).collect();
                    format!("--time takes {}, not {name}", names.join(" or "))
                })?;
            }
            "--kinds" => args.filter.kinds = Some(parse_kinds(&value()?)?),
            "--metacontext" => args.filter.metacontexts = Some(parse_list(&value()?)),
            "--topic" => args.filter.topic = Some(value()?),
            "--window" => args.filter.window = Some(parse_window(&value()?)?),
            "--source" => args.filter.source = Some(value()?),
            "--retrieval" => args.filter.retrieval = Some(value()?),
            "--ui-state" => {
                let state = value()?;
                args.ui_state = bar::parse_ui_state(&state)
                    .ok_or_else(|| format!("--ui-state takes {UI_STATES}, not {state}"))?;
            }
            "--screenshot" => args.screenshot = Some(value()?.into()),
            "--layout-dir" => args.layout_dir = Some(value()?.into()),
            "--dump-layout" => args.dump_layout = Some(value()?.into()),
            "--screenshot-after" => {
                let text = value()?;
                let seconds = text
                    .parse::<f64>()
                    .ok()
                    .filter(|s| s.is_finite() && *s >= 0.0 && *s < 86_400.0)
                    .ok_or_else(|| {
                        format!("--screenshot-after takes a number of seconds, not {text}")
                    })?;
                args.screenshot_after = Some(Duration::from_secs_f64(seconds));
            }
            "--fps" | "--orbit" | "--hide" | "--all-edges" | "--fit-topic" | "--log-events"
            | "--recalculate" | "--no-persist" | "--help" | "-h"
                if inline.is_some() =>
            {
                return Err(format!("{name} takes no value"));
            }
            "--log-events" => args.log_events = true,
            "--recalculate" => args.recalculate = true,
            "--no-persist" => args.no_persist = true,
            "--hide" => args.filter.hide = true,
            "--all-edges" => args.all_edges = true,
            "--fit-topic" => args.fit_topic = true,
            "--fps" => args.fps = true,
            "--orbit" => args.orbit = true,
            "--help" | "-h" => return Ok(Command::Help),
            _ => return Err(format!("unknown argument: {name}")),
        }
    }
    if args.fit_topic && args.focus.is_none() && args.search.is_none() {
        return Err(
            "--fit-topic fits the focused node's topic, so it needs --focus or --search".into(),
        );
    }
    if args.focus.is_some() && args.search.is_some() {
        return Err("--search focuses its first hit, so it cannot be given with --focus".into());
    }
    if args.screenshot_after.is_some() && args.screenshot.is_none() {
        return Err("--screenshot-after delays the screenshot, so it needs --screenshot".into());
    }
    if args.no_persist && args.layout_dir.is_some() {
        return Err(
            "--no-persist keeps no topic map, so it cannot be given with --layout-dir".into(),
        );
    }
    if args.dump_layout.is_some() && args.screenshot.is_some() {
        return Err("--dump-layout and --screenshot each exit, so give one of them".into());
    }
    Ok(Command::Run(Box::new(args)))
}

/// A comma-separated list, without blanks.
fn parse_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(String::from)
        .collect()
}

/// `--kinds`: the kinds named, each singular or plural, the rest off.
fn parse_kinds(text: &str) -> Result<Kinds, String> {
    let mut kinds = Kinds {
        topic: false,
        fact: false,
        inference: false,
    };
    let names = parse_list(text);
    if names.is_empty() {
        return Err("--kinds needs at least one of topic, fact, inference".into());
    }
    for name in names {
        let lower = name.to_ascii_lowercase();
        match lower.strip_suffix('s').unwrap_or(&lower) {
            "topic" => kinds.topic = true,
            "fact" => kinds.fact = true,
            "inference" => kinds.inference = true,
            _ => {
                return Err(format!(
                    "--kinds takes topic, fact and inference, not {name}"
                ));
            }
        }
    }
    Ok(kinds)
}

/// One end of `--window`: a timestamp as the hub writes them, or a date,
/// which as the end of the window means the end of that day.
fn parse_instant(text: &str, end: bool) -> Option<i64> {
    hub::parse_timestamp(text).or_else(|| {
        let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
        let day = if end { date.succ_opt()? } else { date };
        let micros = day.and_hms_opt(0, 0, 0)?.and_utc().timestamp_micros();
        Some(if end { micros - 1 } else { micros })
    })
}

/// `--window FROM..TO`, in microseconds since the Unix epoch.
fn parse_window(text: &str) -> Result<(i64, i64), String> {
    let bad = || format!("--window takes FROM..TO, two ISO dates or timestamps, not {text}");
    let (from, to) = text.split_once("..").ok_or_else(bad)?;
    let from = parse_instant(from.trim(), false).ok_or_else(bad)?;
    let to = parse_instant(to.trim(), true).ok_or_else(bad)?;
    if from > to {
        return Err(format!(
            "--window runs from the earlier time to the later, not {text}"
        ));
    }
    Ok((from, to))
}

/// The filter the command line asked for, checked against the graph: a
/// metacontext, a topic or a document that no drawn node has is refused
/// with the reason, since a filter that matches nothing is more likely a
/// mistyped id than a wish.
fn resolve_filter(args: &FilterArgs, index: &GraphIndex) -> Result<Filter, String> {
    let metacontexts = match &args.metacontexts {
        None => Metacontexts::All,
        Some(ids) => {
            let known: BTreeSet<&str> = index
                .metacontexts
                .iter()
                .flatten()
                .map(String::as_str)
                .collect();
            if let Some(unknown) = ids.iter().find(|id| !known.contains(id.as_str())) {
                return Err(format!(
                    "--metacontext: no drawn node is in {unknown:?}; the graph's metacontexts are {}",
                    known.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            Metacontexts::Only(ids.iter().cloned().collect())
        }
    };
    let topic = match &args.topic {
        None => None,
        Some(wanted) => Some(resolve_topic(wanted, index, "--topic")?),
    };
    if let Some(source) = &args.source
        && !index.sources.iter().any(|s| s.contains(source))
    {
        return Err(format!(
            "--source: no drawn node is read from document {source:?}"
        ));
    }
    Ok(Filter {
        metacontexts,
        kinds: args.kinds.unwrap_or_default(),
        topic,
        window: args.window.map(|(from, to)| {
            let window = filter::window_between(&index.time, from, to);
            match index.mode {
                TimeMode::Recorded => window,
                TimeMode::Claim => filter::clamp_window(&window, &index.time),
            }
        }),
        source: args.source.clone(),
        mode: if args.hide { Mode::Hide } else { Mode::Dim },
        ..Filter::default()
    })
}

/// A drawn topic's id, from its id or its exact name. `option` names the
/// option asked, for the reason a refusal gives.
fn resolve_topic(wanted: &str, index: &GraphIndex, option: &str) -> Result<String, String> {
    let drawn_topic = |id: &str| {
        index
            .by_id
            .get(id)
            .is_some_and(|&i| index.kinds[i] == Kind::Topic)
    };
    if drawn_topic(wanted) {
        return Ok(wanted.to_owned());
    }
    let named: BTreeSet<&str> = index
        .ids
        .iter()
        .zip(&index.names)
        .filter(|&(id, name)| name == wanted && drawn_topic(id))
        .map(|(id, _)| id.as_str())
        .collect();
    match named.len() {
        1 => Ok(named.into_iter().next().unwrap_or_default().to_owned()),
        0 => Err(format!(
            "{option}: no drawn topic has the id or the name {wanted:?}"
        )),
        _ => Err(format!(
            "{option}: {} topics are named {wanted:?}; name one by id: {}",
            named.len(),
            named.into_iter().collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The node `--focus` names, by its index: any drawn node by its id, or a
/// drawn topic by its exact name.
fn resolve_focus(wanted: &str, index: &GraphIndex) -> Result<usize, String> {
    if let Some(&node) = index.by_id.get(wanted) {
        return Ok(node);
    }
    let named =
        (0..index.ids.len()).any(|i| index.kinds[i] == Kind::Topic && index.names[i] == wanted);
    if !named {
        return Err(format!(
            "--focus: no drawn node has the id {wanted:?}, and no drawn topic has it as its name"
        ));
    }
    let topic = resolve_topic(wanted, index, "--focus")?;
    Ok(index.by_id[&topic])
}

/// The options of the page's address (`browser.rs`), read as the command
/// line's, without the native build's development options: a page cannot
/// be left to save a screenshot and exit, and has no terminal for a log of
/// frames or events.
#[cfg(any(test, target_arch = "wasm32"))]
fn browser_args(query: &str) -> Result<Command, String> {
    let command = parse_args(browser::query_args(query)?)?;
    if let Command::Run(args) = &command {
        let native_only = [
            ("screenshot", args.screenshot.is_some()),
            ("screenshot-after", args.screenshot_after.is_some()),
            ("fps", args.fps),
            ("log-events", args.log_events),
            ("layout-dir", args.layout_dir.is_some()),
            ("dump-layout", args.dump_layout.is_some()),
        ];
        if let Some((name, _)) = native_only.iter().find(|&&(_, given)| given) {
            return Err(format!(
                "{name} is for the native build, run with cargo run, and not the page"
            ));
        }
    }
    Ok(command)
}

/// The window: in a browser, the page's canvas (`index.html`), sized to the
/// page.
fn window(graph: &str) -> Window {
    Window {
        title: format!("epimemer-bevy: {graph}"),
        #[cfg(target_arch = "wasm32")]
        canvas: Some(browser::CANVAS.into()),
        #[cfg(target_arch = "wasm32")]
        fit_canvas_to_parent: true,
        ..default()
    }
}

/// Bevy's plugins, with the window.
fn plugins(graph: &str) -> PluginGroupBuilder {
    let plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(window(graph)),
        ..default()
    });
    // In a browser the asset server fetches `assets/` from the page's own
    // address, where trunk puts it. It looks for no `.meta` file beside an
    // asset, since there are none and the browser would log each one it
    // did not find as a failed request.
    #[cfg(target_arch = "wasm32")]
    let plugins = plugins.set(AssetPlugin {
        meta_check: bevy::asset::AssetMetaCheck::Never,
        ..default()
    });
    plugins
}

/// What the start reads before the window opens, ready to be put in the
/// app.
struct Start {
    prepared: Prepared,
    map: MapState,
    catalogue: Catalogue,
    live_state: LiveState,
    stream: Option<Stream>,
    filter: Filter,
    focus: Focus,
    search: Search,
    status: LoadStatus,
}

/// Why the start stopped, and the exit code that gives natively: 1 when the
/// hub or the graph fails, 2 when an option names nothing in the graph.
struct Refusal {
    message: String,
    /// The exit code, natively; a page has none.
    #[cfg_attr(target_arch = "wasm32", expect(dead_code))]
    code: u8,
}

/// Starts the event stream on the session chosen and waits a moment for its
/// first connection, so that the snapshot is asked for after the
/// subscription: the stream, where it started, and the number of the
/// connection made in time, where one was.
async fn follow(args: &Args, session: &hub::SessionInfo) -> (Option<Stream>, Option<u32>) {
    let subscription = Subscription {
        session: session.session_id.clone(),
        graphs: vec![args.request.graph.clone()],
    };
    let started = match stream::start(&args.request.hub, subscription) {
        Ok(started) => started,
        Err(error) => {
            warn!("{error}; the graph is drawn as read, with no live updates");
            return (None, None);
        }
    };
    let first = match background::recv_timeout(&started.first, FIRST_CONNECT_WAIT).await {
        Ok(Ok(connection)) => {
            info!(
                "event stream: subscribed to session {} for graph {:?} on {}",
                session.session_id,
                args.request.graph,
                stream::url(&args.request.hub)
            );
            Some(connection)
        }
        Ok(Err(error)) => {
            warn!("event stream: {error}; trying again in the background");
            None
        }
        Err(_) => {
            warn!(
                "event stream: no connection within {} s; trying again in the background",
                FIRST_CONNECT_WAIT.as_secs()
            );
            None
        }
    };
    (Some(started.stream), first)
}

/// Where the topic maps are kept, from the options: nowhere with
/// `--no-persist`, else natively the directory given or the platform's data
/// directory, and in a browser the page's local storage.
fn map_store(args: &Args) -> Store {
    if args.no_persist {
        return Store::Off;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        match args
            .layout_dir
            .clone()
            .or_else(|| persist::data_dir(|name| std::env::var(name).ok()))
        {
            Some(dir) => Store::Dir(dir),
            None => {
                warn!("no home directory is set, so the topic map is not saved");
                Store::Off
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        Store::LocalStorage
    }
}

/// Reads and places the first graph, on its saved topic map where there
/// is one, and resolves the options that name things in it.
async fn first_load(args: &Args, store: Store) -> Result<Start, Refusal> {
    let failed = |message: String| Refusal { message, code: 1 };
    let refused = |message: String| Refusal { message, code: 2 };
    let chosen = hub::choose(&args.request)
        .await
        .map_err(|error| failed(error.to_string()))?;
    // The stream subscribes before the snapshot is asked for, so that an
    // event between the two is held in the channel and applied over the
    // snapshot rather than missed.
    let (started, first_connection) = follow(args, &chosen.session).await;
    let loaded = hub::snapshot(&args.request, chosen)
        .await
        .map_err(|error| failed(error.to_string()))?;
    reload::log_loaded(&loaded);
    let key = MapKey {
        hub: args.request.hub.clone(),
        graph: args.request.graph.clone(),
    };
    let map = reload::saved_map(&store, &key);
    let mut prepared =
        reload::prepare_seeded(&loaded.snapshot, None, map.as_ref()).map_err(failed)?;
    // The time mode first, so that a window on the command line is read
    // on the axis drawn.
    time_mode::apply_mode(
        args.time,
        &prepared.time,
        &mut prepared.index,
        &mut prepared.places,
    );
    time_mode::log_mode(args.time, &prepared.time, &prepared.index.time);
    let filter = resolve_filter(&args.filter, &prepared.index).map_err(refused)?;
    let focus = match args
        .focus
        .as_deref()
        .map(|wanted| resolve_focus(wanted, &prepared.index))
    {
        None => Focus(None),
        Some(Ok(node)) => {
            info!(
                "focused on {:?} ({})",
                labels::label_text(&prepared.index.names[node], &prepared.index.ids[node]),
                prepared.index.ids[node]
            );
            Focus(Some(node))
        }
        Some(Err(message)) => return Err(refused(message)),
    };
    let (search, focus) = match args.search.as_deref() {
        None => (Search::default(), focus),
        Some(text) => {
            let visible = filter::shown(&filter, &prepared.index).visible;
            let (search, first) = search::start(&prepared.index, &visible, text);
            match first {
                Some(node) => info!(
                    "search {text:?}: {} hits; focused on the first, {:?} ({})",
                    search.hits.len(),
                    labels::label_text(&prepared.index.names[node], &prepared.index.ids[node]),
                    prepared.index.ids[node]
                ),
                None => info!("search {text:?}: no node drawn holds it"),
            }
            (search, Focus(first))
        }
    };
    let status = LoadStatus::Loaded {
        graph: prepared.graph.clone(),
        nodes: prepared.nodes.len(),
    };
    let wanted = if args.recalculate {
        Purpose::Recalculate("asked for by --recalculate".to_owned())
    } else {
        Purpose::Start
    };
    let map = MapState::new(store, key, map, wanted);
    let arrival = Arrival {
        session: loaded.session,
        sessions: loaded.sessions,
        graphs: loaded.graphs,
        prepared,
        purpose: reload::Purpose::Switch,
        map: None,
    };
    // The snapshot itself is not kept: everything drawn has been read out.
    drop(loaded.snapshot);
    let catalogue = reload::catalogue(&args.request.hub, &args.request.graph, &arrival);
    let live_state = LiveState {
        connection: if started.is_some() {
            Connection::Connecting
        } else {
            Connection::Off
        },
        subscribed_before_load: first_connection,
        following: Some((
            catalogue.session.session_id.clone(),
            catalogue.graph.clone(),
        )),
        ..LiveState::default()
    };
    Ok(Start {
        prepared: arrival.prepared,
        map,
        catalogue,
        live_state,
        stream: started,
        filter,
        focus,
        search,
        status,
    })
}

/// Puts what the start read into the app, with every plugin that draws and
/// follows it.
fn assemble(app: &mut App, args: Args, start: Start) {
    let Start {
        prepared,
        map,
        catalogue,
        live_state,
        stream,
        filter,
        focus,
        search,
        status,
    } = start;
    if let Some(stream) = stream {
        app.insert_resource(stream);
    }
    app.insert_resource(prepared.places)
        .insert_resource(prepared.live)
        .insert_resource(live_state)
        .insert_resource(Nodes(prepared.nodes))
        .insert_resource(prepared.index)
        .insert_resource(prepared.time)
        .insert_resource(args.time)
        .insert_resource(prepared.pickers)
        .insert_resource(filter)
        .insert_resource(focus)
        .insert_resource(search)
        .insert_resource(AllEdges(args.all_edges))
        .insert_resource(catalogue)
        .insert_resource(map)
        .add_plugins((
            FilterPlugin,
            ScenePlugin,
            CameraPlugin {
                orbit: args.orbit,
                start: args.preset,
                fit_topic: args.fit_topic,
            },
            PickPlugin,
            FocusPlugin,
            EdgePlugin,
            LabelPlugin,
            FontPlugin,
            ReloadPlugin,
            BarPlugin {
                start: args.ui_state,
            },
        ))
        .add_plugins((
            TimeModePlugin,
            AxisPlugin,
            BracketPlugin,
            TrayPlugin,
            PanelPlugin,
            SearchPlugin,
            RelayoutPlugin,
        ))
        .add_plugins(LivePlugin {
            log_events: args.log_events,
        })
        .add_plugins(RetrievalPlugin {
            wanted: args.filter.retrieval.clone(),
        })
        .add_plugins(ReflectPlugin)
        .insert_resource(status);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(path) = args.screenshot {
            app.add_plugins(ScreenshotPlugin {
                path,
                after: args.screenshot_after,
            });
        }
        if args.fps {
            app.add_plugins(FpsLogPlugin);
        }
        if let Some(path) = args.dump_layout {
            app.add_plugins(DumpLayoutPlugin { path });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> AppExit {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Command::Run(args)) => *args,
        Ok(Command::Help) => {
            println!("{USAGE}\n\n{}", keys::help());
            return AppExit::Success;
        }
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}\n\n{}", keys::help());
            return AppExit::from_code(2);
        }
    };
    // The plugins first, so that logging is set up for the reads below; the
    // window opens only when the app runs.
    let mut app = App::new();
    app.add_plugins(plugins(&args.request.graph));
    // The native reads block rather than wait, so the start runs to its end
    // here.
    let store = map_store(&args);
    let start = match bevy::tasks::block_on(first_load(&args, store)) {
        Ok(start) => start,
        Err(refusal) => {
            error!("{}", refusal.message);
            return AppExit::from_code(refusal.code);
        }
    };
    assemble(&mut app, args, start);
    app.run()
}

/// In a browser the start waits on the page's fetch and WebSocket, so it
/// runs as a task on the page's event loop, and the app runs once it has
/// read the graph. Until then the page says what it is doing, and if the
/// start stops, why.
#[cfg(target_arch = "wasm32")]
fn main() {
    let mut args = match browser_args(&browser::query()) {
        Ok(Command::Run(args)) => *args,
        Ok(Command::Help) => {
            browser::show_status(&format!(
                "The page's address gives the options of the command line: ?graph=memory&preset=3d for --graph memory --preset 3d.\n\n{USAGE}\n\n{}",
                keys::help()
            ));
            return;
        }
        Err(message) => {
            browser::show_failure(&message);
            return;
        }
    };
    args.request.hub = browser::page_host();
    wasm_bindgen_futures::spawn_local(async move {
        // Logging first, so that the reads are logged. The rest of Bevy's
        // plugins wait until the graph is read, and the app runs as soon as
        // they are added, with no wait between: the renderer looks for the
        // GPU in a task its plugin starts, and in a browser it needs the
        // page's canvas for that, which the window takes when the app runs.
        let mut app = App::new();
        app.add_plugins(LogPlugin::default());
        match first_load(&args, map_store(&args)).await {
            Ok(start) => {
                app.add_plugins(plugins(&args.request.graph).disable::<LogPlugin>());
                browser::hide_status();
                assemble(&mut app, args, start);
                app.run();
            }
            Err(refusal) => {
                error!("{}", refusal.message);
                browser::show_failure(&refusal.message);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn parse(line: &str) -> Result<Command, String> {
        parse_args(line.split_whitespace().map(String::from))
    }

    fn run(line: &str) -> Args {
        match parse(line) {
            Ok(Command::Run(args)) => *args,
            other => panic!("{line:?} gave {other:?}"),
        }
    }

    #[test]
    fn the_defaults_read_the_memory_graph_from_the_local_hub() {
        let args = run("");
        assert_eq!(args.request.hub, "127.0.0.1:8765");
        assert_eq!(args.request.graph, "memory");
        assert_eq!(args.request.session, None);
        assert_eq!(args.preset, Preset::Free);
        assert_eq!(args.time, TimeMode::Recorded);
        assert_eq!(args.focus, None);
        assert_eq!(args.search, None);
        assert!(!args.all_edges);
        assert_eq!(args.filter, FilterArgs::default());
        assert_eq!(args.ui_state, UiState::Open);
        assert_eq!(
            (args.screenshot, args.fit_topic, args.fps, args.orbit),
            (None, false, false, false)
        );
    }

    #[test]
    fn the_filter_options_read_as_written() {
        let args = run(
            "--kinds fact,Inferences --metacontext=the-real,night-city --topic Petritype --window 2026-10-14..2026-10-20 --source doc --retrieval latest --hide",
        );
        let filter = args.filter;
        assert_eq!(
            filter.kinds,
            Some(Kinds {
                topic: false,
                fact: true,
                inference: true
            })
        );
        assert_eq!(
            filter.metacontexts,
            Some(vec!["the-real".to_owned(), "night-city".to_owned()])
        );
        assert_eq!(filter.topic.as_deref(), Some("Petritype"));
        assert_eq!(filter.source.as_deref(), Some("doc"));
        assert_eq!(filter.retrieval.as_deref(), Some("latest"));
        assert!(filter.hide);
        let day = |text: &str| hub::parse_timestamp(text).unwrap();
        assert_eq!(
            filter.window,
            Some((day("2026-10-14T00:00:00Z"), day("2026-10-21T00:00:00Z") - 1)),
            "a date as the end takes in that whole day"
        );
        // Timestamps are taken exactly, and compared as instants: 10:30
        // an hour east of UTC is before 10:00 in UTC.
        assert_eq!(
            parse_window("2026-10-14T10:30:00+01:00..2026-10-14T10:00:00Z"),
            Ok((day("2026-10-14T09:30:00Z"), day("2026-10-14T10:00:00Z")))
        );
    }

    #[test]
    fn bad_filter_options_are_refused_with_a_reason() {
        assert!(
            parse("--kinds rocks")
                .unwrap_err()
                .contains("topic, fact and inference")
        );
        assert!(parse("--kinds ,").unwrap_err().contains("at least one"));
        assert!(
            parse("--window 2026-10-14")
                .unwrap_err()
                .contains("FROM..TO")
        );
        assert!(
            parse("--window 2026-10-14..soon")
                .unwrap_err()
                .contains("FROM..TO")
        );
        assert!(
            parse("--window 2026-10-20..2026-10-14")
                .unwrap_err()
                .contains("earlier time to the later")
        );
        assert!(parse("--hide=yes").unwrap_err().contains("takes no value"));
    }

    #[test]
    fn the_filter_is_checked_against_the_graph() {
        let snapshot =
            hub::parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        let layout = layout::place(&snapshot.nodes, &snapshot.edges);
        let index = graph_index::build(&snapshot.nodes, &snapshot.edges, &layout);
        let resolve = |args: FilterArgs| resolve_filter(&args, &index);

        let filter = resolve(FilterArgs {
            topic: Some("Swifts".into()),
            metacontexts: Some(vec!["the-real".into()]),
            source: Some("d0c00000-0000-4000-8000-000000000001".into()),
            window: Some((index.time.start, index.time.start + index.time.span)),
            hide: true,
            ..FilterArgs::default()
        })
        .unwrap();
        assert_eq!(
            filter.topic.as_deref(),
            Some("a1e0c7d2-0000-4000-8000-000000000002"),
            "by its name"
        );
        assert_eq!(
            filter.metacontexts,
            Metacontexts::Only(["the-real".to_owned()].into())
        );
        let window = filter.window.unwrap();
        assert_eq!((window.from, window.to), (0.0, layout::TIME_HEIGHT));
        assert_eq!(filter.mode, Mode::Hide);
        assert_eq!(filter.kinds, Kinds::default());

        let by_id = resolve(FilterArgs {
            topic: Some("a1e0c7d2-0000-4000-8000-000000000001".into()),
            ..FilterArgs::default()
        });
        assert_eq!(
            by_id.unwrap().topic.as_deref(),
            Some("a1e0c7d2-0000-4000-8000-000000000001")
        );
        for (args, reason) in [
            (
                FilterArgs {
                    topic: Some("Owls".into()),
                    ..FilterArgs::default()
                },
                "no drawn topic",
            ),
            (
                FilterArgs {
                    topic: Some("Common swifts winter in sub-Saharan Africa.".into()),
                    ..FilterArgs::default()
                },
                "no drawn topic",
            ),
            (
                FilterArgs {
                    metacontexts: Some(vec!["night-city".into()]),
                    ..FilterArgs::default()
                },
                "the graph's metacontexts are the-real",
            ),
            (
                FilterArgs {
                    source: Some("nowhere".into()),
                    ..FilterArgs::default()
                },
                "no drawn node is read from",
            ),
        ] {
            let error = resolve(args).unwrap_err();
            assert!(error.contains(reason), "{error}");
        }
    }

    #[test]
    fn the_focus_is_a_node_by_id_or_a_topic_by_name() {
        let snapshot =
            hub::parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        let layout = layout::place(&snapshot.nodes, &snapshot.edges);
        let index = graph_index::build(&snapshot.nodes, &snapshot.edges, &layout);
        let swifts = index.by_id["a1e0c7d2-0000-4000-8000-000000000002"];
        assert_eq!(resolve_focus("Swifts", &index), Ok(swifts));
        let fact = index.by_id["fac70000-0000-4000-8000-000000000001"];
        assert_eq!(
            resolve_focus("fac70000-0000-4000-8000-000000000001", &index),
            Ok(fact),
            "a claim by its id"
        );
        let error = resolve_focus("Owls", &index).unwrap_err();
        assert!(
            error.starts_with("--focus: no drawn node has the id"),
            "{error}"
        );
        assert!(
            error.contains("no drawn topic has it as its name"),
            "{error}"
        );
    }

    #[test]
    fn every_option_reads_with_a_space_or_an_equals_sign() {
        let args = run(
            "--hub 127.0.0.1:9000 --graph=games --session abc --screenshot=/tmp/a.png --fps --orbit --preset timeline --ui-state topics:vis --focus=Swifts --all-edges --fit-topic --time claim",
        );
        assert_eq!(args.focus.as_deref(), Some("Swifts"));
        assert_eq!(args.time, TimeMode::Claim);
        assert_eq!(run("--time=Recorded").time, TimeMode::Recorded);
        assert!(args.all_edges && args.fit_topic);
        assert_eq!(
            args.ui_state,
            UiState::Picker(bar::Picker::Topic, "vis".into())
        );
        assert_eq!(run("--ui-state=collapsed").ui_state, UiState::Collapsed);
        assert_eq!(args.preset, Preset::Timeline);
        assert_eq!(run("--preset=now").preset, Preset::Now);
        assert_eq!(args.request.hub, "127.0.0.1:9000");
        assert_eq!(args.request.graph, "games");
        assert_eq!(args.request.session.as_deref(), Some("abc"));
        assert_eq!(args.screenshot, Some(PathBuf::from("/tmp/a.png")));
        assert!(args.fps && args.orbit);
    }

    #[test]
    fn bad_arguments_are_refused_with_a_reason() {
        assert!(parse("--graph").unwrap_err().contains("needs a value"));
        assert!(parse("--graph=").unwrap_err().contains("needs a value"));
        assert!(
            parse("--hub http://127.0.0.1:8765")
                .unwrap_err()
                .contains("not a URL")
        );
        assert!(parse("--fps=yes").unwrap_err().contains("takes no value"));
        assert!(
            parse("--all-edges=on")
                .unwrap_err()
                .contains("takes no value")
        );
        assert!(
            parse("--preset sideways")
                .unwrap_err()
                .contains("graph, timeline, 3d, now")
        );
        assert!(
            parse("--time valid")
                .unwrap_err()
                .contains("--time takes recorded or claim")
        );
        assert!(
            parse("--ui-state sideways")
                .unwrap_err()
                .contains("open, collapsed")
        );
        assert!(
            parse("--colour red")
                .unwrap_err()
                .contains("unknown argument")
        );
        assert_eq!(parse("--fps --help"), Ok(Command::Help));
        assert!(
            parse("--fit-topic")
                .unwrap_err()
                .contains("needs --focus or --search")
        );
        assert!(
            parse("--focus Swifts --search swift")
                .unwrap_err()
                .contains("cannot be given with --focus")
        );
        assert!(
            parse("--screenshot-after 5")
                .unwrap_err()
                .contains("needs --screenshot")
        );
        assert!(
            parse("--screenshot a.png --screenshot-after soon")
                .unwrap_err()
                .contains("a number of seconds")
        );
        assert!(
            parse("--screenshot a.png --screenshot-after -1")
                .unwrap_err()
                .contains("a number of seconds")
        );
        assert!(
            parse("--log-events=yes")
                .unwrap_err()
                .contains("takes no value")
        );
        assert!(
            parse("--no-persist --layout-dir /tmp/maps")
                .unwrap_err()
                .contains("cannot be given with --layout-dir")
        );
        assert!(
            parse("--dump-layout a.json --screenshot a.png")
                .unwrap_err()
                .contains("give one of them")
        );
        assert!(
            parse("--recalculate=1")
                .unwrap_err()
                .contains("takes no value")
        );
    }

    #[test]
    fn the_map_options_read_as_written() {
        let args = run("--recalculate --layout-dir /tmp/maps --dump-layout=/tmp/d.json");
        assert!(args.recalculate && !args.no_persist);
        assert_eq!(args.layout_dir, Some(PathBuf::from("/tmp/maps")));
        assert_eq!(args.dump_layout, Some(PathBuf::from("/tmp/d.json")));
        assert!(matches!(map_store(&args), Store::Dir(dir) if dir == Path::new("/tmp/maps")));
        let off = run("--no-persist");
        assert!(off.no_persist);
        assert!(matches!(map_store(&off), Store::Off));
        let plain = run("");
        assert!(!plain.recalculate && plain.layout_dir.is_none() && plain.dump_layout.is_none());
    }

    #[test]
    fn the_search_option_reads_its_text_and_fits_the_topic_of_its_hit() {
        let args = run("--search=swift --fit-topic");
        assert_eq!(args.search.as_deref(), Some("swift"));
        assert!(args.fit_topic);
        assert_eq!(run("--search Common").search.as_deref(), Some("Common"));
        assert!(parse("--search").unwrap_err().contains("needs a value"));
    }

    #[test]
    fn the_page_address_gives_the_options_with_the_same_defaults() {
        let run = |query: &str| match browser_args(query) {
            Ok(Command::Run(args)) => *args,
            other => panic!("{query:?} gave {other:?}"),
        };
        let plain = run("");
        assert_eq!(plain.request.graph, "memory");
        assert_eq!(plain.preset, Preset::Free);
        assert_eq!(plain.time, TimeMode::Recorded);
        let args = run("?graph=memory&preset=timeline&time=claim&hide&kinds=fact,inference");
        assert_eq!(args.preset, Preset::Timeline);
        assert_eq!(args.time, TimeMode::Claim);
        assert!(args.filter.hide);
        assert_eq!(
            args.filter.kinds.map(|k| (k.topic, k.fact, k.inference)),
            Some((false, true, true))
        );
        assert!(
            run("?orbit=1").orbit,
            "a camera option, not a development one"
        );
        let map = run("?recalculate&no-persist");
        assert!(map.recalculate && map.no_persist);
        assert_eq!(browser_args("?help"), Ok(Command::Help));
        for (query, reason) in [
            ("?screenshot=a.png", "screenshot is for the native build"),
            ("?fps", "fps is for the native build"),
            ("?log-events=true", "log-events is for the native build"),
            ("?layout-dir=/tmp", "layout-dir is for the native build"),
            ("?dump-layout=a.json", "dump-layout is for the native build"),
            ("?preset=sideways", "--preset takes"),
            ("?hub=127.0.0.1:9000", "no CORS headers"),
        ] {
            let error = browser_args(query).unwrap_err();
            assert!(error.contains(reason), "{query}: {error}");
        }
    }

    #[test]
    fn the_live_options_read_as_written() {
        let args = run("--log-events --screenshot=a.png --screenshot-after=7.5");
        assert!(args.log_events);
        assert_eq!(args.screenshot_after, Some(Duration::from_millis(7500)));
        let plain = run("");
        assert!(!plain.log_events);
        assert_eq!(plain.screenshot_after, None);
    }
}
