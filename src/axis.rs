//! The drawn time axis (DESIGN.md §4): a vertical line at the map's edge,
//! ticks at a granularity the visible span calls for, and "now".
//!
//! The axis stands just outside the corner of the map where both ground
//! coordinates are least, which is the left of the screen in the timeline
//! and the 3D presets, and runs the height of the time axis of the mode
//! drawn, reaching to "now" where now lies beyond it. In every update the
//! part of it the camera shows below the bar is worked out exactly from the
//! camera's projection ([`visible_heights`]); the ticks are the calendar's
//! centuries, decades, years, months, weeks or days, the finest that leave
//! [`MIN_TICK_SPACING`] logical pixels between neighbours on the screen
//! ([`choose_granularity`]), so zooming in brings finer ticks. Each tick is a
//! short line into the map and a date in UI text to the left of the axis,
//! placed with `Camera::world_to_viewport` as the labels are.
//!
//! "Now", the timeline's reference time or the wall clock (`time_axis.rs`),
//! has a tick of its own colour, a label, and a faint ring around the map
//! at its height, so that the present reads as a plane through the scene.
//! The axis is drawn in both time modes, on the axis of the one drawn.
//!
//! The claim axis spans the bulk of claim time and clips the rest to its
//! ends (`clip.rs`). At an end that clips a visible node the axis shows a
//! break mark, a short stub of its line past the end crossed by two
//! slashes ([`break_mark`]), with the count of the instants beyond it as a
//! label, "4 earlier" at the foot and "2 later" at the top; and each
//! clipped node has a small arrow in the axis's colour at its clipped end,
//! pointing out of the axis (`clip::clip_mark`). A "now" beyond the claim
//! axis stands at its end and its label says so.

use bevy::camera::CameraUpdateSystems;
use bevy::math::{Mat4, Vec4};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use chrono::{Datelike, Months, NaiveDate};

use crate::camera::{BarCover, Places};
use crate::clip::{clip_mark, clipped_counts, count_text, is_clipped};
use crate::filter::{Shown, filter_alpha, is_visible};
use crate::graph_index::GraphIndex;
use crate::labels::text_width;
use crate::layout::{TimeAxis, instant_at, time_y};
use crate::scene::{AXIS_COLOUR, NOW_COLOUR, NOW_RING_COLOUR, TICK_TEXT_COLOUR, shape};
use crate::time_axis::{Span, TimeMode, TimeModel, now_clipped, now_height};

/// How far outside the map's corner the axis stands, in world units.
pub const AXIS_GAP: f32 = 6.0;
/// How far a tick reaches from the axis into the map, and "now"'s tick.
pub const TICK_LENGTH: f32 = 2.5;
pub const NOW_TICK_LENGTH: f32 = 7.0;
/// The least room between two ticks on the screen, in logical pixels.
pub const MIN_TICK_SPACING: f32 = 56.0;
/// The most ticks drawn, whatever the view.
pub const MAX_TICKS: usize = 40;
/// The size of a tick's date, in logical pixels, and the room between it
/// and the axis.
pub const TICK_FONT: f32 = 11.0;
pub const TICK_GAP: f32 = 6.0;
/// How far the ring at "now" stands outside the farthest node.
pub const RING_PAD: f32 = 4.0;
/// The break mark at an end of the claim axis that clips something, in
/// logical pixels on the screen, so that it reads at any zoom: how far its
/// stub runs past the end, how far each of its two slashes reaches across
/// the line and up it from its middle, and how far apart they are. The
/// count beside it stands at the stub's far end, clear of a tick's date at
/// the end itself.
pub const BREAK_LENGTH: f32 = 16.0;
pub const BREAK_ACROSS: f32 = 5.0;
pub const BREAK_RISE: f32 = 2.5;
pub const BREAK_STEP: f32 = 4.0;
/// The first label slot a tick's date takes: "now"'s and the two counts
/// beside the break marks come first.
const FIRST_TICK_SLOT: usize = 3;
/// The nearest a point may be in front of a perspective camera and still
/// be drawn, in world units; Bevy's default near plane.
const NEAR: f32 = 0.1;

/// The steps the axis's ticks come in, coarsest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Century,
    Decade,
    Year,
    Month,
    Week,
    Day,
}

pub const GRANULARITIES: [Granularity; 6] = [
    Granularity::Century,
    Granularity::Decade,
    Granularity::Year,
    Granularity::Month,
    Granularity::Week,
    Granularity::Day,
];

const DAY: i64 = 86_400_000_000;

/// About how long a step is, in microseconds.
pub fn nominal_micros(granularity: Granularity) -> i64 {
    match granularity {
        Granularity::Century => 36_525 * DAY,
        Granularity::Decade => 3_652 * DAY + DAY / 2,
        Granularity::Year => 365 * DAY + DAY / 4,
        Granularity::Month => 30 * DAY + DAY * 44 / 100,
        Granularity::Week => 7 * DAY,
        Granularity::Day => DAY,
    }
}

/// The finest step that leaves [`MIN_TICK_SPACING`] pixels between ticks
/// when `span` microseconds are shown over `pixels` logical pixels; `None`
/// when nothing is shown or even centuries would crowd.
pub fn choose_granularity(span: i64, pixels: f32) -> Option<Granularity> {
    if span <= 0 || pixels.is_nan() || pixels <= 0.0 {
        return None;
    }
    let per_micro = pixels as f64 / span as f64;
    GRANULARITIES
        .iter()
        .rev()
        .copied()
        .find(|&g| nominal_micros(g) as f64 * per_micro >= MIN_TICK_SPACING as f64)
}

fn midnight(date: NaiveDate) -> Option<i64> {
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_micros())
}

/// The first step at or before a date.
fn floor(granularity: Granularity, date: NaiveDate) -> Option<NaiveDate> {
    let years = |n: i32| NaiveDate::from_ymd_opt(date.year().div_euclid(n) * n, 1, 1);
    match granularity {
        Granularity::Century => years(100),
        Granularity::Decade => years(10),
        Granularity::Year => years(1),
        Granularity::Month => NaiveDate::from_ymd_opt(date.year(), date.month(), 1),
        Granularity::Week => date.checked_sub_days(chrono::Days::new(u64::from(
            date.weekday().num_days_from_monday(),
        ))),
        Granularity::Day => Some(date),
    }
}

/// The step after a date.
fn next(granularity: Granularity, date: NaiveDate) -> Option<NaiveDate> {
    let years = |n: i32| NaiveDate::from_ymd_opt(date.year() + n, 1, 1);
    match granularity {
        Granularity::Century => years(100),
        Granularity::Decade => years(10),
        Granularity::Year => years(1),
        Granularity::Month => date.checked_add_months(Months::new(1)),
        Granularity::Week => date.checked_add_days(chrono::Days::new(7)),
        Granularity::Day => date.checked_add_days(chrono::Days::new(1)),
    }
}

/// The step at or before an instant: midnight UTC on the first day of its
/// century, decade, year or month, on its Monday, or on its day. `None`
/// outside the calendar's range.
pub fn round_down(granularity: Granularity, at: i64) -> Option<i64> {
    let date = chrono::DateTime::from_timestamp_micros(at)?.date_naive();
    midnight(floor(granularity, date)?)
}

/// The step at or after an instant.
pub fn round_up(granularity: Granularity, at: i64) -> Option<i64> {
    let down = round_down(granularity, at)?;
    if down == at {
        return Some(at);
    }
    let date = chrono::DateTime::from_timestamp_micros(down)?.date_naive();
    midnight(next(granularity, date)?)
}

/// The ticks of a step from one instant to another, ends included, at
/// midnight UTC on the first day of each century, decade, year or month,
/// on each Monday, or on each day; at most [`MAX_TICKS`].
pub fn ticks(granularity: Granularity, from: i64, to: i64) -> Vec<i64> {
    let mut found = Vec::new();
    let Some(start) = chrono::DateTime::from_timestamp_micros(from) else {
        return found;
    };
    let mut date = floor(granularity, start.date_naive());
    while let Some(day) = date {
        let Some(at) = midnight(day) else {
            break;
        };
        if at > to || found.len() >= MAX_TICKS {
            break;
        }
        if at >= from {
            found.push(at);
        }
        date = next(granularity, day);
    }
    found
}

/// A tick's date, as fine as its step.
pub fn tick_label(granularity: Granularity, at: i64) -> String {
    let Some(time) = chrono::DateTime::from_timestamp_micros(at) else {
        return at.to_string();
    };
    match granularity {
        Granularity::Century | Granularity::Decade | Granularity::Year => {
            time.format("%Y").to_string()
        }
        Granularity::Month => time.format("%b %Y").to_string(),
        Granularity::Week | Granularity::Day => time.format("%-d %b %Y").to_string(),
    }
}

/// The heights of the vertical line through `x` and `z`, between `lo` and
/// `hi`, that a camera with this projection and view shows: inside the
/// viewport across, between its foot and `top` in normalised device
/// height (1 at the top of the viewport; less where the bar covers it),
/// and in front of the camera. `None` where it shows none of it.
///
/// A point's clip coordinates are linear in its height, so each edge of
/// the view is a bound on the height, found exactly.
pub fn visible_heights(
    clip_from_world: Mat4,
    x: f32,
    z: f32,
    lo: f32,
    hi: f32,
    top: f32,
) -> Option<(f32, f32)> {
    let base = clip_from_world * Vec4::new(x, 0.0, z, 1.0);
    let rise = clip_from_world * Vec4::Y;
    // Each bound is a + b * height >= 0.
    let bounds = [
        (base.w - NEAR, rise.w),
        (base.w - base.x, rise.w - rise.x),
        (base.w + base.x, rise.w + rise.x),
        (top * base.w - base.y, top * rise.w - rise.y),
        (base.w + base.y, rise.w + rise.y),
    ];
    let (mut low, mut high) = (lo.min(hi), lo.max(hi));
    for (a, b) in bounds {
        if b.abs() < 1e-9 {
            if a < 0.0 {
                return None;
            }
        } else if b > 0.0 {
            low = low.max(-a / b);
        } else {
            high = high.min(-a / b);
        }
    }
    (low <= high).then_some((low, high))
}

/// Where the axis stands and the ring at "now" goes, from where the nodes
/// are: the axis's foot on the map, the map's middle, and the ring's
/// radius. `None` for no nodes.
pub fn axis_place(points: &[Vec3]) -> Option<(Vec2, Vec2, f32)> {
    let first = points.first()?;
    let ground = |p: &Vec3| Vec2::new(p.x, p.z);
    let (lo, hi) = points
        .iter()
        .fold((ground(first), ground(first)), |(lo, hi), p| {
            (lo.min(ground(p)), hi.max(ground(p)))
        });
    let middle = (lo + hi) / 2.0;
    let radius = points
        .iter()
        .map(|p| ground(p).distance(middle))
        .fold(0.0, f32::max)
        + RING_PAD;
    Some((lo - Vec2::splat(AXIS_GAP), middle, radius))
}

/// The lines of the break mark at an end of the axis standing at `foot` on
/// the map: a stub of the axis's line from `end` outward, `outward` 1 at
/// the top and -1 at the foot, crossed by two parallel slashes, at `pixel`
/// world units to the logical pixel.
pub fn break_mark(foot: Vec2, end: f32, outward: f32, pixel: f32) -> Vec<[Vec3; 2]> {
    let at = |y: f32| Vec3::new(foot.x, y, foot.y);
    let middle = end + outward * BREAK_LENGTH * pixel / 2.0;
    let reach = Vec3::new(BREAK_ACROSS, BREAK_RISE, 0.0) * pixel;
    let slash = |y: f32| [at(y) - reach, at(y) + reach];
    vec![
        [at(end), at(end + outward * BREAK_LENGTH * pixel)],
        slash(middle - BREAK_STEP * pixel / 2.0),
        slash(middle + BREAK_STEP * pixel / 2.0),
    ]
}

/// The world length of one logical pixel at a point, from how far one unit
/// up and one unit across move it on the screen, whichever is more, so
/// that the axis seen end on still has a measure; `None` where neither
/// moves it.
fn pixel_at(camera: &Camera, global: &GlobalTransform, point: Vec3) -> Option<f32> {
    let from = camera.world_to_viewport(global, point).ok()?;
    let moved = |offset: Vec3| {
        camera
            .world_to_viewport(global, point + offset)
            .map_or(0.0, |p| p.distance(from))
    };
    let pixels = moved(Vec3::Y).max(moved(Vec3::X));
    (pixels > 1e-6).then(|| 1.0 / pixels)
}

/// "Now"'s label: the word, and where it lies beyond the claim axis.
pub fn now_text(before: bool, after: bool) -> &'static str {
    match (before, after) {
        (true, _) => "now, before the axis",
        (_, true) => "now, after the axis",
        _ => "now",
    }
}

/// The heights the axis line runs between: the time axis, the spans that
/// reach beyond it, and "now".
pub fn axis_range(axis: &TimeAxis, spans: &[Option<(f32, f32)>], now: f32) -> (f32, f32) {
    let (lo, hi) = spans.iter().flatten().fold(
        (0.0_f32.min(now), axis.height.max(now)),
        |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)),
    );
    (lo, hi)
}

/// The axis's lines, drawn with gizmos of their own width.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct AxisLines;

/// The layer the axis's dates are drawn in.
#[derive(Component, Debug)]
pub struct AxisLayer;

/// A date beside the axis: "now"'s is slot 0, the counts beside the break
/// marks at the foot and the top slots 1 and 2, the ticks' the rest.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisLabel(pub usize);

pub struct AxisPlugin;

impl Plugin for AxisPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BarCover>()
            .init_resource::<Shown>()
            .insert_gizmo_config(
                AxisLines,
                GizmoConfig {
                    line: GizmoLineConfig {
                        width: 1.5,
                        ..default()
                    },
                    ..default()
                },
            )
            .add_systems(Startup, spawn_labels)
            .add_systems(
                PostUpdate,
                draw_axis
                    .after(CameraUpdateSystems)
                    .before(UiSystems::Prepare),
            );
    }
}

fn spawn_labels(mut commands: Commands) {
    let layer = commands
        .spawn((
            AxisLayer,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                ..default()
            },
            // Above the nodes' labels, so that a topic's name never hides
            // a date.
            GlobalZIndex(2),
            Pickable::IGNORE,
        ))
        .id();
    for slot in 0..FIRST_TICK_SLOT + MAX_TICKS {
        commands.spawn((
            AxisLabel(slot),
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                ..default()
            },
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(TICK_FONT),
                ..default()
            },
            TextColor(TICK_TEXT_COLOUR),
            TextLayout::no_wrap(),
            Pickable::IGNORE,
            ChildOf(layer),
        ));
    }
}

/// A date to show beside the axis: its words, its colour and where its
/// tick is on the screen.
struct Beside {
    words: String,
    colour: Color,
    at: Vec2,
}

/// What the axis is drawn from.
#[derive(bevy::ecs::system::SystemParam)]
struct Source<'w> {
    places: Res<'w, Places>,
    index: Res<'w, GraphIndex>,
    model: Option<Res<'w, TimeModel>>,
    cover: Res<'w, BarCover>,
    shown: Res<'w, Shown>,
}

/// Draws the axis, its ticks and "now", and places the dates beside them.
fn draw_axis(
    source: Source,
    camera: Single<(&Camera, &Transform)>,
    mut gizmos: Gizmos<AxisLines>,
    mut labels: Query<(&AxisLabel, &mut Node, &mut Text, &mut TextColor)>,
) {
    let (camera, transform) = camera.into_inner();
    let mut shown: Vec<Option<Beside>> = Vec::new();
    if let (Some((foot, middle, radius)), Some(viewport)) = (
        axis_place(&source.places.points),
        camera.logical_viewport_rect(),
    ) {
        let axis = source.index.time;
        let now = source
            .model
            .as_deref()
            .map(|model| now_height(model, source.index.mode));
        let (lo, hi) = axis_range(&axis, &source.places.spans, now.unwrap_or(0.0));
        let at = |y: f32| Vec3::new(foot.x, y, foot.y);
        gizmos.line(at(lo), at(hi), AXIS_COLOUR);
        let global = GlobalTransform::from(*transform);
        let screen = |y: f32| camera.world_to_viewport(&global, at(y)).ok();
        let clear_top = viewport.min.y + source.cover.0.unwrap_or(0.0) * viewport.height();
        let readable = |p: Vec2| p.y >= clear_top && viewport.contains(p);
        let claim = source.index.mode == TimeMode::Claim;
        let (before, after) = match source.model.as_deref() {
            Some(model) if claim => clipped_counts(&model.claim.spans, &source.shown.visible),
            _ => (0, 0),
        };
        let now_beyond = source
            .model
            .as_deref()
            .map(|model| now_clipped(model, source.index.mode))
            .unwrap_or_default();
        if let Some(now) = now {
            gizmos.line(at(now), at(now) + Vec3::X * NOW_TICK_LENGTH, NOW_COLOUR);
            gizmos
                .circle(
                    Isometry3d::new(
                        Vec3::new(middle.x, now, middle.y),
                        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                    ),
                    radius,
                    NOW_RING_COLOUR,
                )
                .resolution(128);
            shown.push(screen(now).filter(|&p| readable(p)).map(|p| Beside {
                words: now_text(now_beyond.below > 0, now_beyond.above > 0).to_owned(),
                colour: NOW_COLOUR,
                at: p,
            }));
        } else {
            shown.push(None);
        }
        for (count, end, outward) in [(before, 0.0, -1.0), (after, axis.height, 1.0)] {
            // Nothing to mark, or the axis seen end on, where the stub has
            // no length on the screen and its count would sit on "now".
            let stub = pixel_at(camera, &global, at(end))
                .filter(|_| count > 0)
                .map(|pixel| (pixel, end + outward * BREAK_LENGTH * pixel))
                .filter(|&(_, beside)| match (screen(end), screen(beside)) {
                    (Some(a), Some(b)) => a.distance(b) >= BREAK_LENGTH / 2.0,
                    _ => false,
                });
            let Some((pixel, beside)) = stub else {
                shown.push(None);
                continue;
            };
            for [a, b] in break_mark(foot, end, outward, pixel) {
                gizmos.line(a, b, AXIS_COLOUR);
            }
            shown.push(screen(beside).filter(|&p| readable(p)).map(|p| Beside {
                words: count_text(count, outward < 0.0),
                colour: TICK_TEXT_COLOUR,
                at: p,
            }));
        }
        let clip_from_world = camera.clip_from_view() * Mat4::from(global.affine().inverse());
        let top = 1.0 - 2.0 * source.cover.0.unwrap_or(0.0).clamp(0.0, 1.0);
        let visible = visible_heights(clip_from_world, foot.x, foot.y, lo, hi, top);
        if let Some((y0, y1)) = visible
            && let (Some(a), Some(b)) = (screen(y0), screen(y1))
        {
            let (from, to) = (instant_at(&axis, y0), instant_at(&axis, y1));
            if let Some(step) = choose_granularity(to - from, a.distance(b)) {
                for tick in ticks(step, from, to) {
                    let y = time_y(&axis, tick);
                    gizmos.line(at(y), at(y) + Vec3::X * TICK_LENGTH, AXIS_COLOUR);
                    shown.push(screen(y).filter(|&p| readable(p)).map(|p| Beside {
                        words: tick_label(step, tick),
                        colour: TICK_TEXT_COLOUR,
                        at: p,
                    }));
                }
            }
        }
    }
    if source.index.mode == TimeMode::Claim
        && let Some(model) = source.model.as_deref()
        && let Some((foot, _, _)) = axis_place(&source.places.points)
        && let Some(pixel) = pixel_at(
            camera,
            &GlobalTransform::from(*transform),
            Vec3::new(foot.x, source.index.time.height / 2.0, foot.y),
        )
    {
        draw_clip_marks(&source, model, pixel, &mut gizmos);
    }
    for (slot, mut node, mut text, mut colour) in &mut labels {
        let Some(Some(label)) = shown.get(slot.0) else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        if text.0 != label.words {
            text.0.clone_from(&label.words);
        }
        colour.set_if_neq(TextColor(label.colour));
        let width = text_width(&label.words, TICK_FONT);
        let (left, top) = (
            px(label.at.x - TICK_GAP - width),
            px(label.at.y - TICK_FONT * 0.65),
        );
        if node.display != Display::Flex || node.left != left || node.top != top {
            node.display = Display::Flex;
            node.left = left;
            node.top = top;
        }
    }
}

/// Draws the arrow at each clipped end of every visible node clipped to the
/// claim axis, at its drawn end as the places have it, faint for a node
/// the filter dims, `pixel` world units to the logical pixel as measured
/// at the axis.
fn draw_clip_marks(source: &Source, model: &TimeModel, pixel: f32, gizmos: &mut Gizmos<AxisLines>) {
    let shown = &source.shown;
    for (i, span) in model.claim.spans.iter().enumerate() {
        let Some(span) = span.filter(|s| is_clipped(&s.clipped)) else {
            continue;
        };
        let Some(&point) = source.places.points.get(i) else {
            continue;
        };
        if !is_visible(&shown.visible, i) {
            continue;
        }
        let (lo, hi) = source
            .places
            .spans
            .get(i)
            .copied()
            .flatten()
            .unwrap_or((span.lo, span.hi));
        let (y, height, _) = shape(Some(Span { lo, hi, ..span }), point.y);
        let passes = shown.passes.get(i).copied().unwrap_or(true);
        let colour = AXIS_COLOUR.with_alpha(AXIS_COLOUR.alpha() * filter_alpha(shown.mode, passes));
        let ends = [
            (span.clipped.below > 0, y - height / 2.0, -1.0),
            (span.clipped.above > 0, y + height / 2.0, 1.0),
        ];
        for (_, end, outward) in ends.into_iter().filter(|(clipped, ..)| *clipped) {
            for [a, b] in clip_mark(point.with_y(end), outward, pixel) {
                gizmos.line(a, b, colour);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::parse_timestamp;

    fn micros(text: &str) -> i64 {
        parse_timestamp(text).unwrap()
    }

    #[test]
    fn the_step_is_the_finest_that_leaves_room_between_ticks() {
        let year = nominal_micros(Granularity::Year);
        // Ten years over 600 pixels: 60 pixels a year.
        assert_eq!(
            choose_granularity(10 * year, 600.0),
            Some(Granularity::Year)
        );
        // Three months over 600 pixels: about 46 pixels a week.
        assert_eq!(
            choose_granularity(91 * DAY, 600.0),
            Some(Granularity::Month)
        );
        assert_eq!(choose_granularity(91 * DAY, 800.0), Some(Granularity::Week));
        assert_eq!(choose_granularity(10 * DAY, 700.0), Some(Granularity::Day));
        // Six and a half centuries over 600 pixels: about 92 a century.
        assert_eq!(
            choose_granularity(655 * year, 600.0),
            Some(Granularity::Century)
        );
        assert_eq!(
            choose_granularity(65 * year, 600.0),
            Some(Granularity::Decade)
        );
        assert_eq!(
            choose_granularity(100_000 * year, 600.0),
            None,
            "crowded at any step"
        );
        assert_eq!(choose_granularity(0, 600.0), None);
        assert_eq!(choose_granularity(year, 0.0), None, "the axis seen end on");
    }

    #[test]
    fn ticks_fall_on_the_calendar() {
        let from = micros("2026-07-23T22:20:22Z");
        let to = micros("2026-10-20T16:15:14Z");
        let months: Vec<String> = ticks(Granularity::Month, from, to)
            .into_iter()
            .map(|t| tick_label(Granularity::Month, t))
            .collect();
        assert_eq!(months, ["Aug 2026", "Sep 2026", "Oct 2026"]);
        let weeks = ticks(Granularity::Week, from, to);
        assert_eq!(
            tick_label(Granularity::Week, weeks[0]),
            "27 Jul 2026",
            "a Monday"
        );
        assert!(weeks.windows(2).all(|w| w[1] - w[0] == 7 * DAY));
        let days = ticks(
            Granularity::Day,
            micros("2026-10-01T00:00:00Z"),
            micros("2026-10-03T00:00:00Z"),
        );
        assert_eq!(days.len(), 3, "the ends are inside");
        let centuries: Vec<String> = ticks(
            Granularity::Century,
            micros("1372-01-01T00:00:00Z"),
            micros("2026-11-01T00:00:00Z"),
        )
        .into_iter()
        .map(|t| tick_label(Granularity::Century, t))
        .collect();
        assert_eq!(
            centuries,
            ["1400", "1500", "1600", "1700", "1800", "1900", "2000"]
        );
        let decades = ticks(
            Granularity::Decade,
            micros("1991-06-01T00:00:00Z"),
            micros("2026-01-01T00:00:00Z"),
        );
        assert_eq!(tick_label(Granularity::Decade, decades[0]), "2000");
        assert_eq!(
            ticks(Granularity::Day, 0, 400 * DAY).len(),
            MAX_TICKS,
            "never more than the most"
        );
        assert!(ticks(Granularity::Year, 10, 5).is_empty());
    }

    #[test]
    fn the_part_of_the_axis_a_camera_shows_is_found_from_its_projection() {
        // An orthographic camera at the origin looking along -z, spanning
        // 100 across and 100 up.
        let ortho = Mat4::orthographic_rh(-50.0, 50.0, -50.0, 50.0, -1000.0, 1000.0);
        assert_eq!(
            visible_heights(ortho, 0.0, -10.0, -300.0, 300.0, 1.0),
            Some((-50.0, 50.0))
        );
        assert_eq!(
            visible_heights(ortho, 0.0, -10.0, -300.0, 300.0, 0.5),
            Some((-50.0, 25.0)),
            "the bar covers the top quarter"
        );
        assert_eq!(
            visible_heights(ortho, 0.0, -10.0, 0.0, 30.0, 1.0),
            Some((0.0, 30.0)),
            "all of it"
        );
        assert_eq!(
            visible_heights(ortho, 80.0, -10.0, -300.0, 300.0, 1.0),
            None,
            "off to the side"
        );
        assert_eq!(
            visible_heights(ortho, 0.0, -10.0, 60.0, 90.0, 1.0),
            None,
            "above the view"
        );
        // A perspective camera with a right angle of view: at 10 in front
        // it spans 10 each way.
        let perspective =
            Mat4::perspective_infinite_reverse_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1);
        let (lo, hi) = visible_heights(perspective, 0.0, -10.0, -100.0, 100.0, 1.0).unwrap();
        assert!(
            (lo + 10.0).abs() < 1e-3 && (hi - 10.0).abs() < 1e-3,
            "{lo}..{hi}"
        );
        assert_eq!(
            visible_heights(perspective, 0.0, 10.0, -100.0, 100.0, 1.0),
            None,
            "behind"
        );
    }

    #[test]
    fn an_end_that_clips_has_a_break_mark_past_it() {
        // At a pixel to the world unit.
        let lines = break_mark(Vec2::new(-10.0, 4.0), 0.0, -1.0, 1.0);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            [
                Vec3::new(-10.0, 0.0, 4.0),
                Vec3::new(-10.0, -BREAK_LENGTH, 4.0)
            ],
            "a stub of the line below the foot"
        );
        let middles: Vec<Vec3> = lines[1..].iter().map(|[a, b]| (*a + *b) / 2.0).collect();
        for ([a, b], middle) in lines[1..].iter().zip(&middles) {
            assert!(
                middle.y < 0.0 && middle.y > -BREAK_LENGTH,
                "across the stub"
            );
            assert_eq!((middle.x, middle.z), (-10.0, 4.0));
            assert!(b.x > a.x && b.y > a.y, "slanting the same way");
        }
        assert!(((middles[0].y - middles[1].y).abs() - BREAK_STEP).abs() < 1e-5);
        let top = break_mark(Vec2::ZERO, 400.0, 1.0, 0.25);
        assert_eq!(
            top[0][1].y,
            400.0 + BREAK_LENGTH * 0.25,
            "above the top, a quarter as long at four pixels to the unit"
        );
        assert_eq!(now_text(false, false), "now");
        assert_eq!(now_text(true, false), "now, before the axis");
        assert_eq!(now_text(false, true), "now, after the axis");
    }

    #[test]
    fn calendar_steps_round_down_and_up() {
        let at = micros("2026-10-21T09:30:00Z");
        assert_eq!(
            round_down(Granularity::Month, at),
            Some(micros("2026-10-01T00:00:00Z"))
        );
        assert_eq!(
            round_up(Granularity::Month, at),
            Some(micros("2026-11-01T00:00:00Z"))
        );
        assert_eq!(
            round_up(Granularity::Week, at),
            Some(micros("2026-10-26T00:00:00Z")),
            "the next Monday"
        );
        assert_eq!(
            round_down(Granularity::Century, micros("1372-01-01T00:00:00Z")),
            Some(micros("1300-01-01T00:00:00Z"))
        );
        let midnight = micros("2026-10-21T00:00:00Z");
        assert_eq!(round_up(Granularity::Day, midnight), Some(midnight));
        assert_eq!(round_down(Granularity::Day, midnight), Some(midnight));
    }

    #[test]
    fn the_axis_stands_outside_the_maps_corner_and_reaches_now() {
        let points = [Vec3::new(-10.0, 0.0, 4.0), Vec3::new(30.0, 50.0, -20.0)];
        let (foot, middle, radius) = axis_place(&points).unwrap();
        assert_eq!(foot, Vec2::new(-10.0 - AXIS_GAP, -20.0 - AXIS_GAP));
        assert_eq!(middle, Vec2::new(10.0, -8.0));
        assert!((radius - (Vec2::new(20.0, 12.0).length() + RING_PAD)).abs() < 1e-4);
        assert_eq!(axis_place(&[]), None);
        let axis = TimeAxis {
            start: 0,
            span: 10,
            height: 300.0,
        };
        assert_eq!(axis_range(&axis, &[], 150.0), (0.0, 300.0));
        assert_eq!(
            axis_range(&axis, &[], 320.0),
            (0.0, 320.0),
            "now after the latest node"
        );
        assert_eq!(
            axis_range(&axis, &[Some((-40.0, 10.0)), None], 150.0),
            (-40.0, 300.0),
            "a smear's tail"
        );
    }
}
