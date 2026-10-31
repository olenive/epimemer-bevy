//! Switching the time mode (DESIGN.md §4, §10): `C`, or the bar's Recorded
//! and Claim buttons, set [`TimeMode`], and the scene follows it.
//!
//! A switch puts the new mode's axis, heights and spans into the graph
//! index and the places ([`apply_mode`]), which the filter, the edges, the
//! labels and the camera's fits read; turns the time window into the same
//! stretch of calendar time on the new axis, within the claim axis where
//! that is the new one; gives every node the mesh of
//! its new profile; and moves every disc and column to its new place and
//! height over [`TWEEN_SECONDS`], the places moving with them so that the
//! lines and the labels follow. The camera stays where it is. A node with
//! no position in the new mode is hidden from the start of the move, and
//! one that gains a position shows from the start of it.
//!
//! The graph index records the mode its heights are in, so a switch is
//! whatever makes [`TimeMode`] differ from it, from any source.

use bevy::prelude::*;

use crate::camera::Places;
use crate::filter::{Filter, TimeWindow, clamp_window, window_between, window_instants};
use crate::graph_index::GraphIndex;
use crate::keys::MODE_KEY;
use crate::layout::TimeAxis;
use crate::presets::{self, NowLayer, TWEEN_SECONDS, ease_in_out};
use crate::scene::{self, Disc, DiscAssets};
use crate::time_axis::{self, TimeMode, TimeModel, centre, other_mode};
use crate::ui_focus::UiFocus;

/// A switch of time mode under way: where each node's place and span, and
/// each disc's middle and height, move from and to.
#[derive(Debug, Clone, PartialEq)]
pub struct Switch {
    elapsed: f32,
    /// Each node's height and span on the axis, before and after, in node
    /// order.
    from: Vec<Placing>,
    to: Vec<Placing>,
    /// Each disc's middle and drawn height, before and after.
    discs: Vec<(Entity, [f32; 2], [f32; 2])>,
}

/// A node's height on the axis and the stretch it spans there.
pub type Placing = (f32, Option<(f32, f32)>);

/// The switch under way, if any.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct ModeTween(pub Option<Switch>);

/// Follows [`TimeMode`]. The app inserts the mode it starts in, already
/// applied to the index and the places, and the [`TimeModel`].
pub struct TimeModePlugin;

impl Plugin for TimeModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimeMode>()
            .init_resource::<TimeModel>()
            .init_resource::<ModeTween>()
            .init_resource::<UiFocus>()
            .add_systems(
                Update,
                (switch_on_key, start_switch, advance_switch).chain(),
            );
    }
}

/// Puts a mode's axis, each node's height and span on it, and its "now"
/// layer into the index and the places. A node with no position in the
/// mode keeps its recorded height. A model of another graph changes
/// nothing but the mode the index records.
pub fn apply_mode(mode: TimeMode, model: &TimeModel, index: &mut GraphIndex, places: &mut Places) {
    index.mode = mode;
    let times = time_axis::times(model, mode);
    if times.spans.len() != index.ids.len() || places.points.len() != index.ids.len() {
        return;
    }
    index.heights = times
        .spans
        .iter()
        .zip(&model.recorded.spans)
        .zip(&index.heights)
        .map(|((span, recorded), &old)| span.or(*recorded).map_or(old, |s| centre(&s)))
        .collect();
    index.spans = times
        .spans
        .iter()
        .map(|span| span.map(|s| (s.lo, s.hi)))
        .collect();
    index.time = times.axis;
    for (point, &height) in places.points.iter_mut().zip(&index.heights) {
        point.y = height;
    }
    places.spans = index.spans.clone();
    let day = presets::day_height(&times.axis);
    places.now = match mode {
        TimeMode::Recorded => NowLayer::Latest(day),
        TimeMode::Claim => NowLayer::At {
            height: time_axis::now_height(model, mode),
            day,
        },
    };
}

/// The same stretch of calendar time as `window` on another axis.
pub fn convert_window(window: &TimeWindow, from: &TimeAxis, to: &TimeAxis) -> TimeWindow {
    let (start, end) = window_instants(from, window);
    window_between(to, start, end)
}

/// The places' heights and spans, in node order.
fn placings(places: &Places) -> Vec<Placing> {
    places
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.y, places.spans.get(i).copied().flatten()))
        .collect()
}

fn write_placings(places: &mut Places, placings: &[Placing]) {
    for (point, &(y, _)) in places.points.iter_mut().zip(placings) {
        point.y = y;
    }
    places.spans = placings.iter().map(|&(_, span)| span).collect();
}

/// `s` of the way from `a` to `b`, exactly `b` at the end.
fn lerp(a: f32, b: f32, s: f32) -> f32 {
    a * (1.0 - s) + b * s
}

/// A node's placing `s` of the way from one to another. A span that
/// appears or goes does so at once.
pub fn blend(from: Placing, to: Placing, s: f32) -> Placing {
    let span = match (from.1, to.1) {
        (Some(a), Some(b)) => Some((lerp(a.0, b.0, s), lerp(a.1, b.1, s))),
        (_, span) => span,
    };
    (lerp(from.0, to.0, s), span)
}

/// `C` switches to the other mode, unless a text field has the keyboard.
fn switch_on_key(keys: Res<ButtonInput<KeyCode>>, ui: Res<UiFocus>, mut mode: ResMut<TimeMode>) {
    if !ui.typing && keys.just_pressed(MODE_KEY.0) {
        *mode = other_mode(*mode);
    }
}

/// What a switch changes.
#[derive(bevy::ecs::system::SystemParam)]
struct Scene<'w, 's> {
    index: ResMut<'w, GraphIndex>,
    places: ResMut<'w, Places>,
    filter: ResMut<'w, Filter>,
    assets: Option<Res<'w, DiscAssets>>,
    discs: Query<
        'w,
        's,
        (
            Entity,
            &'static Disc,
            &'static Transform,
            &'static mut Mesh3d,
        ),
    >,
    tween: ResMut<'w, ModeTween>,
}

/// Starts a switch when the mode asked for is not the one drawn.
fn start_switch(mode: Res<TimeMode>, model: Res<TimeModel>, mut scene: Scene) {
    if scene.index.mode == *mode {
        return;
    }
    let old_axis = scene.index.time;
    let from = placings(&scene.places);
    apply_mode(*mode, &model, &mut scene.index, &mut scene.places);
    let to = placings(&scene.places);
    if let Some(window) = scene.filter.window {
        let converted = convert_window(&window, &old_axis, &scene.index.time);
        scene.filter.window = Some(match *mode {
            TimeMode::Recorded => converted,
            TimeMode::Claim => clamp_window(&converted, &scene.index.time),
        });
    }
    let spans = &time_axis::times(&model, *mode).spans;
    let mut discs = Vec::new();
    for (entity, disc, transform, mut mesh) in &mut scene.discs {
        let fallback = model
            .recorded
            .spans
            .get(disc.index)
            .copied()
            .flatten()
            .map_or(transform.translation.y, |s| s.lo);
        let (y, height, profile) = scene::shape(spans.get(disc.index).copied().flatten(), fallback);
        if let Some(assets) = scene.assets.as_deref() {
            let wanted = scene::profile_mesh(assets, profile);
            if mesh.0 != wanted {
                mesh.0 = wanted;
            }
        }
        discs.push((
            entity,
            [transform.translation.y, transform.scale.y],
            [y, height],
        ));
    }
    write_placings(&mut scene.places, &from);
    scene.tween.0 = Some(Switch {
        elapsed: 0.0,
        from,
        to,
        discs,
    });
    log_mode(*mode, &model, &scene.index.time);
}

/// Logs the mode switched to and what it draws.
pub fn log_mode(mode: TimeMode, model: &TimeModel, axis: &TimeAxis) {
    let stamp = |micros: i64| {
        chrono::DateTime::from_timestamp_micros(micros)
            .map_or_else(|| micros.to_string(), |t| t.format("%Y-%m-%d").to_string())
    };
    let c = &model.counts;
    match mode {
        TimeMode::Recorded => info!(
            "time mode: recorded time, the axis from {} to {}, {:.0} high",
            stamp(axis.start),
            stamp(axis.start + axis.span),
            axis.height
        ),
        TimeMode::Claim => info!(
            "time mode: claim time, the axis from {} to {}, {:.0} high: {} solid columns, {} smears, {} instants, {} discs at their recorded time, {} claims with no place; {} nodes beyond the axis, drawn clipped, with {} instants before it and {} after it",
            stamp(axis.start),
            stamp(axis.start + axis.span),
            axis.height,
            c.columns,
            c.smears,
            c.instants,
            c.discs,
            c.unplaced,
            c.clipped_nodes,
            c.clipped_below,
            c.clipped_above
        ),
    }
}

/// Ends a switch under way at once, every disc and the places where the
/// switch would leave them, so that the picture can be changed under it
/// (`live.rs`).
pub fn finish_switch(world: &mut World) {
    let Some(switch) = world
        .get_resource_mut::<ModeTween>()
        .and_then(|mut tween| tween.0.take())
    else {
        return;
    };
    for (entity, _, to) in &switch.discs {
        if let Some(mut transform) = world.get_mut::<Transform>(*entity) {
            transform.translation.y = to[0];
            transform.scale.y = to[1];
        }
    }
    if let Some(mut places) = world.get_resource_mut::<Places>() {
        write_placings(&mut places, &switch.to);
    }
}

/// Moves every disc, and the places, a step along the switch under way.
fn advance_switch(
    time: Res<Time>,
    mut tween: ResMut<ModeTween>,
    mut places: ResMut<Places>,
    mut transforms: Query<&mut Transform, With<Disc>>,
) {
    let Some(switch) = tween.0.as_mut() else {
        return;
    };
    switch.elapsed += time.delta_secs();
    let t = (switch.elapsed / TWEEN_SECONDS).min(1.0);
    let s = ease_in_out(t);
    for (entity, from, to) in &switch.discs {
        if let Ok(mut transform) = transforms.get_mut(*entity) {
            transform.translation.y = lerp(from[0], to[0], s);
            transform.scale.y = lerp(from[1], to[1], s);
        }
    }
    let between: Vec<Placing> = switch
        .from
        .iter()
        .zip(&switch.to)
        .map(|(&a, &b)| blend(a, b, s))
        .collect();
    write_placings(&mut places, &between);
    if t >= 1.0 {
        tween.0 = None;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::filter::{Kinds, Shown};
    use crate::graph_index::tests::{edge, node};
    use crate::hub::NodeType;
    use crate::hub::{Snapshot, TimelineView, TimepointView};
    use crate::reload::prepare;
    use crate::scene::{DISC_HEIGHT, DiscMaterial};
    use crate::time_axis::Profile;

    /// A topic, a claim with an interval from 2016 to 2020, and a claim with
    /// no timelink, recorded on 1, 2 and 3 September 2026.
    fn snapshot() -> Snapshot {
        let mut timelink = edge("dated", "line", "timelink");
        timelink.metadata.insert("timepoint_id".into(), "p".into());
        Snapshot {
            schema_version: Some(1),
            graph: "test".into(),
            nodes: vec![
                node("topic", NodeType::Topic, 1),
                node("dated", NodeType::Fact, 2),
                node("plain", NodeType::Fact, 3),
            ],
            edges: vec![timelink],
            timelines: vec![TimelineView {
                timeline_id: "line".into(),
                name: "The world".into(),
                description: String::new(),
                timepoints: vec![TimepointView {
                    timepoint_id: "p".into(),
                    start: Some("2016-01-01T00:00:00Z".into()),
                    end: Some("2020-01-01T00:00:00Z".into()),
                    label: None,
                    kind: "interval".into(),
                    earliest: None,
                    latest: None,
                    contested: false,
                    temporal_contradiction_id: None,
                    metadata: Default::default(),
                }],
                reference_time: None,
                created_at: "2026-09-01T00:00:00Z".into(),
                graph: "test".into(),
            }],
            metacontexts: Vec::new(),
        }
    }

    fn app() -> App {
        let prepared = prepare(&snapshot()).unwrap();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_asset::<Mesh>()
            .init_asset::<DiscMaterial>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(Shown::default())
            .insert_resource(prepared.index)
            .insert_resource(prepared.places)
            .insert_resource(prepared.time)
            .insert_resource(Filter::default())
            .add_plugins(TimeModePlugin);
        scene::spawn_discs(app.world_mut(), prepared.nodes, &[], &Shown::default());
        app.update();
        app
    }

    fn disc(app: &mut App, index: usize) -> (Transform, Handle<Mesh>) {
        let mut query = app.world_mut().query::<(&Disc, &Transform, &Mesh3d)>();
        query
            .iter(app.world())
            .find(|(d, _, _)| d.index == index)
            .map(|(_, t, m)| (*t, m.0.clone()))
            .unwrap()
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

    #[test]
    fn a_switch_moves_every_node_to_its_place_in_the_other_mode() {
        let mut app = app();
        let model = app.world().resource::<TimeModel>().clone();
        let dated = app.world().resource::<GraphIndex>().by_id["dated"];
        let recorded_y = disc(&mut app, dated).0.translation.y;
        assert_eq!(disc(&mut app, dated).0.scale.y, DISC_HEIGHT);

        press(&mut app, MODE_KEY.0);
        assert_eq!(*app.world().resource::<TimeMode>(), TimeMode::Claim);
        let index = app.world().resource::<GraphIndex>();
        assert_eq!(index.mode, TimeMode::Claim);
        assert_eq!(index.time, model.claim.axis);
        let span = model.claim.spans[dated].unwrap();
        assert_eq!(index.spans[dated], Some((span.lo, span.hi)));
        assert!(app.world().resource::<ModeTween>().0.is_some(), "under way");
        let halfway = disc(&mut app, dated).0.translation.y;
        assert!(
            halfway != recorded_y && halfway != centre(&span),
            "{halfway}"
        );

        for _ in 0..10 {
            app.update();
        }
        assert!(app.world().resource::<ModeTween>().0.is_none(), "arrived");
        let (transform, mesh) = disc(&mut app, dated);
        assert_eq!(transform.translation.y, centre(&span));
        assert_eq!(
            transform.scale.y,
            span.hi - span.lo,
            "a column over its interval"
        );
        let assets = app.world().resource::<DiscAssets>().clone();
        assert_eq!(mesh, scene::profile_mesh(&assets, Profile::Solid));
        let places = app.world().resource::<Places>();
        assert_eq!(places.points[dated].y, centre(&span));
        assert_eq!(places.spans[dated], Some((span.lo, span.hi)));
        assert!(matches!(places.now, NowLayer::At { .. }));

        press(&mut app, MODE_KEY.0);
        for _ in 0..10 {
            app.update();
        }
        let (transform, _) = disc(&mut app, dated);
        assert_eq!(transform.translation.y, recorded_y, "and back");
        assert_eq!(transform.scale.y, DISC_HEIGHT);
        assert!(matches!(
            app.world().resource::<Places>().now,
            NowLayer::Latest(_)
        ));
    }

    #[test]
    fn the_window_keeps_its_stretch_of_calendar_time_across_a_switch() {
        let mut app = app();
        let recorded = app.world().resource::<GraphIndex>().time;
        let day = |d: i64| recorded.start + (d - 1) * presets::DAY_MICROS;
        let window = window_between(&recorded, day(2), day(3));
        app.world_mut().resource_mut::<Filter>().window = Some(window);
        app.world_mut().resource_mut::<Filter>().kinds = Kinds::default();
        *app.world_mut().resource_mut::<TimeMode>() = TimeMode::Claim;
        app.update();
        let claim = app.world().resource::<GraphIndex>().time;
        let moved = app.world().resource::<Filter>().window.unwrap();
        let (from, to) = window_instants(&claim, &moved);
        // The claim axis spans ten years, so a height is good to minutes.
        let tolerance = 5 * 60 * 1_000_000;
        assert!((from - day(2)).abs() < tolerance && (to - day(3)).abs() < tolerance);
        assert_eq!(convert_window(&window, &recorded, &recorded), window);
    }

    #[test]
    fn in_claim_time_the_window_lies_within_the_claim_axis() {
        let mut app = app();
        let recorded = app.world().resource::<GraphIndex>().time;
        let micros = |text: &str| crate::hub::parse_timestamp(text).unwrap();
        // From 2010, before the claim axis's start in 2016, to 2030, after
        // its end at the start of 2027.
        let window = window_between(
            &recorded,
            micros("2010-01-01T00:00:00Z"),
            micros("2030-01-01T00:00:00Z"),
        );
        app.world_mut().resource_mut::<Filter>().window = Some(window);
        *app.world_mut().resource_mut::<TimeMode>() = TimeMode::Claim;
        app.update();
        let claim = app.world().resource::<GraphIndex>().time;
        let moved = app.world().resource::<Filter>().window.unwrap();
        assert_eq!((moved.from, moved.to), (0.0, claim.height));
    }

    #[test]
    fn a_blend_moves_heights_and_spans_and_snaps_a_span_that_comes_or_goes() {
        let from = (10.0, Some((0.0, 20.0)));
        let to = (30.0, Some((30.0, 30.0)));
        assert_eq!(blend(from, to, 0.5), (20.0, Some((15.0, 25.0))));
        assert_eq!(blend(from, to, 1.0), to);
        assert_eq!(blend((10.0, None), to, 0.0), (10.0, to.1));
        assert_eq!(blend(from, (30.0, None), 0.2).1, None);
    }
}
