//! Validity brackets (DESIGN.md §4): in claim time, the periods the focused
//! node's sources assert it held, each a thin bracket beside its column.
//!
//! A `sourced_from` edge may carry validity intervals, each from a start to
//! an end that is a precise instant, a name, unknown or unbounded. Only a
//! precise instant has a coordinate on the axis. A bracket with both ends
//! precise runs from the one to the other, with a short tick toward the
//! column at each end. A bracket with one precise end has a tick there and
//! runs on past it for [`OPEN_LENGTH`], solid for half of that and dashed
//! for the rest, with no tick, which is how an open end is drawn; the side
//! panel says what the open end was (`panel.rs`). An interval with neither
//! end precise has no bracket at all, only its lines in the panel. The
//! brackets of one node stand side by side, outward from its column, in
//! the order of its edges. They are drawn while a node is focused in claim
//! time, with gizmos, a little fainter for an interval read from tense and
//! context than for one the source stated.

use bevy::prelude::*;

use crate::camera::Places;
use crate::filter::{Shown, is_visible};
use crate::focus::Focus;
use crate::graph_index::GraphIndex;
use crate::layout::{TimeAxis, time_y};
use crate::scene::{BRACKET_COLOUR, FOCUS_SCALE, INFERRED_BRACKET_COLOUR, disc_radius};
use crate::time_axis::{Bound, TimeMode, TimeModel};

/// How far outside the focused disc the first bracket stands, and how far
/// apart the brackets stand, in world units.
pub const BRACKET_GAP: f32 = 0.8;
pub const BRACKET_STEP: f32 = 0.8;
/// How far a closed end's tick reaches toward the column.
pub const BRACKET_TICK: f32 = 0.7;
/// How far an open end runs on past the precise one, and in how many
/// dashes its second half is drawn.
pub const OPEN_LENGTH: f32 = 8.0;
pub const OPEN_DASHES: usize = 4;

/// The pieces of a bracket beside a column standing at `column` on the
/// map, `offset` out from it along the map's x axis, from `start` to `end`
/// on `axis`; `None` where neither end has a coordinate.
pub fn bracket(
    column: Vec3,
    offset: f32,
    start: &Bound,
    end: &Bound,
    axis: &TimeAxis,
) -> Option<Vec<[Vec3; 2]>> {
    let at = |y: f32| Vec3::new(column.x + offset, y, column.z);
    let tick = |y: f32| [at(y), at(y) - Vec3::X * BRACKET_TICK];
    let open = |from: f32, direction: f32| {
        let half = OPEN_LENGTH / 2.0;
        let dash = half / (2 * OPEN_DASHES - 1) as f32;
        let solid = [at(from), at(from + direction * half)];
        std::iter::once(solid)
            .chain((0..OPEN_DASHES).map(move |k| {
                let begin = from + direction * (half + 2.0 * k as f32 * dash);
                [at(begin), at(begin + direction * dash)]
            }))
            .collect::<Vec<_>>()
    };
    match (start, end) {
        (Bound::At(s), Bound::At(e)) => {
            let (lo, hi) = (time_y(axis, *s.min(e)), time_y(axis, *s.max(e)));
            Some(vec![[at(lo), at(hi)], tick(lo), tick(hi)])
        }
        (Bound::At(s), _) => {
            let lo = time_y(axis, *s);
            let mut pieces = vec![tick(lo)];
            pieces.extend(open(lo, 1.0));
            Some(pieces)
        }
        (_, Bound::At(e)) => {
            let hi = time_y(axis, *e);
            let mut pieces = vec![tick(hi)];
            pieces.extend(open(hi, -1.0));
            Some(pieces)
        }
        _ => None,
    }
}

/// The gizmo lines of the brackets.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct BracketLines;

pub struct BracketPlugin;

impl Plugin for BracketPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Focus>()
            .init_resource::<TimeMode>()
            .insert_gizmo_config(
                BracketLines,
                GizmoConfig {
                    line: GizmoLineConfig {
                        width: 2.0,
                        ..default()
                    },
                    ..default()
                },
            )
            .add_systems(PostUpdate, draw_brackets);
    }
}

/// What the brackets are drawn from.
#[derive(bevy::ecs::system::SystemParam)]
struct Source<'w> {
    mode: Res<'w, TimeMode>,
    focus: Res<'w, Focus>,
    model: Option<Res<'w, TimeModel>>,
    index: Res<'w, GraphIndex>,
    places: Res<'w, Places>,
    shown: Res<'w, Shown>,
}

/// Draws the focused node's brackets, in claim time, while it is visible.
fn draw_brackets(source: Source, mut gizmos: Gizmos<BracketLines>) {
    let (Some(node), Some(model)) = (source.focus.0, source.model.as_deref()) else {
        return;
    };
    if *source.mode != TimeMode::Claim
        || source.index.mode != TimeMode::Claim
        || !is_visible(&source.shown.visible, node)
    {
        return;
    }
    let (Some(&column), Some(&kind), Some(intervals)) = (
        source.places.points.get(node),
        source.index.kinds.get(node),
        model.validity.get(node),
    ) else {
        return;
    };
    let radius = disc_radius(kind) * FOCUS_SCALE;
    for (k, validity) in intervals.iter().enumerate() {
        let offset = radius + BRACKET_GAP + k as f32 * BRACKET_STEP;
        let colour = if validity.stated {
            BRACKET_COLOUR
        } else {
            INFERRED_BRACKET_COLOUR
        };
        let pieces = bracket(
            column,
            offset,
            &validity.start,
            &validity.end,
            &source.index.time,
        );
        for [a, b] in pieces.into_iter().flatten() {
            gizmos.line(a, b, colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AXIS: TimeAxis = TimeAxis {
        start: 0,
        span: 1_000,
        height: 100.0,
    };

    #[test]
    fn a_bracket_with_both_ends_runs_between_them_with_a_tick_at_each() {
        let column = Vec3::new(10.0, 50.0, -3.0);
        let pieces = bracket(column, 2.0, &Bound::At(700), &Bound::At(200), &AXIS).unwrap();
        let x = 12.0;
        assert_eq!(
            pieces,
            [
                [Vec3::new(x, 20.0, -3.0), Vec3::new(x, 70.0, -3.0)],
                [
                    Vec3::new(x, 20.0, -3.0),
                    Vec3::new(x - BRACKET_TICK, 20.0, -3.0)
                ],
                [
                    Vec3::new(x, 70.0, -3.0),
                    Vec3::new(x - BRACKET_TICK, 70.0, -3.0)
                ],
            ],
            "the earlier end at the foot, whichever comes first"
        );
    }

    #[test]
    fn an_end_with_no_coordinate_is_drawn_open() {
        let column = Vec3::ZERO;
        let up = bracket(column, 1.0, &Bound::At(300), &Bound::Unknown, &AXIS).unwrap();
        assert_eq!(
            up[0],
            [
                Vec3::new(1.0, 30.0, 0.0),
                Vec3::new(1.0 - BRACKET_TICK, 30.0, 0.0)
            ],
            "closed at the start"
        );
        assert_eq!(
            up.len(),
            2 + OPEN_DASHES,
            "a tick, a solid half and the dashes, with no tick at the end"
        );
        let top = up
            .iter()
            .flat_map(|p| p.iter().map(|v| v.y))
            .fold(f32::MIN, f32::max);
        assert!(
            (top - (30.0 + OPEN_LENGTH)).abs() < 1e-4,
            "runs on past the start: {top}"
        );
        let down = bracket(column, 1.0, &Bound::Unbounded, &Bound::At(300), &AXIS).unwrap();
        let bottom = down
            .iter()
            .flat_map(|p| p.iter().map(|v| v.y))
            .fold(f32::MAX, f32::min);
        assert!((bottom - (30.0 - OPEN_LENGTH)).abs() < 1e-4);
        assert_eq!(
            bracket(
                column,
                1.0,
                &Bound::Named("spring".into()),
                &Bound::Unknown,
                &AXIS
            ),
            None
        );
        assert_eq!(
            bracket(column, 1.0, &Bound::Unreadable, &Bound::Unbounded, &AXIS),
            None
        );
    }
}
