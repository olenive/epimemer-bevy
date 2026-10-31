//! Development affordances, so a change can be checked without a person at
//! the window.
//!
//! - [`ScreenshotPlugin`] waits until the scene has been drawn, saves a PNG
//!   of what the camera sees and exits. The camera renders into an
//!   offscreen image for this rather than into the window, because on
//!   macOS a window reads back black while the display sleeps, which is
//!   exactly when nobody is at the window. The camera is the default UI
//!   camera, so the bar and the labels are drawn into the same image, laid
//!   out at the window's size and density. Bevy compiles render pipelines
//!   in the background and draws nothing for a mesh whose pipeline is not
//!   ready, so "drawn" means: every pipeline compiled and the camera at
//!   rest, with no change of view and no switch of time mode under way,
//!   for a run of consecutive frames. With a delay, from
//!   `--screenshot-after`, the screenshot waits until that long after the
//!   start as well, so that what the event stream brings meanwhile is in
//!   it.
//!   The topic map settles first as well: no read of the embeddings under
//!   way, wanted or due, and no move on the map (`relayout.rs`).
//! - [`DumpLayoutPlugin`] waits for the topic map to settle in the same
//!   way, writes every topic's id and place on the map to a JSON file and
//!   exits, so that two runs can be compared without a window being looked
//!   at.
//! - [`FpsLogPlugin`] logs the frame rate once a second, and how many draw
//!   calls the translucent phase, where every disc is drawn, took.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::core_pipeline::core_3d::Transparent3d;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::render_phase::{PhaseItem, ViewSortedRenderPhases};
use bevy::render::render_resource::{PipelineCache, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};

use crate::camera::{Places, Tween};
use crate::graph_index::GraphIndex;
use crate::layout::Kind;
use crate::relayout::{LayoutTween, MapState};
use crate::scene::Disc;
use crate::time_mode::ModeTween;

/// How many consecutive frames with every pipeline ready count as drawn.
const SETTLED_FRAMES: u32 = 30;
/// A scene that is not drawn by then will not be.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(60);
/// The screenshot's size in pixels: the default window, 1280 by 720, at
/// the density of a high-resolution display.
const SCREENSHOT_SIZE: (u32, u32) = (2560, 1440);
/// That density, as pixels per logical pixel, so that the bar is laid out
/// as it is in the default window.
const SCREENSHOT_SCALE: f32 = 2.0;

/// Saves a screenshot of the camera's view to `path` once the scene is
/// drawn, and no sooner than `after` from the start where that is given,
/// then exits: with success when the file was written, with an error
/// otherwise.
pub struct ScreenshotPlugin {
    pub path: PathBuf,
    pub after: Option<Duration>,
}

/// Whether the render world had no pipeline waiting to compile, as of the
/// last extract.
#[derive(Resource, Debug, Default)]
struct PipelinesReady(bool);

#[derive(Resource, Debug)]
struct ScreenshotState {
    path: PathBuf,
    /// The offscreen image the camera renders into.
    image: Option<Handle<Image>>,
    started: Instant,
    /// The least time after the start to take it.
    after: Duration,
    settled: u32,
    requested: bool,
}

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PipelinesReady::default())
            .insert_resource(ScreenshotState {
                path: self.path.clone(),
                image: None,
                started: Instant::now(),
                after: self.after.unwrap_or_default(),
                settled: 0,
                requested: false,
            })
            .add_systems(PostStartup, render_offscreen)
            .add_systems(Update, take_screenshot);
        app.sub_app_mut(RenderApp)
            .add_systems(ExtractSchedule, report_pipelines);
    }
}

/// Points the camera at an offscreen image, once it has been spawned.
fn render_offscreen(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<ScreenshotState>,
    camera: Single<Entity, With<Camera3d>>,
) {
    let (width, height) = SCREENSHOT_SIZE;
    let image = images.add(Image::new_target_texture(
        width,
        height,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    ));
    commands.entity(*camera).insert(offscreen(image.clone()));
    state.image = Some(image);
}

/// The offscreen target. The screenshot names the same target, scale factor
/// and all: Bevy tells image targets apart by both, and a capture of the
/// image at another scale factor reads a texture nothing was drawn into.
fn offscreen(image: Handle<Image>) -> RenderTarget {
    RenderTarget::Image(ImageRenderTarget {
        handle: image,
        scale_factor: SCREENSHOT_SCALE,
    })
}

fn report_pipelines(mut main_world: ResMut<MainWorld>, pipelines: Res<PipelineCache>) {
    if let Some(mut ready) = main_world.get_resource_mut::<PipelinesReady>() {
        ready.0 = pipelines.waiting_pipelines().count() == 0;
    }
}

/// Whether the topic map has settled: nothing waits to be read and
/// nothing moves on the map.
fn map_settled(map: Option<&MapState>, tween: Option<&LayoutTween>) -> bool {
    map.is_none_or(MapState::idle) && tween.is_none_or(|t| t.0.is_none())
}

/// What moves by itself and must come to rest before the screenshot: a
/// switch of time mode and the topic map.
#[derive(bevy::ecs::system::SystemParam)]
struct Moving<'w> {
    switch: Option<Res<'w, ModeTween>>,
    map: Option<Res<'w, MapState>>,
    layout: Option<Res<'w, LayoutTween>>,
}

fn take_screenshot(
    mut commands: Commands,
    mut state: ResMut<ScreenshotState>,
    ready: Res<PipelinesReady>,
    discs: Query<(), With<Disc>>,
    tweens: Query<(), With<Tween>>,
    moving: Moving,
    mut exit: MessageWriter<AppExit>,
) {
    if state.started.elapsed() > SCREENSHOT_TIMEOUT + state.after {
        error!(
            "no screenshot after {} s: the scene never settled",
            SCREENSHOT_TIMEOUT.as_secs()
        );
        exit.write(AppExit::error());
        return;
    }
    let Some(image) = state.image.clone() else {
        return;
    };
    if state.requested {
        return;
    }
    let switching = moving.switch.is_some_and(|s| s.0.is_some());
    let mapped = map_settled(moving.map.as_deref(), moving.layout.as_deref());
    state.settled = if ready.0 && !discs.is_empty() && tweens.is_empty() && !switching && mapped {
        state.settled + 1
    } else {
        0
    };
    if state.settled < SETTLED_FRAMES || state.started.elapsed() < state.after {
        return;
    }
    info!(
        "scene drawn {:.2} s after the app was assembled; taking the screenshot",
        state.started.elapsed().as_secs_f64()
    );
    state.requested = true;
    commands
        .spawn(Screenshot(offscreen(image)))
        .observe(save_and_exit(state.path.clone()));
}

/// Writes the captured image as a PNG and asks the app to exit, with an
/// error when the write failed. Bevy's own `save_to_disk` logs a failure
/// and carries on, which would leave the exit status saying nothing.
fn save_and_exit(path: PathBuf) -> impl FnMut(On<ScreenshotCaptured>, MessageWriter<AppExit>) {
    move |captured, mut exit| {
        let written = captured
            .image
            .clone()
            .try_into_dynamic()
            .map_err(|e| e.to_string())
            .and_then(|image| image.to_rgb8().save(&path).map_err(|e| e.to_string()));
        match written {
            Ok(()) => {
                info!("screenshot saved to {}", path.display());
                exit.write(AppExit::Success);
            }
            Err(e) => {
                error!("cannot save the screenshot to {}: {e}", path.display());
                exit.write(AppExit::error());
            }
        }
    }
}

/// Writes every topic's id and place on the map to `path` once the topic
/// map has settled, then exits: with success when the file was written.
pub struct DumpLayoutPlugin {
    pub path: PathBuf,
}

/// Where the dump goes, and how many consecutive settled updates it has
/// waited.
#[derive(Resource, Debug)]
struct DumpState {
    path: PathBuf,
    settled: u32,
    started: Instant,
}

impl Plugin for DumpLayoutPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DumpState {
            path: self.path.clone(),
            settled: 0,
            started: Instant::now(),
        })
        .add_systems(Update, dump_layout);
    }
}

/// One topic as the dump writes it.
#[derive(serde::Serialize)]
struct DumpedTopic<'a> {
    node_id: &'a str,
    name: &'a str,
    x: f32,
    z: f32,
}

/// The dump: the graph, the map's model, and the topics in id order.
#[derive(serde::Serialize)]
struct Dump<'a> {
    graph: &'a str,
    model_id: Option<&'a str>,
    topics: Vec<DumpedTopic<'a>>,
}

fn dump_layout(
    mut state: ResMut<DumpState>,
    map: Option<Res<MapState>>,
    layout: Option<Res<LayoutTween>>,
    index: Res<GraphIndex>,
    places: Res<Places>,
    mut exit: MessageWriter<AppExit>,
) {
    if state.started.elapsed() > SCREENSHOT_TIMEOUT {
        error!(
            "no layout dump after {} s: the topic map never settled",
            SCREENSHOT_TIMEOUT.as_secs()
        );
        exit.write(AppExit::error());
        return;
    }
    state.settled = if map_settled(map.as_deref(), layout.as_deref()) {
        state.settled + 1
    } else {
        0
    };
    if state.settled != SETTLED_FRAMES {
        return;
    }
    let mut topics: Vec<DumpedTopic> = (0..index.ids.len())
        .filter(|&i| index.kinds[i] == Kind::Topic && index.active[i])
        .filter_map(|i| {
            let p = places.points.get(i)?;
            Some(DumpedTopic {
                node_id: &index.ids[i],
                name: &index.names[i],
                x: p.x,
                z: p.z,
            })
        })
        .collect();
    topics.sort_by(|a, b| a.node_id.cmp(b.node_id));
    let dump = Dump {
        graph: map.as_deref().map_or("", |m| m.key.graph.as_str()),
        model_id: map
            .as_deref()
            .and_then(|m| m.map.as_ref())
            .map(|m| m.model()),
        topics,
    };
    let written = serde_json::to_string_pretty(&dump)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(&state.path, text).map_err(|e| e.to_string()));
    match written {
        Ok(()) => {
            info!(
                "layout of {} topics written to {}",
                dump.topics.len(),
                state.path.display()
            );
            exit.write(AppExit::Success);
        }
        Err(e) => {
            error!("cannot write the layout to {}: {e}", state.path.display());
            exit.write(AppExit::error());
        }
    }
}

/// Logs the frame rate and frame time to stderr once a second, and the
/// draw calls of the translucent phase.
pub struct FpsLogPlugin;

impl Plugin for FpsLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_systems(Update, log_fps);
        app.sub_app_mut(RenderApp)
            .add_systems(Render, log_draws.in_set(RenderSystems::PrepareBindGroups));
    }
}

/// How many draw calls a sorted phase makes from these items, counted the
/// way Bevy's renderer walks them: an item that opens a batch draws the
/// whole batch, and the items the batch covers are skipped.
pub fn draw_calls(batch_lengths: impl IntoIterator<Item = usize>) -> usize {
    let lengths: Vec<usize> = batch_lengths.into_iter().collect();
    let (mut index, mut draws) = (0, 0);
    while index < lengths.len() {
        if lengths[index] == 0 {
            index += 1;
        } else {
            draws += 1;
            index += lengths[index];
        }
    }
    draws
}

/// Logs, once a second, how many items the translucent phase holds and how
/// many draw calls it makes of them, after the renderer has batched them.
fn log_draws(phases: Res<ViewSortedRenderPhases<Transparent3d>>, mut next: Local<Option<Instant>>) {
    let now = Instant::now();
    if next.is_some_and(|at| now < at) {
        return;
    }
    *next = Some(now + Duration::from_secs(1));
    for phase in phases.values() {
        let lengths = phase.items.values().map(|item| item.batch_range().len());
        info!(
            "translucent phase: {} items in {} draw calls",
            phase.items.len(),
            draw_calls(lengths)
        );
    }
}

fn log_fps(diagnostics: Res<DiagnosticsStore>, time: Res<Time<Real>>, mut next: Local<f64>) {
    let now = time.elapsed_secs_f64();
    if now < *next {
        return;
    }
    *next = now + 1.0;
    let fps = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS);
    let frame = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME);
    let (Some(fps), Some(frame)) = (fps, frame) else {
        return;
    };
    let worst = frame.values().copied().fold(0.0_f64, f64::max);
    if let (Some(rate), Some(ms)) = (fps.smoothed(), frame.smoothed()) {
        info!(
            "fps {rate:.1}, frame {ms:.2} ms, worst {worst:.2} ms over the last {} frames",
            frame.history_len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_counts_as_one_draw_and_the_items_it_covers_as_none() {
        assert_eq!(draw_calls([]), 0);
        assert_eq!(draw_calls([3, 0, 0]), 1, "one batch of three");
        assert_eq!(draw_calls([1, 1, 1]), 3, "three batches of one");
        assert_eq!(
            draw_calls([2, 0, 0, 2, 0]),
            2,
            "an item a batch does not cover is skipped"
        );
    }
}
