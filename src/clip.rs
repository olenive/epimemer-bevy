//! The claim axis's clip (DESIGN.md §4): the claim axis spans the bulk of
//! what claim time places, and whatever lies beyond it is drawn at the end
//! it lies beyond. Plain data and pure functions; the axis draws the marks
//! (`axis.rs`) and the tray lists what is clipped (`tray.rs`).
//!
//! One far timepoint must not squeeze the rest of claim time into a band:
//! on the development graph an interval from 1372 to 1399 once stretched
//! the claim axis to six and a half centuries, and the whole of 2026 fell
//! within one unit at its top. So the claim axis is chosen from the bulk of
//! the instants claim time places: the starts and ends of the columns, the
//! instants, the bounds of the smears, and the recorded times of the discs.
//! [`claim_range`] leaves out the lowest and the highest hundredth of them,
//! rounded down, so that the range holds the central 98%; grows a range
//! shorter than [`MIN_CLAIM_SPAN`] evenly about its middle to that length,
//! so that claim times within one day still read; and widens the range
//! outward to whole units of the axis's ticks, the coarsest unit the range
//! holds at least [`MIN_UNITS`] times, so that its ends fall on the calendar
//! and an instant just past the bulk stays on the axis. A graph with fewer
//! than a hundred such instants has none left out. The axis's height
//! follows from the range as it did from the whole span
//! (`time_axis::axis_height`).
//!
//! A span with an instant beyond the axis is clipped ([`clipped_span`]):
//! both its ends are brought within the axis, so a column that starts
//! before the axis is drawn from its foot, a column wholly before it is a
//! flat disc at its foot, and the same at the top. The span keeps how many
//! of its instants lie beyond each end ([`Clipped`]). The tail of a smear
//! with a missing bound is no instant, so it may still reach past the axis
//! while its bound lies within it.
//!
//! Recorded time is never clipped: its axis spans every node. A graph whose
//! claims have no timepoint with a position draws claim time on the
//! recorded axis, so that switching the mode moves nothing.

use bevy::math::Vec3;

use crate::axis::{GRANULARITIES, Granularity, nominal_micros, round_down, round_up};
use crate::layout::TimeAxis;
use crate::presets::DAY_MICROS;
use crate::time_axis::{Placement, Span, TimeModel, date, instants, placement_text, span, start};

/// The shortest claim axis, in microseconds: a day.
pub const MIN_CLAIM_SPAN: i64 = DAY_MICROS;
/// The claim axis's range is widened to the coarsest unit of the axis's
/// ticks that it holds at least this many times, so that the widening adds
/// at most as much again.
pub const MIN_UNITS: i64 = 2;
/// How far the mark at a clipped end of a column stands out from it, how
/// long it is, and how far it reaches to each side, in logical pixels on
/// the screen, so that it reads at any zoom.
pub const MARK_GAP: f32 = 2.0;
pub const MARK_LENGTH: f32 = 7.0;
pub const MARK_WIDTH: f32 = 3.5;

/// How many of a span's instants lie before the axis's start and after its
/// end. A span with any is drawn clipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Clipped {
    pub below: u8,
    pub above: u8,
}

/// Whether a span has an instant beyond either end of the axis.
pub fn is_clipped(clipped: &Clipped) -> bool {
    clipped.below > 0 || clipped.above > 0
}

/// The range of the central share of these instants: what is left once the
/// lowest and the highest hundredth of them, rounded down, are left out.
/// `None` for no instants.
pub fn bulk(instants: &[i64]) -> Option<(i64, i64)> {
    let mut sorted = instants.to_vec();
    sorted.sort_unstable();
    let count = sorted.len();
    let left_out = count / 100;
    let lo = *sorted.get(left_out)?;
    Some((lo, sorted[count - 1 - left_out]))
}

/// A range at least [`MIN_CLAIM_SPAN`] long: a shorter one grows evenly
/// about its middle to that length.
pub fn with_floor((lo, hi): (i64, i64)) -> (i64, i64) {
    if hi.saturating_sub(lo) >= MIN_CLAIM_SPAN {
        return (lo, hi);
    }
    let start = lo + (hi - lo) / 2 - MIN_CLAIM_SPAN / 2;
    (start, start + MIN_CLAIM_SPAN)
}

/// The unit a range this long is widened to: the coarsest step of the
/// axis's ticks that it holds at least [`MIN_UNITS`] times, or a day.
pub fn widening_unit(span: i64) -> Granularity {
    GRANULARITIES
        .iter()
        .copied()
        .find(|&g| span >= MIN_UNITS.saturating_mul(nominal_micros(g)))
        .unwrap_or(Granularity::Day)
}

/// A range widened outward to whole units of [`widening_unit`]: its start
/// down to the step at or before it, its end up to the step at or after
/// it. An end outside the calendar's range stays as it is.
pub fn widen((lo, hi): (i64, i64)) -> (i64, i64) {
    let unit = widening_unit(hi - lo);
    (
        round_down(unit, lo).unwrap_or(lo),
        round_up(unit, hi).unwrap_or(hi),
    )
}

/// The claim axis's range for the instants claim time places: the central
/// 98% of them, at least a day long, widened to whole units. `None` for no
/// instants.
pub fn claim_range(instants: &[i64]) -> Option<(i64, i64)> {
    bulk(instants).map(with_floor).map(widen)
}

/// How many of a placement's instants lie before the axis's start and
/// after its end.
pub fn beyond(placement: &Placement, axis: &TimeAxis) -> Clipped {
    let end = axis.start.saturating_add(axis.span);
    let count = |keep: &dyn Fn(i64) -> bool| {
        let n = instants(placement).into_iter().filter(|&t| keep(t)).count();
        u8::try_from(n).unwrap_or(u8::MAX)
    };
    Clipped {
        below: count(&|t| t < axis.start),
        above: count(&|t| t > end),
    }
}

/// A placement's span on the claim axis, clipped: where any of its instants
/// lies beyond an end of the axis, both ends of the span are brought within
/// the axis, and the span records how many lay beyond each end.
pub fn clipped_span(placement: &Placement, axis: &TimeAxis) -> Span {
    let drawn = span(placement, axis);
    let clipped = beyond(placement, axis);
    if !is_clipped(&clipped) {
        return drawn;
    }
    let top = axis.height.max(0.0);
    Span {
        lo: drawn.lo.clamp(0.0, top),
        hi: drawn.hi.clamp(0.0, top),
        clipped,
        ..drawn
    }
}

/// How many instants of the visible nodes' spans lie before the axis and
/// after it: the counts beside the axis's break marks. A list of visibility
/// too short to say counts a node visible.
pub fn clipped_counts(spans: &[Option<Span>], visible: &[bool]) -> (usize, usize) {
    spans
        .iter()
        .enumerate()
        .filter(|&(i, _)| visible.get(i).copied().unwrap_or(true))
        .filter_map(|(_, span)| span.as_ref())
        .fold((0, 0), |(below, above), s| {
            (
                below + usize::from(s.clipped.below),
                above + usize::from(s.clipped.above),
            )
        })
}

/// The words beside a break mark: how many instants lie beyond that end.
pub fn count_text(count: usize, below: bool) -> String {
    format!("{count} {}", if below { "earlier" } else { "later" })
}

/// Where a clipped placement lies against the axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Every instant before the axis: a flat disc at its foot.
    Before,
    /// Every instant after it: a flat disc at its top.
    After,
    /// From before its start to after its end: its whole height.
    Across,
    /// Starting before it and ending on it: from its foot.
    StartsBefore,
    /// Starting on it and ending after it: up to its top.
    EndsAfter,
}

/// Where a placement, clipped as `clipped` says, lies against the axis.
pub fn side(placement: &Placement, clipped: &Clipped) -> Side {
    let count = instants(placement).len();
    let (below, above) = (usize::from(clipped.below), usize::from(clipped.above));
    match (below, above) {
        (b, 0) if b >= count => Side::Before,
        (0, a) if a >= count => Side::After,
        (b, a) if b > 0 && a > 0 => Side::Across,
        (b, _) if b > 0 => Side::StartsBefore,
        _ => Side::EndsAfter,
    }
}

/// Where a clipped placement lies against the axis, in a few words.
pub fn side_text(placement: &Placement, clipped: &Clipped) -> &'static str {
    match side(placement, clipped) {
        Side::Before => "before the axis",
        Side::After => "after the axis",
        Side::Across => "past both ends of the axis",
        Side::StartsBefore => "starting before the axis",
        Side::EndsAfter => "ending after the axis",
    }
}

/// How a clipped node is drawn, for the side panel: its true dates, where
/// they lie against the axis, and where it is drawn instead.
pub fn clip_note(placement: &Placement, clipped: &Clipped, axis: &TimeAxis) -> String {
    let (from, to) = (date(axis.start), date(axis.start.saturating_add(axis.span)));
    let drawn = match side(placement, clipped) {
        Side::Before => "flat at its foot",
        Side::After => "flat at its top",
        Side::Across => "over its whole height",
        Side::StartsBefore => "from its foot",
        Side::EndsAfter => "up to its top",
    };
    format!(
        "{}, {}, which runs from {from} to {to}, so drawn {drawn}",
        placement_text(placement),
        side_text(placement, clipped)
    )
}

/// A node claim time draws clipped, with its true place.
#[derive(Clone, Debug, PartialEq)]
pub struct BeyondEntry {
    pub node: usize,
    pub placement: Placement,
    pub clipped: Clipped,
}

/// Every node claim time draws clipped: those with an instant before the
/// axis first, then those after it, each by when it starts, ties in layout
/// order.
pub fn beyond_tray(model: &TimeModel) -> Vec<BeyondEntry> {
    let mut entries: Vec<BeyondEntry> = model
        .claim
        .spans
        .iter()
        .enumerate()
        .filter_map(|(node, span)| {
            let span = span.as_ref().filter(|s| is_clipped(&s.clipped))?;
            let placement = model.claim_places.get(node).copied().flatten()?;
            Some(BeyondEntry {
                node,
                placement,
                clipped: span.clipped,
            })
        })
        .collect();
    entries.sort_by_key(|e| (e.clipped.below == 0, start(&e.placement), e.node));
    entries
}

/// A row's line under its node in the tray's "beyond the axis" section:
/// the true dates and where they lie.
pub fn beyond_line(entry: &BeyondEntry) -> String {
    format!(
        "{}, {}",
        placement_text(&entry.placement),
        side_text(&entry.placement, &entry.clipped)
    )
}

/// The lines of the mark at a clipped end of a column: a small arrow
/// pointing out of the axis from `end`, the drawn end of the column at its
/// place, `outward` 1 at the top and -1 at the foot, `pixel` the world
/// length of a logical pixel there. Seen from the side it is an arrowhead
/// with its shaft, from above a cross.
pub fn clip_mark(end: Vec3, outward: f32, pixel: f32) -> Vec<[Vec3; 2]> {
    let base = end + Vec3::Y * outward * MARK_GAP * pixel;
    let tip = base + Vec3::Y * outward * MARK_LENGTH * pixel;
    let width = MARK_WIDTH * pixel;
    [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z]
        .into_iter()
        .map(|side| [tip, base + side * width])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::parse_timestamp;
    use crate::time_axis::{ModeTimes, Profile};

    fn micros(text: &str) -> i64 {
        parse_timestamp(text).unwrap()
    }

    fn day(text: &str) -> i64 {
        micros(&format!("{text}T00:00:00Z"))
    }

    /// The claim axis from 1 July to 1 November 2026, 400 high.
    fn axis() -> TimeAxis {
        let (start, end) = (day("2026-07-01"), day("2026-11-01"));
        TimeAxis {
            start,
            span: end - start,
            height: 400.0,
        }
    }

    #[test]
    fn the_range_holds_the_central_98_percent_and_leaves_the_outliers_out() {
        // A hundred and ninety-six instants an hour apart from noon on 1
        // August, and four far ones: two in the fourteenth century, and
        // 2077 and 2100. Two hundred instants leave out two at each end.
        let first = micros("2026-08-01T12:00:00Z");
        let hour = 3_600_000_000;
        let mut instants: Vec<i64> = (0..196).map(|k| first + k * hour).collect();
        instants.extend([
            day("2100-01-01"),
            day("1372-01-01"),
            day("2077-01-01"),
            day("1399-12-31"),
        ]);
        assert_eq!(
            bulk(&instants),
            Some((first, first + 195 * hour)),
            "the far four are out, every hour is in"
        );
        // Eight days and three hours: whole days hold it more than twice,
        // weeks not twice, so it is widened to whole days.
        assert_eq!(widening_unit(195 * hour), Granularity::Day);
        assert_eq!(
            claim_range(&instants),
            Some((day("2026-08-01"), day("2026-08-10"))),
            "to the midnight after 3 p.m. on 9 August"
        );
        assert_eq!(bulk(&[]), None);
        assert_eq!(claim_range(&[]), None);
        // Fewer than a hundred instants leave none out.
        let few = [day("1372-01-01"), day("2026-08-01"), day("2026-09-01")];
        assert_eq!(bulk(&few), Some((day("1372-01-01"), day("2026-09-01"))));
    }

    #[test]
    fn a_range_is_widened_to_whole_units_of_the_ticks() {
        let year = nominal_micros(Granularity::Year);
        assert_eq!(widening_unit(655 * year), Granularity::Century);
        assert_eq!(widening_unit(150 * year), Granularity::Decade);
        assert_eq!(widening_unit(25 * year), Granularity::Decade);
        assert_eq!(widening_unit(5 * year), Granularity::Year);
        assert_eq!(widening_unit(89 * DAY_MICROS), Granularity::Month);
        assert_eq!(widening_unit(20 * DAY_MICROS), Granularity::Week);
        assert_eq!(widening_unit(3 * DAY_MICROS), Granularity::Day);
        assert_eq!(widening_unit(0), Granularity::Day);

        // The development graph's bulk on 21 October 2026, from 23 July to
        // 20 October: whole months.
        let bulk = (
            micros("2026-07-23T22:20:23Z"),
            micros("2026-10-20T22:10:02Z"),
        );
        assert_eq!(widen(bulk), (day("2026-07-01"), day("2026-11-01")));
        // Three weeks: whole weeks, Monday to Monday.
        assert_eq!(
            widen((
                micros("2026-10-01T09:00:00Z"),
                micros("2026-10-21T09:00:00Z")
            )),
            (day("2026-09-28"), day("2026-10-26"))
        );
        // An end already on a step stays where it is.
        assert_eq!(
            widen((day("2003-06-01"), day("2028-03-01"))),
            (day("2000-01-01"), day("2030-01-01")),
            "twenty-five years are widened to whole decades"
        );
        assert_eq!(
            widen((day("2020-01-01"), day("2026-01-01"))),
            (day("2020-01-01"), day("2026-01-01")),
            "six years on whole years"
        );
    }

    #[test]
    fn a_range_shorter_than_a_day_grows_to_one_and_widens_to_whole_days() {
        let (lo, hi) = (
            micros("2026-10-21T09:00:00Z"),
            micros("2026-10-21T15:00:00Z"),
        );
        assert_eq!(
            with_floor((lo, hi)),
            (day("2026-10-21"), day("2026-10-22")),
            "a day about noon"
        );
        assert_eq!(
            claim_range(&[lo, hi]),
            Some((day("2026-10-21"), day("2026-10-22")))
        );
        // Every claim time at one midnight: a day about it, widened to the
        // two days around it, so it stands in the middle.
        let at = day("2026-10-21");
        assert_eq!(
            claim_range(&[at, at, at]),
            Some((day("2026-10-20"), day("2026-10-22")))
        );
        let long = (day("2026-01-01"), day("2026-03-01"));
        assert_eq!(with_floor(long), long, "a longer range is left alone");
    }

    #[test]
    fn a_column_beyond_an_end_is_drawn_to_that_end_and_counts_its_instants() {
        let axis = axis();
        let interval = |from: &str, to: &str| Placement::Interval {
            start: day(from),
            end: day(to),
        };
        let inside = clipped_span(&interval("2026-08-01", "2026-09-01"), &axis);
        assert_eq!(inside, span(&interval("2026-08-01", "2026-09-01"), &axis));
        assert!(!is_clipped(&inside.clipped));

        let starts_before = clipped_span(&interval("2026-03-01", "2026-08-01"), &axis);
        assert_eq!(starts_before.lo, 0.0, "from the axis's foot");
        assert_eq!(
            starts_before.hi,
            crate::layout::time_y(&axis, day("2026-08-01")),
            "the part inside is drawn as it is"
        );
        assert_eq!(starts_before.clipped, Clipped { below: 1, above: 0 });
        assert_eq!(starts_before.profile, Profile::Solid);

        let before = clipped_span(&interval("1372-01-01", "1400-01-01"), &axis);
        assert_eq!((before.lo, before.hi), (0.0, 0.0), "flat at the foot");
        assert_eq!(before.clipped, Clipped { below: 2, above: 0 });

        let ends_after = clipped_span(&interval("2026-10-01", "2027-03-01"), &axis);
        assert_eq!(ends_after.hi, 400.0, "up to the top");
        assert!(ends_after.lo > 0.0 && ends_after.lo < 400.0);
        assert_eq!(ends_after.clipped, Clipped { below: 0, above: 1 });

        let after = clipped_span(&interval("2100-01-01", "2101-01-01"), &axis);
        assert_eq!((after.lo, after.hi), (400.0, 400.0), "flat at the top");
        assert_eq!(after.clipped, Clipped { below: 0, above: 2 });

        let across = clipped_span(&interval("2000-01-01", "2030-01-01"), &axis);
        assert_eq!((across.lo, across.hi), (0.0, 400.0));
        assert_eq!(across.clipped, Clipped { below: 1, above: 1 });

        let instant = clipped_span(&Placement::Instant(day("1914-07-28")), &axis);
        assert_eq!((instant.lo, instant.hi), (0.0, 0.0));
        assert_eq!(instant.profile, Profile::Instant);
        let disc = clipped_span(&Placement::Recorded(day("2027-01-01")), &axis);
        assert_eq!(disc.clipped, Clipped { below: 0, above: 1 });

        // The ends of the axis are on it.
        let edges = clipped_span(&interval("2026-07-01", "2026-11-01"), &axis);
        assert_eq!((edges.lo, edges.hi), (0.0, 400.0));
        assert!(!is_clipped(&edges.clipped));

        // A smear whose bound is inside keeps its tail past the foot.
        let tail = Placement::Smear {
            earliest: None,
            latest: Some(day("2026-07-03")),
        };
        let smear = clipped_span(&tail, &axis);
        assert!(smear.lo < 0.0 && !is_clipped(&smear.clipped));
        let gone = Placement::Smear {
            earliest: Some(day("2020-01-01")),
            latest: None,
        };
        let smear = clipped_span(&gone, &axis);
        assert_eq!((smear.lo, smear.hi), (0.0, 0.0), "the tail goes with it");
        assert_eq!(smear.clipped, Clipped { below: 1, above: 0 });
    }

    #[test]
    fn the_break_marks_count_the_visible_instants_beyond_each_end() {
        let axis = axis();
        let spans: Vec<Option<Span>> = [
            Some(Placement::Interval {
                start: day("1372-01-01"),
                end: day("1400-01-01"),
            }),
            Some(Placement::Interval {
                start: day("2026-03-01"),
                end: day("2026-08-01"),
            }),
            Some(Placement::Recorded(day("2026-08-01"))),
            None,
            Some(Placement::Interval {
                start: day("2026-10-01"),
                end: day("2027-01-01"),
            }),
            Some(Placement::Instant(day("1914-07-28"))),
        ]
        .iter()
        .map(|p| p.map(|p| clipped_span(&p, &axis)))
        .collect();
        assert_eq!(clipped_counts(&spans, &[]), (4, 1));
        assert_eq!(
            clipped_counts(&spans, &[false, true, true, true, true, true]),
            (2, 1),
            "a hidden node's instants are not counted"
        );
        assert_eq!(count_text(4, true), "4 earlier");
        assert_eq!(count_text(2, false), "2 later");
    }

    #[test]
    fn the_beyond_section_lists_the_clipped_nodes_with_their_dates() {
        let axis = axis();
        let places = vec![
            Some(Placement::Recorded(day("2026-08-01"))),
            Some(Placement::Interval {
                start: day("2026-10-01"),
                end: day("2027-01-01"),
            }),
            Some(Placement::Interval {
                start: day("1914-01-01"),
                end: day("1915-01-01"),
            }),
            None,
            Some(Placement::Interval {
                start: day("1372-01-01"),
                end: day("1400-01-01"),
            }),
            Some(Placement::Interval {
                start: day("2026-06-01"),
                end: day("2026-08-01"),
            }),
        ];
        let model = TimeModel {
            claim: ModeTimes {
                axis,
                spans: places
                    .iter()
                    .map(|p| p.map(|p| clipped_span(&p, &axis)))
                    .collect(),
            },
            claim_places: places,
            ..TimeModel::default()
        };
        let entries = beyond_tray(&model);
        let nodes: Vec<usize> = entries.iter().map(|e| e.node).collect();
        assert_eq!(nodes, [4, 2, 5, 1], "before the axis by start, then after");
        let lines: Vec<String> = entries.iter().map(beyond_line).collect();
        assert_eq!(
            lines,
            [
                "from 1372-01-01 to 1400-01-01, before the axis",
                "from 1914-01-01 to 1915-01-01, before the axis",
                "from 2026-06-01 to 2026-08-01, starting before the axis",
                "from 2026-10-01 to 2027-01-01, ending after the axis",
            ]
        );
        assert_eq!(
            clip_note(&entries[0].placement, &entries[0].clipped, &axis),
            "from 1372-01-01 to 1400-01-01, before the axis, which runs from \
             2026-07-01 to 2026-11-01, so drawn flat at its foot"
        );
        assert_eq!(
            clip_note(&entries[3].placement, &entries[3].clipped, &axis),
            "from 2026-10-01 to 2027-01-01, ending after the axis, which runs from \
             2026-07-01 to 2026-11-01, so drawn up to its top"
        );
        let both = Placement::Interval {
            start: day("2000-01-01"),
            end: day("2030-01-01"),
        };
        assert_eq!(
            side_text(&both, &beyond(&both, &axis)),
            "past both ends of the axis"
        );
    }

    #[test]
    fn a_clipped_end_is_marked_by_an_arrow_pointing_out_of_the_axis() {
        // At a pixel to the world unit.
        let lines = clip_mark(Vec3::new(10.0, 0.0, 5.0), -1.0, 1.0);
        assert_eq!(lines.len(), 4);
        let tip = Vec3::new(10.0, -(MARK_GAP + MARK_LENGTH), 5.0);
        assert!(lines.iter().all(|[from, _]| *from == tip), "below the foot");
        let base = Vec3::new(10.0, -MARK_GAP, 5.0);
        assert!(
            lines.iter().all(|[_, to]| (to.y - base.y).abs() < 1e-6
                && (to.distance(base) - MARK_WIDTH).abs() < 1e-5)
        );
        let up = clip_mark(Vec3::new(0.0, 400.0, 0.0), 1.0, 0.5);
        assert!(
            up.iter()
                .all(|[tip, base]| tip.y > base.y && base.y > 400.0)
        );
        assert!(
            (up[0][0].y - 400.0 - (MARK_GAP + MARK_LENGTH) * 0.5).abs() < 1e-4,
            "at two pixels to the unit, half as long in the world"
        );
    }
}
