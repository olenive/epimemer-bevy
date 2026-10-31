//! The time window's range slider: a track standing for the whole time
//! axis of the time mode drawn, from its foot at the left end to its top
//! at the right, and two handles on it, one for each end of the window.
//!
//! A handle's place is a fraction of the track, from 0 at the left to 1 at
//! the right, and stands for the height that fraction of the way up the
//! time axis. Dragging a handle, or pressing the track, moves an end of
//! `Filter::window`; with no window the handles sit at the two ends of the
//! track. The handles may pass each other, as the window allows. Each
//! handle's date is written beside the slider. A switch of time mode keeps
//! the window's stretch of calendar time and moves the handles to it on
//! the new axis (`time_mode.rs`).

use bevy::prelude::*;
use bevy::ui_widgets::observe;

use crate::filter::{Filter, TimeWindow};
use crate::graph_index::GraphIndex;
use crate::layout::{TimeAxis, instant_at};

use super::style;

/// The track's width, in logical pixels.
pub const TRACK_WIDTH: f32 = 240.0;
/// A handle's width and height, in logical pixels.
const HANDLE_WIDTH: f32 = 10.0;
const HANDLE_HEIGHT: f32 = 18.0;
/// The bar the handles slide along, in logical pixels.
const RAIL_HEIGHT: f32 = 4.0;

/// One end of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    From,
    To,
}

/// The slider's track: the node the handles sit in.
#[derive(Component, Debug)]
pub struct Track;

/// A handle, and the fraction it stood at when its drag started.
#[derive(Component, Debug)]
pub struct Handle {
    pub end: End,
    start: f32,
}

/// The stretch of rail between the handles, lit.
#[derive(Component, Debug)]
pub struct Fill;

/// Where the handles stand, as fractions of the track, for a window on a
/// time axis `height` high. No window puts them at the two ends; an end
/// outside the axis stands at the end of the track it is beyond.
pub fn handle_fractions(window: Option<&TimeWindow>, height: f32) -> [f32; 2] {
    match window {
        None => [0.0, 1.0],
        Some(window) => [window.from, window.to].map(|h| fraction_of(h, height)),
    }
}

/// The fraction of the track a height stands at.
pub fn fraction_of(height: f32, axis_height: f32) -> f32 {
    if axis_height > 0.0 {
        (height / axis_height).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The height on the time axis a fraction of the track stands for.
pub fn height_at(fraction: f32, axis_height: f32) -> f32 {
    fraction.clamp(0.0, 1.0) * axis_height
}

/// Where a handle is after a drag of `distance` logical pixels to the right
/// from `start`, on a track `track_width` pixels wide; it stops at the ends.
pub fn dragged(start: f32, distance: f32, track_width: f32) -> f32 {
    if track_width <= 0.0 {
        return start.clamp(0.0, 1.0);
    }
    (start + distance / track_width).clamp(0.0, 1.0)
}

/// The window with one end moved to the height a fraction of the track
/// stands for, the other end where it was. With no window the other end is
/// at its end of the axis.
pub fn with_end(window: Option<&TimeWindow>, end: End, fraction: f32, height: f32) -> TimeWindow {
    let [from, to] = window.map_or([0.0, height], |w| [w.from, w.to]);
    let moved = height_at(fraction, height);
    match end {
        End::From => TimeWindow { from: moved, to },
        End::To => TimeWindow { from, to: moved },
    }
}

/// The handle nearer a fraction of the track; the `To` handle on a tie
/// above both, so two handles at the right end can be pulled apart.
pub fn nearest(fractions: [f32; 2], at: f32) -> End {
    let [from, to] = fractions.map(|f| (f - at).abs());
    if from < to || (from == to && at < fractions[0]) {
        End::From
    } else {
        End::To
    }
}

/// The date and time a height stands for, in UTC to the minute.
pub fn stamp(axis: &TimeAxis, height: f32) -> String {
    let micros = instant_at(axis, height);
    chrono::DateTime::from_timestamp_micros(micros).map_or_else(
        || micros.to_string(),
        |t| t.format("%Y-%m-%d %H:%M").to_string(),
    )
}

/// The slider's dates, the earlier first.
pub fn range_text(window: Option<&TimeWindow>, axis: &TimeAxis) -> String {
    let [a, b] = handle_fractions(window, axis.height).map(|f| height_at(f, axis.height));
    format!("{} to {}", stamp(axis, a.min(b)), stamp(axis, a.max(b)))
}

/// The slider: a track with its rail, its fill and its two handles.
pub fn slider() -> impl Bundle {
    (
        Track,
        Node {
            width: px(TRACK_WIDTH),
            height: px(HANDLE_HEIGHT),
            // Room for the handles, which stand half over the ends.
            margin: UiRect::horizontal(px(HANDLE_WIDTH)),
            ..default()
        },
        observe(press_track),
        children![
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    right: px(0),
                    top: px((HANDLE_HEIGHT - RAIL_HEIGHT) / 2.0),
                    height: px(RAIL_HEIGHT),
                    border_radius: BorderRadius::all(px(2)),
                    ..default()
                },
                BackgroundColor(style::RAIL),
                Pickable::IGNORE,
            ),
            (
                Fill,
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(0),
                    width: percent(100),
                    top: px((HANDLE_HEIGHT - RAIL_HEIGHT) / 2.0),
                    height: px(RAIL_HEIGHT),
                    ..default()
                },
                BackgroundColor(style::ACCENT),
                Pickable::IGNORE,
            ),
            handle(End::From),
            handle(End::To),
        ],
    )
}

fn handle(end: End) -> impl Bundle {
    (
        Handle { end, start: 0.0 },
        Button,
        Node {
            position_type: PositionType::Absolute,
            width: px(HANDLE_WIDTH),
            height: px(HANDLE_HEIGHT),
            margin: UiRect::left(px(-HANDLE_WIDTH / 2.0)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(3)),
            ..default()
        },
        BackgroundColor(style::HANDLE),
        BorderColor::all(style::HANDLE_EDGE),
        observe(press_handle),
        observe(start_drag),
        observe(drag_handle),
    )
}

/// A press on a handle is the handle's, not the track's under it.
fn press_handle(mut press: On<Pointer<Press>>) {
    press.propagate(false);
}

/// A press on the track brings the nearer handle to it.
fn press_track(press: On<Pointer<Press>>, index: Res<GraphIndex>, mut filter: ResMut<Filter>) {
    if press.button != PointerButton::Primary {
        return;
    }
    // The UI backend gives the hit relative to the node: -0.5 at its left
    // edge, 0.5 at its right.
    let Some(position) = press.hit.position else {
        return;
    };
    let at = (position.x + 0.5).clamp(0.0, 1.0);
    let height = index.time.height;
    let fractions = handle_fractions(filter.window.as_ref(), height);
    let window = with_end(filter.window.as_ref(), nearest(fractions, at), at, height);
    filter.window = Some(window);
}

fn start_drag(
    start: On<Pointer<DragStart>>,
    index: Res<GraphIndex>,
    filter: Res<Filter>,
    mut handles: Query<&mut Handle>,
) {
    if let Ok(mut handle) = handles.get_mut(start.entity) {
        let [from, to] = handle_fractions(filter.window.as_ref(), index.time.height);
        handle.start = match handle.end {
            End::From => from,
            End::To => to,
        };
    }
}

fn drag_handle(
    drag: On<Pointer<Drag>>,
    handles: Query<(&Handle, &ChildOf)>,
    tracks: Query<&ComputedNode, With<Track>>,
    index: Res<GraphIndex>,
    mut filter: ResMut<Filter>,
) {
    if drag.button != PointerButton::Primary {
        return;
    }
    let Ok((handle, parent)) = handles.get(drag.entity) else {
        return;
    };
    let width = tracks.get(parent.parent()).map_or(TRACK_WIDTH, |track| {
        track.size().x * track.inverse_scale_factor
    });
    let at = dragged(handle.start, drag.distance.x, width);
    let window = with_end(filter.window.as_ref(), handle.end, at, index.time.height);
    filter.set_if_neq(Filter {
        window: Some(window),
        ..filter.clone()
    });
}

/// Puts the handles and the fill where the filter's window says, whenever
/// the filter or the graph changes.
pub fn place_handles(
    filter: Res<Filter>,
    index: Res<GraphIndex>,
    mut handles: Query<(&Handle, &mut Node), Without<Fill>>,
    mut fill: Query<&mut Node, With<Fill>>,
) {
    if !filter.is_changed() && !index.is_changed() {
        return;
    }
    let [from, to] = handle_fractions(filter.window.as_ref(), index.time.height);
    for (handle, mut node) in &mut handles {
        let at = match handle.end {
            End::From => from,
            End::To => to,
        };
        node.left = percent(at * 100.0);
    }
    for mut node in &mut fill {
        node.left = percent(from.min(to) * 100.0);
        node.width = percent((to - from).abs() * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{TIME_HEIGHT, time_y};

    const H: f32 = TIME_HEIGHT;

    #[test]
    fn no_window_puts_the_handles_at_the_ends_and_a_window_where_its_heights_are() {
        assert_eq!(handle_fractions(None, H), [0.0, 1.0]);
        let window = TimeWindow {
            from: H * 0.25,
            to: H * 0.5,
        };
        assert_eq!(handle_fractions(Some(&window), H), [0.25, 0.5]);
        let crossed = TimeWindow {
            from: H * 0.8,
            to: H * 0.1,
        };
        assert_eq!(handle_fractions(Some(&crossed), H), [0.8, 0.1]);
        let beyond = TimeWindow {
            from: -40.0,
            to: H + 40.0,
        };
        assert_eq!(handle_fractions(Some(&beyond), H), [0.0, 1.0], "clamped");
        assert_eq!(handle_fractions(Some(&window), 0.0), [0.0, 0.0]);
    }

    #[test]
    fn a_fraction_of_the_track_is_that_fraction_of_the_axis_and_back() {
        for fraction in [0.0, 0.1, 0.5, 0.75, 1.0] {
            let height = height_at(fraction, H);
            assert!((height - fraction * H).abs() < 1e-4);
            assert!((fraction_of(height, H) - fraction).abs() < 1e-6);
        }
        assert_eq!(height_at(1.5, H), H, "clamped");
        assert_eq!(height_at(-0.5, H), 0.0);
    }

    #[test]
    fn a_drag_moves_a_handle_by_its_share_of_the_track_and_stops_at_the_ends() {
        assert!((dragged(0.25, 60.0, 240.0) - 0.5).abs() < 1e-6);
        assert!((dragged(0.5, -30.0, 240.0) - 0.375).abs() < 1e-6);
        assert_eq!(dragged(0.9, 500.0, 240.0), 1.0);
        assert_eq!(dragged(0.1, -500.0, 240.0), 0.0);
        assert_eq!(dragged(0.3, 50.0, 0.0), 0.3, "no track, no move");
    }

    #[test]
    fn moving_one_end_keeps_the_other() {
        let window = with_end(None, End::From, 0.5, H);
        assert_eq!(
            window,
            TimeWindow {
                from: H * 0.5,
                to: H
            }
        );
        let window = with_end(Some(&window), End::To, 0.25, H);
        assert_eq!(
            window,
            TimeWindow {
                from: H * 0.5,
                to: H * 0.25
            },
            "the handles may cross"
        );
        assert_eq!(
            handle_fractions(Some(&with_end(None, End::To, 0.2, H)), H),
            [0.0, 0.2]
        );
    }

    #[test]
    fn a_press_takes_the_nearer_handle() {
        assert_eq!(nearest([0.0, 1.0], 0.3), End::From);
        assert_eq!(nearest([0.0, 1.0], 0.7), End::To);
        assert_eq!(nearest([1.0, 1.0], 0.4), End::From, "below both");
        assert_eq!(nearest([0.0, 0.0], 0.4), End::To, "above both");
    }

    #[test]
    fn the_dates_are_the_instants_the_handles_stand_for() {
        let start = crate::hub::parse_timestamp("2026-10-01T00:00:00Z").unwrap();
        let end = crate::hub::parse_timestamp("2026-10-11T00:00:00Z").unwrap();
        let axis = TimeAxis {
            start,
            span: end - start,
            height: H,
        };
        assert_eq!(
            range_text(None, &axis),
            "2026-10-01 00:00 to 2026-10-11 00:00"
        );
        let noon = crate::hub::parse_timestamp("2026-10-03T12:00:00Z").unwrap();
        let window = TimeWindow {
            from: H,
            to: time_y(&axis, noon),
        };
        assert_eq!(
            range_text(Some(&window), &axis),
            "2026-10-03 12:00 to 2026-10-11 00:00",
            "the earlier first"
        );
    }
}
