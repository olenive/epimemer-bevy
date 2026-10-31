//! The camera: the presets of DESIGN.md §6 and the free camera between them.
//!
//! The keys in `keys.rs` ask for a view with a [`ViewRequest`], and `T`
//! asks for the region of the focused node's topic (`focus.rs`); the view
//! itself comes from `presets.rs`, and the camera
//! tweens to it over [`TWEEN_SECONDS`], switching projection at the start. A
//! request during a tween retargets it from wherever the camera has got to.
//! Between tweens the camera is free: hold the right mouse button and drag
//! to look; W, A, S and D move along the view, Q and E move down and up; the
//! scroll wheel makes the movement faster or slower. In an orthographic view
//! moving forward changes nothing on the screen, so there W and S zoom
//! instead, by as much as moving that far would in perspective.
//!
//! Every view frames the nodes the filter leaves visible (`filter.rs`):
//! all of them while it dims, only the passing ones while it hides. `M`
//! passes over a metacontext with no visible node.
//!
//! The camera keeps the point it looks at as a depth along its view, in
//! [`FlyCamera::look_depth`]: a view sets it, moving the camera carries it
//! along, and an orthographic zoom brings it nearer. The labels rank the
//! nodes by their distance from that point when nothing is focused
//! (`labels.rs`).
//!
//! The bar's view buttons ask for the same [`ViewRequest`] the keys do. The
//! wheel over the bar scrolls the bar, and while a text field in the bar
//! takes the keyboard the keys go to it (`ui_focus.rs`). The camera is also
//! the default UI camera, so the bar is drawn into whatever it renders to:
//! the window, or the offscreen image of `--screenshot`.
//!
//! A graph switch (`reload.rs`) calls [`refit_new_graph`]: the 3D preset of
//! the new graph, at a speed fitted to its size. When the topic map moves
//! every topic (`relayout.rs`), [`ViewRequest::Again`] fits the view last
//! asked for again to where the nodes now are.
//!
//! Every view is fitted to the part of the window below the bar, which the
//! bar reports as [`BarCover`], and left of the side panel while it is
//! open, which the panel reports as [`PanelCover`]. The bar is laid out only after the camera
//! has been placed, so the starting view is fitted again, once, as soon as
//! the bar's height is known; a view chosen meanwhile is left alone. With
//! `fit_topic` set, a development affordance, the camera then fits the
//! focused node's topic, as `T` does.
//!
//! With `orbit` on, a development affordance, the camera circles the graph
//! by itself so that the frame rate can be measured while the view moves,
//! and the keys do nothing.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use bevy::camera::ScalingMode;
use bevy::ecs::system::SystemParam;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::filter::Shown;
use crate::focus::Focus;
use crate::keys::{MOVE_KEYS, Move, VIEW_KEYS};
use crate::presets::{
    self, FOV, FREE_PITCH, FREE_YAW, Lens, NowLayer, Preset, Region, Screen, TWEEN_SECONDS, View,
};
use crate::ui_focus::UiFocus;

/// Far enough that no part of the graph is clipped from any view. An
/// orthographic view's near plane is as far behind the camera, so nothing
/// behind it is clipped either: a fit may put the camera inside the stack.
const FAR: f32 = 20_000.0;
/// How far the view turns per pixel of mouse movement.
const LOOK_PER_PIXEL: f32 = 0.003;
/// The view turns at most to straight up or straight down, where the graph
/// preset looks, so it never turns over.
const PITCH_LIMIT: f32 = FRAC_PI_2;
/// The starting speed, as a fraction of the graph's size per second.
const START_SPEED: f32 = 0.15;
/// How much one notch of the wheel changes the speed, and how far the speed
/// may go from where it started, either way.
const SPEED_STEP: f32 = 1.2;
const SPEED_RANGE: f32 = 100.0;
/// A trackpad scrolls in pixels; this many make one notch.
const PIXELS_PER_LINE: f32 = 24.0;
/// The closest an orthographic view zooms, as the height it spans.
const MIN_ORTHOGRAPHIC_HEIGHT: f32 = 1.0;
/// How fast `orbit` circles the graph, in radians per second.
const ORBIT_RATE: f32 = 0.3;

/// Where the nodes are and the regions a view can be fitted to: what the
/// presets frame. `points` are in node order, the order the discs carry.
#[derive(Resource, Debug, Clone, Default)]
pub struct Places {
    /// Each node's place: its disc, or the middle of its column.
    pub points: Vec<Vec3>,
    /// The stretch of the time axis each node spans, low end first, in the
    /// time mode drawn; `None` for a node with no position in it. Empty
    /// where every node is a disc at its point.
    pub spans: Vec<Option<(f32, f32)>>,
    /// Every topic's region, keyed by the topic node's index.
    pub topics: HashMap<usize, Region>,
    /// The metacontexts with a drawn node, in the order `M` visits them.
    pub metacontexts: Vec<Region>,
    /// The layer the "now" preset frames, in the time mode drawn.
    pub now: NowLayer,
}

/// The fraction of the viewport's height, from the top, that the bar
/// covers, once the bar has been laid out; `None` before then and without
/// a bar.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct BarCover(pub Option<f32>);

/// The fraction of the viewport's width, from the right, that the side
/// panel covers while it is open (`panel.rs`); none while it is collapsed
/// and without a panel.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelCover(pub f32);

/// A view the camera is asked to go to.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewRequest {
    Preset(Preset),
    /// The 3D preset, and the starting speed.
    Reset,
    /// Every node, from the camera's direction and in its projection.
    FitAll,
    /// The next metacontext's nodes, from the camera's direction and in its
    /// projection.
    NextMetacontext,
    /// The region of the focused node's topic, from the camera's direction
    /// and in its projection: the topic itself where a topic is focused, the
    /// topic it was placed beside where a claim is.
    FocusedTopic,
    /// The view last asked for, fitted again to where the nodes are now:
    /// the same preset, or the same region, which `M` does not advance.
    Again,
}

/// The free camera's state: where it looks, as yaw and pitch, how far
/// ahead the point it looks at is, and how fast it moves.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct FlyCamera {
    pub yaw: f32,
    pub pitch: f32,
    /// How far along the view the point the camera looks at is: a view's
    /// focus, as `presets::transform` stands the camera back from it.
    pub look_depth: f32,
    pub speed: f32,
    pub start_speed: f32,
}

/// The point a camera looks at.
pub fn look_at(transform: &Transform, fly: &FlyCamera) -> Vec3 {
    transform.translation + *transform.forward() * fly.look_depth
}

/// The topic whose region holds a node, by the topic's index: the node
/// itself where it is a topic, else the topic whose region lists it, the
/// lowest index where several do. `None` for a node in no region, such as
/// an unfiled claim.
pub fn region_of(topics: &HashMap<usize, Region>, node: usize) -> Option<usize> {
    if topics.contains_key(&node) {
        return Some(node);
    }
    topics
        .iter()
        .filter(|(_, region)| region.members.contains(&node))
        .map(|(&topic, _)| topic)
        .min()
}

/// A change of view under way; the free controls wait while it runs.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    from: View,
    to: View,
    elapsed: f32,
}

/// The metacontext `M` framed last, as an index into `Places::metacontexts`.
#[derive(Resource, Debug, Default)]
struct MetacontextCursor(Option<usize>);

/// The preset the camera starts at.
#[derive(Resource, Debug, Clone, Copy)]
struct StartPreset(Preset);

/// The view last asked for, which [`ViewRequest::Again`] fits again.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
struct LastView(ViewRequest);

/// Whether the camera is still to fit the focused node's topic once the
/// starting view has been fitted below the bar.
#[derive(Resource, Debug, Clone, Copy)]
struct StartTopic(bool);

pub struct CameraPlugin {
    /// Circle the graph by itself instead of following the input. The orbit
    /// is a perspective view, so it starts from the 3D preset.
    pub orbit: bool,
    pub start: Preset,
    /// Fit the focused node's topic, as `T` does, once the starting view
    /// has been fitted below the bar.
    pub fit_topic: bool,
}

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ViewRequest>()
            .insert_resource(StartPreset(if self.orbit {
                Preset::Free
            } else {
                self.start
            }))
            .insert_resource(StartTopic(self.fit_topic && !self.orbit))
            .insert_resource(LastView(ViewRequest::Preset(if self.orbit {
                Preset::Free
            } else {
                self.start
            })))
            .init_resource::<MetacontextCursor>()
            .init_resource::<UiFocus>()
            .init_resource::<Focus>()
            .init_resource::<BarCover>()
            .add_systems(Startup, spawn_camera);
        if self.orbit {
            app.add_systems(Update, orbit);
        } else {
            app.add_systems(
                Update,
                (
                    fit_start_below_bar,
                    fit_topic_at_start,
                    request_from_keys,
                    start_tween,
                    advance_tween,
                    fly,
                )
                    .chain(),
            );
        }
    }
}

/// The speed after the wheel turned this many notches, kept within
/// [`SPEED_RANGE`] of the starting speed.
pub fn speed_after_scroll(speed: f32, start_speed: f32, notches: f32) -> f32 {
    (speed * SPEED_STEP.powf(notches)).clamp(start_speed / SPEED_RANGE, start_speed * SPEED_RANGE)
}

/// The projection for a view.
pub fn projection(view: &View) -> Projection {
    match view.lens {
        Lens::Perspective => Projection::Perspective(PerspectiveProjection {
            fov: FOV,
            far: FAR,
            ..default()
        }),
        Lens::Orthographic => Projection::Orthographic(OrthographicProjection {
            near: -FAR,
            far: FAR,
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: view.height,
            },
            ..OrthographicProjection::default_3d()
        }),
    }
}

/// The height an orthographic projection spans, or `None` for any other.
pub fn orthographic_height(projection: &Projection) -> Option<f32> {
    match projection {
        Projection::Orthographic(o) => Some(match o.scaling_mode {
            ScalingMode::FixedVertical { viewport_height } => viewport_height * o.scale,
            _ => o.area.height(),
        }),
        _ => None,
    }
}

fn lens(projection: &Projection) -> Lens {
    match projection {
        Projection::Orthographic(_) => Lens::Orthographic,
        _ => Lens::Perspective,
    }
}

fn aspect(window: &Window) -> f32 {
    let (width, height) = (window.width(), window.height());
    if height > 0.0 { width / height } else { 1.0 }
}

/// The window, and the bar and the side panel over it: what a view is
/// fitted to.
#[derive(SystemParam)]
struct Viewport<'w, 's> {
    window: Single<'w, 's, &'static Window, With<PrimaryWindow>>,
    cover: Res<'w, BarCover>,
    panel: Option<Res<'w, PanelCover>>,
}

/// The screen a view is fitted to: the window's shape, less the bar and
/// the side panel.
fn screen(viewport: &Viewport) -> Screen {
    Screen {
        aspect: aspect(&viewport.window),
        inset: viewport.cover.0.unwrap_or(0.0),
        right: viewport.panel.as_ref().map_or(0.0, |panel| panel.0),
    }
}

/// The starting speed for a graph with nodes at these points: a fraction of
/// the graph's size per second.
pub fn start_speed(points: &[Vec3]) -> f32 {
    let size = presets::bounds(points).map_or(1.0, |(lo, hi)| (hi - lo).length().max(1.0));
    size * START_SPEED
}

/// After a graph switch: the 3D preset of the new graph, the speed fitted to
/// its size, and `M` starting again from its first metacontext. Reads the
/// new graph's [`Places`], which the caller has inserted.
pub fn refit_new_graph(world: &mut World) {
    let speed = start_speed(&world.resource::<Places>().points);
    if let Some(mut cursor) = world.get_resource_mut::<MetacontextCursor>() {
        cursor.0 = None;
    }
    let mut cameras = world.query::<&mut FlyCamera>();
    for mut fly in cameras.iter_mut(world) {
        fly.speed = speed;
        fly.start_speed = speed;
    }
    world.write_message(ViewRequest::Preset(Preset::Free));
}

fn spawn_camera(
    mut commands: Commands,
    places: Res<Places>,
    shown: Res<Shown>,
    start: Res<StartPreset>,
    viewport: Viewport,
) {
    let view = presets::preset_view(
        start.0,
        &places.points,
        &places.spans,
        &shown.visible,
        places.now,
        screen(&viewport),
    );
    let speed = start_speed(&places.points);
    commands.spawn((
        Camera3d::default(),
        IsDefaultUiCamera,
        projection(&view),
        presets::transform(&view),
        FlyCamera {
            yaw: view.yaw,
            pitch: view.pitch,
            look_depth: presets::distance(view.height),
            speed,
            start_speed: speed,
        },
    ));
}

/// Fits the starting view again, once, when the bar's height is first
/// known, unless a change of view has started meanwhile.
fn fit_start_below_bar(
    mut done: Local<bool>,
    start: Res<StartPreset>,
    places: Res<Places>,
    shown: Res<Shown>,
    requests: MessageReader<ViewRequest>,
    viewport: Viewport,
    camera: Single<(&mut Transform, &mut FlyCamera, &mut Projection, Has<Tween>)>,
) {
    if *done || viewport.cover.0.is_none() {
        return;
    }
    *done = true;
    let (mut transform, mut fly, mut projection, tweening) = camera.into_inner();
    if tweening || !requests.is_empty() {
        return;
    }
    let view = presets::preset_view(
        start.0,
        &places.points,
        &places.spans,
        &shown.visible,
        places.now,
        screen(&viewport),
    );
    *transform = presets::transform(&view);
    *projection = self::projection(&view);
    fly.yaw = view.yaw;
    fly.pitch = view.pitch;
    fly.look_depth = presets::distance(view.height);
}

/// Asks for the focused node's topic once the bar's height is known, when
/// the app started with `fit_topic`.
fn fit_topic_at_start(
    mut pending: ResMut<StartTopic>,
    cover: Res<BarCover>,
    mut requests: MessageWriter<ViewRequest>,
) {
    if pending.0 && cover.0.is_some() {
        pending.0 = false;
        requests.write(ViewRequest::FocusedTopic);
    }
}

fn request_from_keys(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<UiFocus>,
    mut requests: MessageWriter<ViewRequest>,
) {
    if focus.typing {
        return;
    }
    for key in &VIEW_KEYS {
        if keys.just_pressed(key.code) {
            requests.write(key.request);
        }
    }
}

/// Turns the latest request into a tween from where the camera is now.
/// A region keeps the direction and projection the camera has, or is
/// heading for when a tween is under way.
/// What a view can be fitted to: where the nodes are, which the filter
/// leaves visible, and which is focused.
#[derive(SystemParam)]
struct Framing<'w> {
    places: Res<'w, Places>,
    shown: Res<'w, Shown>,
    focus: Res<'w, Focus>,
}

fn start_tween(
    mut commands: Commands,
    mut requests: MessageReader<ViewRequest>,
    framing: Framing,
    mut cursor: ResMut<MetacontextCursor>,
    mut last: ResMut<LastView>,
    viewport: Viewport,
    camera: Single<(
        Entity,
        &Transform,
        &mut FlyCamera,
        &Projection,
        Option<&Tween>,
    )>,
) {
    let Some(asked) = requests.read().last().copied() else {
        return;
    };
    let again = asked == ViewRequest::Again;
    let request = if again { last.0 } else { asked };
    if !again {
        last.0 = request;
    }
    let (entity, transform, mut fly, projection, tween) = camera.into_inner();
    let Framing {
        places,
        shown,
        focus,
    } = framing;
    let (yaw, pitch, lens) = match tween {
        Some(tween) => (tween.to.yaw, tween.to.pitch, tween.to.lens),
        None => (fly.yaw, fly.pitch, lens(projection)),
    };
    let aspect = screen(&viewport);
    let (points, spans, visible) = (&places.points, &places.spans, &shown.visible);
    let region = |region: &Region| {
        presets::region_view(region, points, spans, visible, yaw, pitch, lens, aspect)
    };
    let target = match request {
        ViewRequest::Preset(preset) => {
            presets::preset_view(preset, points, spans, visible, places.now, aspect)
        }
        ViewRequest::Reset if again => {
            presets::preset_view(Preset::Free, points, spans, visible, places.now, aspect)
        }
        ViewRequest::Reset => {
            fly.speed = fly.start_speed;
            presets::preset_view(Preset::Free, points, spans, visible, places.now, aspect)
        }
        ViewRequest::FitAll => presets::fit(
            &presets::visible_points(points, spans, visible),
            yaw,
            pitch,
            lens,
            aspect,
        ),
        ViewRequest::NextMetacontext => {
            let count = places.metacontexts.len();
            let start = match (again, cursor.0) {
                (true, Some(i)) => i,
                (_, current) => current.map_or(0, |i| i + 1),
            };
            let Some(next) = (start..start + count)
                .map(|i| i % count)
                .find(|&i| presets::region_visible(&places.metacontexts[i], visible))
            else {
                info!("no visible node has a metacontext");
                return;
            };
            cursor.0 = Some(next);
            let chosen = &places.metacontexts[next];
            info!(
                "metacontext {} of {count}: {:?} ({}), {} nodes",
                next + 1,
                chosen.name,
                chosen.id,
                chosen.members.len()
            );
            region(chosen)
        }
        ViewRequest::FocusedTopic => {
            let Some(focused) = focus.0 else {
                info!("nothing is focused, so there is no topic to fit");
                return;
            };
            let Some(topic) =
                region_of(&places.topics, focused).and_then(|t| places.topics.get(&t))
            else {
                info!("the focused node is in no topic's region");
                return;
            };
            info!(
                "topic {:?} ({}), {} nodes",
                topic.name,
                topic.id,
                topic.members.len()
            );
            region(topic)
        }
        // Never recorded as the last view, so never read back here.
        ViewRequest::Again => return,
    };
    let from = presets::current_view(
        transform,
        fly.yaw,
        fly.pitch,
        orthographic_height(projection),
        target.focus,
    );
    commands.entity(entity).insert(Tween {
        from,
        to: target,
        elapsed: 0.0,
    });
}

fn advance_tween(
    mut commands: Commands,
    time: Res<Time>,
    camera: Single<(
        Entity,
        &mut Transform,
        &mut FlyCamera,
        &mut Projection,
        &mut Tween,
    )>,
) {
    let (entity, mut transform, mut fly, mut projection, mut tween) = camera.into_inner();
    tween.elapsed += time.delta_secs();
    let t = (tween.elapsed / TWEEN_SECONDS).min(1.0);
    let view = presets::between(&tween.from, &tween.to, presets::ease_in_out(t));
    *transform = presets::transform(&view);
    fly.yaw = view.yaw;
    fly.pitch = view.pitch;
    fly.look_depth = presets::distance(view.height);
    let changed = lens(&projection) != view.lens
        || orthographic_height(&projection).is_some_and(|h| h != view.height);
    if changed {
        *projection = self::projection(&view);
    }
    if t >= 1.0 {
        commands.entity(entity).remove::<Tween>();
    }
}

fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    focus: Res<UiFocus>,
    camera: Single<(&mut Transform, &mut FlyCamera, &mut Projection), Without<Tween>>,
) {
    let (mut transform, mut fly, mut projection) = camera.into_inner();

    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / PIXELS_PER_LINE,
    };
    if notches != 0.0 && !focus.pointer_over_ui {
        fly.speed = speed_after_scroll(fly.speed, fly.start_speed, notches);
    }

    if buttons.pressed(MouseButton::Right) && motion.delta != Vec2::ZERO {
        fly.yaw -= motion.delta.x * LOOK_PER_PIXEL;
        fly.pitch = (fly.pitch - motion.delta.y * LOOK_PER_PIXEL).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        transform.rotation = presets::rotation(fly.yaw, fly.pitch);
    }

    let forward = *transform.forward();
    let right = *transform.right();
    let heading: Vec3 = MOVE_KEYS
        .iter()
        .filter(|&&(code, _)| keys.pressed(code) && !focus.typing)
        .map(|&(_, direction)| match direction {
            Move::Forward => forward,
            Move::Back => -forward,
            Move::Right => right,
            Move::Left => -right,
            Move::Up => Vec3::Y,
            Move::Down => -Vec3::Y,
        })
        .sum();
    let step = heading.normalize_or_zero() * fly.speed * time.delta_secs();
    if step == Vec3::ZERO {
        return;
    }
    transform.translation += step;
    let ahead = step.dot(forward);
    if ahead != 0.0
        && matches!(*projection, Projection::Orthographic(_))
        && let Projection::Orthographic(ortho) = &mut *projection
        && let ScalingMode::FixedVertical { viewport_height } = &mut ortho.scaling_mode
    {
        *viewport_height =
            (*viewport_height - 2.0 * ahead * (FOV / 2.0).tan()).max(MIN_ORTHOGRAPHIC_HEIGHT);
        fly.look_depth = presets::distance(*viewport_height);
    }
}

fn orbit(
    time: Res<Time>,
    places: Res<Places>,
    shown: Res<Shown>,
    viewport: Viewport,
    camera: Single<(&mut Transform, &mut FlyCamera)>,
) {
    let view = presets::fit(
        &presets::visible_points(&places.points, &places.spans, &shown.visible),
        FREE_YAW + time.elapsed_secs() * ORBIT_RATE,
        FREE_PITCH,
        Lens::Perspective,
        screen(&viewport),
    );
    let (mut transform, mut fly) = camera.into_inner();
    *transform = presets::transform(&view);
    fly.look_depth = presets::distance(view.height);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::filter::Mode;
    use crate::presets::{GRAPH_PITCH, GRAPH_YAW};

    /// One frame of the headless app.
    const FRAME: Duration = Duration::from_millis(50);
    /// The default window's aspect ratio, which the headless app's window has.
    const ASPECT: f32 = 1280.0 / 720.0;

    /// Two topics with a claim each, and two metacontexts.
    fn places() -> Places {
        let region = |id: &str, members: &[usize]| Region {
            id: id.into(),
            name: id.into(),
            members: members.to_vec(),
        };
        Places {
            points: vec![
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(-100.0, 20.0, 30.0),
                Vec3::new(104.0, 280.0, 3.0),
                Vec3::new(-96.0, 150.0, 25.0),
                Vec3::new(0.0, 300.0, -120.0),
            ],
            spans: Vec::new(),
            topics: [(0, region("t0", &[0, 2])), (1, region("t1", &[1, 3]))]
                .into_iter()
                .collect(),
            metacontexts: vec![region("a", &[0, 1, 3]), region("b", &[2, 4])],
            now: NowLayer::Latest(30.0),
        }
    }

    /// The camera systems with no window on the screen and no renderer,
    /// each update one [`FRAME`] long.
    fn app(start: Preset) -> App {
        app_showing(start, Shown::default())
    }

    /// The same, with the filter showing what `shown` says.
    fn app_showing(start: Preset, shown: Shown) -> App {
        app_with(start, shown, false)
    }

    /// The same, fitting the focused node's topic at the start or not.
    fn app_with(start: Preset, shown: Shown, fit_topic: bool) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AccumulatedMouseScroll>()
            .insert_resource(places())
            .insert_resource(shown)
            .add_plugins(CameraPlugin {
                orbit: false,
                start,
                fit_topic,
            });
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.update();
        app
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

    fn request(app: &mut App, request: ViewRequest) {
        app.world_mut().write_message(request);
        app.update();
    }

    /// Focuses a node and asks for its topic, as `T` does.
    fn fit_topic_of(app: &mut App, node: usize) {
        app.insert_resource(Focus(Some(node)));
        request(app, ViewRequest::FocusedTopic);
    }

    /// Long enough for any tween to end.
    fn settle(app: &mut App) {
        for _ in 0..15 {
            app.update();
        }
    }

    fn camera(app: &mut App) -> (Transform, Projection, FlyCamera, bool) {
        let mut query = app
            .world_mut()
            .query::<(&Transform, &Projection, &FlyCamera, Has<Tween>)>();
        let (transform, projection, fly, tweening) = query.single(app.world()).unwrap();
        (*transform, projection.clone(), *fly, tweening)
    }

    fn assert_at(app: &mut App, view: &View) {
        let (transform, projection, fly, tweening) = camera(app);
        let expected = presets::transform(view);
        assert!(!tweening, "the tween has ended");
        assert!(
            transform.translation.distance(expected.translation) < 1e-2,
            "at {} and not {}",
            transform.translation,
            expected.translation
        );
        assert!((transform.forward().as_vec3() - expected.forward().as_vec3()).length() < 1e-4);
        assert!((transform.up().as_vec3() - expected.up().as_vec3()).length() < 1e-4);
        assert_eq!(lens(&projection), view.lens);
        if view.lens == Lens::Orthographic {
            let height = orthographic_height(&projection).unwrap();
            assert!((height - view.height).abs() < 1e-2, "{height}");
        }
        assert!(presets::rotation(fly.yaw, fly.pitch).angle_between(expected.rotation) < 2e-3);
    }

    fn preset(preset: Preset) -> View {
        let places = places();
        presets::preset_view(preset, &places.points, &[], &[], places.now, ASPECT)
    }

    #[test]
    fn the_camera_starts_at_the_preset_asked_for() {
        for start in [Preset::Free, Preset::Graph, Preset::Timeline, Preset::Now] {
            let mut app = app(start);
            assert_at(&mut app, &preset(start));
        }
    }

    #[test]
    fn every_preset_key_lands_where_its_preset_says() {
        let mut app = app(Preset::Free);
        for (key, target) in [
            (KeyCode::Digit1, Preset::Graph),
            (KeyCode::Digit2, Preset::Timeline),
            (KeyCode::Digit3, Preset::Free),
            (KeyCode::KeyN, Preset::Now),
            (KeyCode::Digit2, Preset::Timeline),
            (KeyCode::KeyR, Preset::Free),
        ] {
            press(&mut app, key);
            let (_, projection, _, tweening) = camera(&mut app);
            assert!(tweening, "{key:?} starts a tween");
            assert_eq!(
                lens(&projection),
                preset(target).lens,
                "switched at the start"
            );
            settle(&mut app);
            assert_at(&mut app, &preset(target));
        }
    }

    #[test]
    fn reset_restores_the_starting_speed() {
        let mut app = app(Preset::Graph);
        let mut query = app.world_mut().query::<&mut FlyCamera>();
        query.single_mut(app.world_mut()).unwrap().speed *= 7.0;
        press(&mut app, KeyCode::KeyR);
        settle(&mut app);
        assert_at(&mut app, &preset(Preset::Free));
        let (_, _, fly, _) = camera(&mut app);
        assert_eq!(fly.speed, fly.start_speed);
    }

    #[test]
    fn a_key_during_a_tween_retargets_it() {
        let mut app = app(Preset::Free);
        press(&mut app, KeyCode::Digit1);
        app.update();
        let (midway, _, _, tweening) = camera(&mut app);
        assert!(tweening);
        press(&mut app, KeyCode::Digit2);
        let (next, _, _, _) = camera(&mut app);
        assert!(
            next.translation.distance(midway.translation) < 100.0,
            "the new tween starts where the old one had got to"
        );
        settle(&mut app);
        assert_at(&mut app, &preset(Preset::Timeline));
    }

    #[test]
    fn a_topic_keeps_the_direction_and_projection_and_f_fits_everything_again() {
        let places = places();
        let mut app = app(Preset::Graph);
        request(&mut app, ViewRequest::FocusedTopic);
        assert!(!camera(&mut app).3, "with nothing focused T does nothing");
        // Node 3 is a claim of topic 1.
        fit_topic_of(&mut app, 3);
        settle(&mut app);
        let topic = presets::region_view(
            &places.topics[&1],
            &places.points,
            &[],
            &[],
            GRAPH_YAW,
            GRAPH_PITCH,
            Lens::Orthographic,
            ASPECT,
        );
        assert_at(&mut app, &topic);
        press(&mut app, KeyCode::KeyF);
        settle(&mut app);
        assert_at(&mut app, &preset(Preset::Graph));

        // From the 3D preset, in perspective, and asked for midway through a
        // tween toward it: the tween's target sets the direction.
        press(&mut app, KeyCode::Digit3);
        app.update();
        app.insert_resource(Focus(Some(0)));
        press(&mut app, KeyCode::KeyT);
        settle(&mut app);
        let topic = presets::region_view(
            &places.topics[&0],
            &places.points,
            &[],
            &[],
            FREE_YAW,
            FREE_PITCH,
            Lens::Perspective,
            ASPECT,
        );
        assert_at(&mut app, &topic);
        fit_topic_of(&mut app, 99);
        assert!(
            !camera(&mut app).3,
            "a node in no topic's region does nothing"
        );
    }

    #[test]
    fn fit_topic_fits_the_focused_nodes_topic_once_the_bar_is_laid_out() {
        let places = places();
        let mut app = app_with(Preset::Free, Shown::default(), true);
        app.insert_resource(Focus(Some(3)));
        settle(&mut app);
        assert_at(&mut app, &preset(Preset::Free));
        app.insert_resource(BarCover(Some(0.1)));
        settle(&mut app);
        let screen = Screen {
            aspect: ASPECT,
            inset: 0.1,
            right: 0.0,
        };
        let topic = presets::region_view(
            &places.topics[&1],
            &places.points,
            &[],
            &[],
            FREE_YAW,
            FREE_PITCH,
            Lens::Perspective,
            screen,
        );
        assert_at(&mut app, &topic);
    }

    #[test]
    fn a_node_belongs_to_its_own_topic_or_the_region_listing_it() {
        let topics = places().topics;
        assert_eq!(region_of(&topics, 1), Some(1), "a topic is its own");
        assert_eq!(region_of(&topics, 2), Some(0));
        assert_eq!(region_of(&topics, 3), Some(1));
        assert_eq!(region_of(&topics, 4), None, "unfiled");
    }

    #[test]
    fn the_point_looked_at_follows_every_view_and_the_orthographic_zoom() {
        let mut app = app(Preset::Free);
        let (transform, _, fly, _) = camera(&mut app);
        let free = preset(Preset::Free);
        assert!(look_at(&transform, &fly).distance(free.focus) < 1e-2);
        press(&mut app, KeyCode::Digit1);
        settle(&mut app);
        let (transform, _, fly, _) = camera(&mut app);
        let graph = preset(Preset::Graph);
        assert!(look_at(&transform, &fly).distance(graph.focus) < 1e-2);
        // W zooms an orthographic view in on the point it looks at.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        let (transform, projection, fly, _) = camera(&mut app);
        assert!(orthographic_height(&projection).unwrap() < graph.height);
        assert!(look_at(&transform, &fly).distance(graph.focus) < 1e-2);
    }

    #[test]
    fn again_fits_the_view_last_asked_for_to_where_the_nodes_now_are() {
        let mut app = app(Preset::Timeline);
        press(&mut app, KeyCode::Digit1);
        settle(&mut app);
        assert_at(&mut app, &preset(Preset::Graph));
        // The map moves under the view, as a new topic map moves it.
        let moved: Vec<Vec3> = places().points.iter().map(|p| *p * 2.0).collect();
        app.world_mut().resource_mut::<Places>().points = moved.clone();
        request(&mut app, ViewRequest::Again);
        settle(&mut app);
        let expected = presets::preset_view(Preset::Graph, &moved, &[], &[], places().now, ASPECT);
        assert_at(&mut app, &expected);
        // M is fitted again where it is, not advanced.
        press(&mut app, KeyCode::KeyM);
        settle(&mut app);
        request(&mut app, ViewRequest::Again);
        settle(&mut app);
        let first = presets::region_view(
            &places().metacontexts[0],
            &moved,
            &[],
            &[],
            GRAPH_YAW,
            GRAPH_PITCH,
            Lens::Orthographic,
            ASPECT,
        );
        assert_at(&mut app, &first);
    }

    #[test]
    fn m_visits_each_metacontext_in_turn() {
        let places = places();
        let mut app = app(Preset::Timeline);
        for expected in [0, 1, 0] {
            press(&mut app, KeyCode::KeyM);
            settle(&mut app);
            let view = presets::region_view(
                &places.metacontexts[expected],
                &places.points,
                &[],
                &[],
                presets::TIMELINE_YAW,
                presets::TIMELINE_PITCH,
                Lens::Orthographic,
                ASPECT,
            );
            assert_at(&mut app, &view);
        }
    }

    #[test]
    fn hidden_nodes_leave_every_fit_and_m_passes_over_their_metacontext() {
        let places = places();
        // Hidden: metacontext a's nodes 0, 1 and 3, so b's 2 and 4 remain.
        let shown = Shown {
            mode: Mode::Hide,
            passes: vec![false, false, true, false, true],
            visible: vec![false, false, true, false, true],
        };
        let mut app = app_showing(Preset::Graph, shown.clone());
        let left = [places.points[2], places.points[4]];
        let graph = presets::fit(&left, GRAPH_YAW, GRAPH_PITCH, Lens::Orthographic, ASPECT);
        assert_at(&mut app, &graph);
        for _ in 0..2 {
            press(&mut app, KeyCode::KeyM);
            settle(&mut app);
            let b = presets::region_view(
                &places.metacontexts[1],
                &places.points,
                &[],
                &shown.visible,
                GRAPH_YAW,
                GRAPH_PITCH,
                Lens::Orthographic,
                ASPECT,
            );
            assert_at(&mut app, &b);
        }
        // Topic 0's claim 2 is visible and the topic itself is not: the fit
        // is the claim alone.
        fit_topic_of(&mut app, 2);
        settle(&mut app);
        let claim = presets::fit(
            &left[..1],
            GRAPH_YAW,
            GRAPH_PITCH,
            Lens::Orthographic,
            ASPECT,
        );
        assert_at(&mut app, &claim);
        press(&mut app, KeyCode::KeyF);
        settle(&mut app);
        assert_at(&mut app, &graph);
    }

    #[test]
    fn the_free_controls_work_after_a_tween() {
        let mut app = app(Preset::Free);
        press(&mut app, KeyCode::Digit1);
        settle(&mut app);
        let (before, projection, _, _) = camera(&mut app);
        let height = orthographic_height(&projection).unwrap();

        // W zooms an orthographic view, and A moves it sideways.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        settle(&mut app);
        let (_, projection, _, _) = camera(&mut app);
        assert!(orthographic_height(&projection).unwrap() < height);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::KeyW);
        keys.press(KeyCode::KeyA);
        app.update();
        let (after, _, _, _) = camera(&mut app);
        assert!(after.translation.x < before.translation.x);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::KeyA);
        keys.clear();

        // A right drag turns the view, from straight down.
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(0.0, -100.0);
        app.update();
        let (turned, _, fly, _) = camera(&mut app);
        assert!(fly.pitch > GRAPH_PITCH);
        assert!(turned.forward().y > -1.0 + 1e-3);
    }

    #[test]
    fn the_bar_keeps_the_wheel_over_it_and_the_keys_while_typing() {
        let mut app = app(Preset::Free);
        let speed = |app: &mut App| camera(app).2.speed;
        let before = speed(&mut app);
        app.insert_resource(UiFocus {
            pointer_over_ui: true,
            typing: false,
        });
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .delta = Vec2::new(0.0, 3.0);
        app.update();
        assert_eq!(speed(&mut app), before, "the wheel scrolled the bar");
        app.insert_resource(UiFocus::default());
        app.update();
        assert!(
            speed(&mut app) > before,
            "over the scene it changes the speed"
        );
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .delta = Vec2::ZERO;

        app.insert_resource(UiFocus {
            pointer_over_ui: true,
            typing: true,
        });
        let (at, _, _, _) = camera(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        press(&mut app, KeyCode::Digit1);
        let (still, _, _, tweening) = camera(&mut app);
        assert_eq!(still.translation, at.translation, "W typed, not moved");
        assert!(!tweening, "1 typed, not a view");

        // A right drag looks around even with the pointer over the bar.
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(40.0, 0.0);
        app.update();
        let (_, _, fly, _) = camera(&mut app);
        assert!((fly.yaw - FREE_YAW).abs() > 1e-3);
    }

    #[test]
    fn the_start_is_fitted_below_the_bar_once_and_every_view_after_it() {
        let screen = Screen {
            aspect: ASPECT,
            inset: 0.1,
            right: 0.0,
        };
        let below = |preset: Preset| {
            let places = places();
            presets::preset_view(preset, &places.points, &[], &[], places.now, screen)
        };
        let mut app = app(Preset::Graph);
        assert_at(&mut app, &preset(Preset::Graph));
        app.insert_resource(BarCover(Some(0.1)));
        app.update();
        assert_at(&mut app, &below(Preset::Graph));
        press(&mut app, KeyCode::Digit2);
        settle(&mut app);
        assert_at(&mut app, &below(Preset::Timeline));
        // The bar collapsing changes the next view, not the one shown.
        app.insert_resource(BarCover(Some(0.03)));
        app.update();
        assert_at(&mut app, &below(Preset::Timeline));
    }

    #[test]
    fn every_view_is_fitted_left_of_the_open_panel() {
        let screen = Screen {
            aspect: ASPECT,
            inset: 0.1,
            right: 0.3,
        };
        let beside = |preset: Preset| {
            let places = places();
            presets::preset_view(preset, &places.points, &[], &[], places.now, screen)
        };
        let mut app = app(Preset::Timeline);
        app.insert_resource(PanelCover(0.3));
        app.insert_resource(BarCover(Some(0.1)));
        app.update();
        assert_at(&mut app, &beside(Preset::Timeline));
        press(&mut app, KeyCode::Digit3);
        settle(&mut app);
        assert_at(&mut app, &beside(Preset::Free));
    }

    #[test]
    fn a_view_chosen_before_the_bar_is_laid_out_is_left_alone() {
        let mut app = app(Preset::Graph);
        press(&mut app, KeyCode::Digit2);
        app.insert_resource(BarCover(Some(0.1)));
        settle(&mut app);
        assert_at(&mut app, &preset(Preset::Timeline));
    }

    #[test]
    fn a_new_graph_gets_the_3d_preset_and_a_speed_for_its_size() {
        let mut app = app(Preset::Graph);
        press(&mut app, KeyCode::KeyM);
        settle(&mut app);
        let points: Vec<Vec3> = places().points.iter().map(|&p| p * 3.0).collect();
        app.world_mut().resource_mut::<Places>().points = points.clone();
        refit_new_graph(app.world_mut());
        app.update();
        settle(&mut app);
        let view = presets::preset_view(
            Preset::Free,
            &points,
            &[],
            &[],
            NowLayer::Latest(30.0),
            ASPECT,
        );
        assert_at(&mut app, &view);
        let (_, _, fly, _) = camera(&mut app);
        assert_eq!(fly.start_speed, start_speed(&points));
        assert_eq!(fly.speed, fly.start_speed);
        assert_eq!(app.world().resource::<MetacontextCursor>().0, None);
    }

    #[test]
    fn the_wheel_scales_speed_within_its_range() {
        assert!((speed_after_scroll(10.0, 10.0, 1.0) - 12.0).abs() < 1e-4);
        assert!((speed_after_scroll(12.0, 10.0, -1.0) - 10.0).abs() < 1e-4);
        assert_eq!(speed_after_scroll(10.0, 10.0, 1000.0), 10.0 * SPEED_RANGE);
        assert_eq!(speed_after_scroll(10.0, 10.0, -1000.0), 10.0 / SPEED_RANGE);
    }

    #[test]
    fn a_views_projection_reads_back_as_the_same_view() {
        let view = View {
            focus: Vec3::ZERO,
            yaw: GRAPH_YAW,
            pitch: GRAPH_PITCH,
            height: 420.0,
            lens: Lens::Orthographic,
        };
        let ortho = projection(&view);
        assert_eq!(lens(&ortho), Lens::Orthographic);
        assert_eq!(orthographic_height(&ortho), Some(420.0));
        let perspective = projection(&View {
            lens: Lens::Perspective,
            ..view
        });
        assert_eq!(lens(&perspective), Lens::Perspective);
        assert_eq!(orthographic_height(&perspective), None);
        let Projection::Perspective(p) = perspective else {
            unreachable!()
        };
        assert_eq!(p.fov, FOV);
    }
}
