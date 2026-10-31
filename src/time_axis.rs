//! Time (DESIGN.md §4): where each node stands on the time axis in each of
//! the two time modes, as plain data and pure functions.
//!
//! The axis is calendar time in both modes. The modes differ in which
//! instant or period a node is placed at:
//!
//! - **Recorded time** places every node at its `created_at`, as the layout
//!   does (`layout.rs`).
//! - **Claim time** places a claim by its timepoint, reached through its
//!   `timelink` edge and the edge's `metadata.timepoint_id` on a timeline
//!   in the snapshot. An `interval` is a solid column from its start to its
//!   end; an `instant` is a thin disc at its start; a `vague` point with
//!   one or both of its bounds is a smear across what is known, a missing
//!   bound fading out over [`FADE_SHARE`] of the axis; a `vague` point with
//!   neither bound, a contested point, a kind this version does not know,
//!   or a timepoint the snapshot does not hold has no position, and the
//!   claim is listed in the tray instead of drawn. A claim with no
//!   `timelink` at all stays a disc at its recorded time, never a column,
//!   and topics stay discs at their recorded time in both modes. A claim
//!   with several timelinks is drawn by the one that starts earliest, and
//!   the others are listed in its tray entry.
//!
//! The recorded axis spans every node and is [`TIME_HEIGHT`] high, as it
//! always was. The claim axis spans the bulk of what claim time places,
//! the central 98% of its instants widened to whole units of the ticks,
//! and whatever lies beyond it is drawn clipped to the end it lies beyond
//! (`clip.rs`), so that one far timepoint does not squeeze the rest into a
//! band. A graph whose claims have no timepoint with a position draws claim
//! time on the recorded axis. The claim axis keeps the recorded axis's
//! scale while it can and is capped at [`MAX_TIME_HEIGHT`], beyond which a
//! longer range is squeezed into it. Once drawn, neither axis is rescaled:
//! a node arriving on the event stream, or in a snapshot read again over
//! the picture ([`build_seeded`]), grows an axis upward at its own scale to
//! reach its recorded time (`layout::extend_axis`), past the cap where it
//! must, so that nothing already drawn moves. On the recorded axis an
//! instant below the start stands below 0; on the claim axis a timepoint
//! beyond either end is drawn clipped, and grows nothing.
//!
//! A `sourced_from` edge's validity intervals are kept per node for the
//! brackets drawn beside a focused column, with each end read as a
//! coordinate where it is a precise instant and as what it was otherwise.
//!
//! "Now" is the first timeline's `reference_time`, or the wall clock where
//! no timeline states one; the wall clock is read when the graph is loaded
//! and passed in as a value. In claim time a "now" beyond the axis stands
//! at the end it lies beyond, as a clipped instant does.

use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::clip::{self, Clipped, beyond, claim_range, clipped_span};
use crate::hub::{
    ImpreciseInstant, Snapshot, TimelineView, TimepointView, ValidityInterval, parse_timestamp,
};
use crate::layout::{Kind, Layout, TIME_HEIGHT, TimeAxis, extend_axis, time_axis, time_y};

/// The highest the claim axis grows, in world units: twice the recorded
/// axis, so that the picture stays within a few hundred units.
pub const MAX_TIME_HEIGHT: f32 = 2.0 * TIME_HEIGHT;
/// How far a smear with a missing bound fades out, as a share of its
/// axis's height: a tenth of the axis.
pub const FADE_SHARE: f32 = 0.1;

/// The time mode: which time the axis places a node at (DESIGN.md §4, §10).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeMode {
    #[default]
    Recorded,
    Claim,
}

/// The other mode.
pub fn other_mode(mode: TimeMode) -> TimeMode {
    match mode {
        TimeMode::Recorded => TimeMode::Claim,
        TimeMode::Claim => TimeMode::Recorded,
    }
}

/// Each mode's name on the command line.
pub const MODE_NAMES: [(&str, TimeMode); 2] =
    [("recorded", TimeMode::Recorded), ("claim", TimeMode::Claim)];

/// The mode with this name, ignoring case.
pub fn parse_mode(name: &str) -> Option<TimeMode> {
    MODE_NAMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, mode)| mode)
}

/// How a node is drawn along the axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    /// A disc at the node's recorded time.
    Disc,
    /// An instant timepoint: a thin solid disc at it.
    Instant,
    /// An interval timepoint: a solid column from its start to its end.
    Solid,
    /// A vague point with both bounds: a smear across them, fading toward
    /// each end.
    FadeBoth,
    /// A vague point with its earliest bound only: a smear up from it,
    /// fading out toward the top.
    FadeTop,
    /// A vague point with its latest bound only: a smear down from it,
    /// fading out toward the bottom.
    FadeBottom,
}

/// The stretch of the axis a node spans, as heights, and how it is drawn.
/// `lo` and `hi` are equal for a disc. A span clipped to the claim axis
/// says how many of its instants lie beyond each end (`clip.rs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub lo: f32,
    pub hi: f32,
    pub profile: Profile,
    pub clipped: Clipped,
}

/// A disc's span at a height.
pub fn disc_at(y: f32) -> Span {
    Span {
        lo: y,
        hi: y,
        profile: Profile::Disc,
        clipped: Clipped::default(),
    }
}

/// The middle of a span.
pub fn centre(span: &Span) -> f32 {
    (span.lo + span.hi) / 2.0
}

/// Where a node stands in time, in microseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// At its recorded time.
    Recorded(i64),
    Instant(i64),
    Interval {
        start: i64,
        end: i64,
    },
    /// A vague point, by the bounds it has: at least one.
    Smear {
        earliest: Option<i64>,
        latest: Option<i64>,
    },
}

/// Why a timepoint has no position on the axis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unplaced {
    /// A vague point with neither bound.
    NoBounds,
    /// The stated order cannot hold.
    Contested,
    /// A kind this version does not know, as the hub named it.
    UnknownKind(String),
    /// The timelink carries no timepoint id, or names a timeline or a
    /// timepoint the snapshot does not hold.
    NotInSnapshot,
    /// A date the kind needs is missing or does not parse, or the dates
    /// run backwards.
    BadDates,
}

/// A timepoint a claim is linked to, and where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Linked {
    pub timepoint_id: String,
    pub label: Option<String>,
    pub place: Result<Placement, Unplaced>,
}

/// One end of a validity interval, as the axis can use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bound {
    /// A precise instant: a coordinate.
    At(i64),
    /// A name, with no coordinate.
    Named(String),
    Unknown,
    Unbounded,
    /// A precise instant whose time does not parse, or a kind this version
    /// does not know.
    Unreadable,
}

/// A validity interval of one of a node's `sourced_from` edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validity {
    pub edge_id: String,
    /// The document the edge points at.
    pub document: String,
    pub start: Bound,
    pub end: Bound,
    /// Whether the dates were in the text, rather than read from tense and
    /// context.
    pub stated: bool,
}

/// One mode's axis and every drawn node's span on it, in layout order;
/// `None` for a node with no position in that mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModeTimes {
    pub axis: TimeAxis,
    pub spans: Vec<Option<Span>>,
}

/// Where "now" came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NowSource {
    /// A timeline's `reference_time`, by the timeline's name.
    Reference(String),
    #[default]
    WallClock,
}

/// Everything about time the scene reads, built once per graph by
/// [`build`] and kept as a resource.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct TimeModel {
    pub recorded: ModeTimes,
    pub claim: ModeTimes,
    /// Each drawn node's linked timepoints, in layout order: the one its
    /// column draws first, then the rest by start, then those with no
    /// position. Empty for a topic and for a claim with no timelink.
    pub linked: Vec<Vec<Linked>>,
    /// Each drawn node's validity intervals, from its `sourced_from` edges.
    pub validity: Vec<Vec<Validity>>,
    /// Each drawn node's place in claim time, as dates, in layout order;
    /// `None` for a node with no position. What a clipped span leaves out.
    pub claim_places: Vec<Option<Placement>>,
    /// "Now", in microseconds since the Unix epoch.
    pub now: i64,
    pub now_source: NowSource,
    pub counts: ClaimCounts,
}

/// One mode's times.
pub fn times(model: &TimeModel, mode: TimeMode) -> &ModeTimes {
    match mode {
        TimeMode::Recorded => &model.recorded,
        TimeMode::Claim => &model.claim,
    }
}

/// The height of "now" on a mode's axis. In claim time a "now" beyond the
/// axis stands at the end it lies beyond.
pub fn now_height(model: &TimeModel, mode: TimeMode) -> f32 {
    let axis = &times(model, mode).axis;
    let y = time_y(axis, model.now);
    match mode {
        TimeMode::Recorded => y,
        TimeMode::Claim => y.clamp(0.0, axis.height.max(0.0)),
    }
}

/// Whether "now" lies beyond the axis drawn: before it or after it. Never
/// in recorded time, whose "now" is drawn where it falls.
pub fn now_clipped(model: &TimeModel, mode: TimeMode) -> Clipped {
    match mode {
        TimeMode::Recorded => Clipped::default(),
        TimeMode::Claim => beyond(&Placement::Instant(model.now), &model.claim.axis),
    }
}

/// What claim time makes of the graph, for the log and the report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClaimCounts {
    /// Drawn nodes by how claim time draws them.
    pub discs: usize,
    pub instants: usize,
    pub columns: usize,
    pub smears: usize,
    /// Claims linked to timepoints none of which has a position.
    pub unplaced: usize,
    /// Claims linked to more than one timepoint.
    pub several: usize,
    /// The timepoints on the snapshot's timelines, by kind, and how many
    /// of each some drawn claim's column draws.
    pub interval_points: usize,
    pub interval_points_drawn: usize,
    pub vague_points: usize,
    pub vague_points_with_bounds: usize,
    pub vague_points_drawn: usize,
    pub instant_points: usize,
    pub instant_points_drawn: usize,
    pub other_points: usize,
    /// Drawn nodes clipped to the claim axis, and how many of their
    /// instants lie before it and after it.
    pub clipped_nodes: usize,
    pub clipped_below: usize,
    pub clipped_above: usize,
}

/// Reads a timepoint's place on the axis, by its kind.
pub fn read_timepoint(point: &TimepointView) -> Result<Placement, Unplaced> {
    if point.contested {
        return Err(Unplaced::Contested);
    }
    let date = |text: &Option<String>| -> Result<Option<i64>, Unplaced> {
        match text {
            None => Ok(None),
            Some(text) => parse_timestamp(text).map(Some).ok_or(Unplaced::BadDates),
        }
    };
    match point.kind.as_str() {
        "interval" => match (date(&point.start)?, date(&point.end)?) {
            (Some(start), Some(end)) if start <= end => Ok(Placement::Interval { start, end }),
            _ => Err(Unplaced::BadDates),
        },
        "instant" => date(&point.start)?
            .map(Placement::Instant)
            .ok_or(Unplaced::BadDates),
        "vague" => match (date(&point.earliest)?, date(&point.latest)?) {
            (None, None) => Err(Unplaced::NoBounds),
            (Some(earliest), Some(latest)) if earliest > latest => Err(Unplaced::BadDates),
            (earliest, latest) => Ok(Placement::Smear { earliest, latest }),
        },
        other => Err(Unplaced::UnknownKind(other.to_owned())),
    }
}

/// When a placement starts, for the earliest-start rule: the lowest bound
/// it states.
pub fn start(placement: &Placement) -> i64 {
    match *placement {
        Placement::Recorded(t) | Placement::Instant(t) => t,
        Placement::Interval { start, .. } => start,
        Placement::Smear { earliest, latest } => earliest.or(latest).unwrap_or(i64::MAX),
    }
}

/// Every instant a placement states.
pub fn instants(placement: &Placement) -> Vec<i64> {
    match *placement {
        Placement::Recorded(t) | Placement::Instant(t) => vec![t],
        Placement::Interval { start, end } => vec![start, end],
        Placement::Smear { earliest, latest } => earliest.into_iter().chain(latest).collect(),
    }
}

/// How far a missing bound fades out on an axis.
pub fn fade_distance(axis: &TimeAxis) -> f32 {
    FADE_SHARE * axis.height
}

/// A placement's span on an axis.
pub fn span(placement: &Placement, axis: &TimeAxis) -> Span {
    let y = |t: i64| time_y(axis, t);
    let fade = fade_distance(axis);
    let (lo, hi, profile) = match *placement {
        Placement::Recorded(t) => (y(t), y(t), Profile::Disc),
        Placement::Instant(t) => (y(t), y(t), Profile::Instant),
        Placement::Interval { start, end } => (y(start), y(end), Profile::Solid),
        Placement::Smear {
            earliest: Some(earliest),
            latest: Some(latest),
        } => (y(earliest), y(latest), Profile::FadeBoth),
        Placement::Smear {
            earliest: Some(earliest),
            latest: None,
        } => (y(earliest), y(earliest) + fade, Profile::FadeTop),
        Placement::Smear {
            earliest: None,
            latest: Some(latest),
        } => (y(latest) - fade, y(latest), Profile::FadeBottom),
        Placement::Smear {
            earliest: None,
            latest: None,
        } => (0.0, 0.0, Profile::Disc),
    };
    Span {
        lo,
        hi,
        profile,
        clipped: Clipped::default(),
    }
}

/// The height of the claim axis for a range of time, given the recorded
/// axis's span: the recorded axis's scale while it fits, never less than
/// [`TIME_HEIGHT`] and never more than [`MAX_TIME_HEIGHT`].
pub fn axis_height(span: i64, recorded_span: i64) -> f32 {
    if recorded_span <= 0 || span <= 0 {
        return TIME_HEIGHT;
    }
    let scaled = TIME_HEIGHT as f64 * span as f64 / recorded_span as f64;
    (scaled as f32).clamp(TIME_HEIGHT, MAX_TIME_HEIGHT)
}

/// The order linked timepoints are kept in: those with a position by
/// start, ties by id, then those with none, by id.
pub fn linked_order(a: &Linked, b: &Linked) -> std::cmp::Ordering {
    let key = |l: &Linked| match &l.place {
        Ok(p) => (0, start(p)),
        Err(_) => (1, 0),
    };
    key(a)
        .cmp(&key(b))
        .then_with(|| a.timepoint_id.cmp(&b.timepoint_id))
}

/// A node's place in claim time: a topic or a claim with no timelink at
/// its recorded time, a claim by the first of its linked timepoints that
/// has a position, and `None` for a claim none of whose timepoints has
/// one.
pub fn claim_placement(kind: Kind, recorded: i64, linked: &[Linked]) -> Option<Placement> {
    if kind == Kind::Topic || linked.is_empty() {
        return Some(Placement::Recorded(recorded));
    }
    linked.first().and_then(|l| l.place.clone().ok())
}

/// Reads one end of a validity interval.
pub fn bound(instant: &ImpreciseInstant) -> Bound {
    match instant {
        ImpreciseInstant::Precise { at, .. } => {
            parse_timestamp(at).map_or(Bound::Unreadable, Bound::At)
        }
        ImpreciseInstant::Named { label } => Bound::Named(label.clone()),
        ImpreciseInstant::Unknown => Bound::Unknown,
        ImpreciseInstant::Unbounded => Bound::Unbounded,
        ImpreciseInstant::Other => Bound::Unreadable,
    }
}

pub fn read_validity(edge_id: &str, document: &str, interval: &ValidityInterval) -> Validity {
    Validity {
        edge_id: edge_id.to_owned(),
        document: document.to_owned(),
        start: bound(&interval.start),
        end: bound(&interval.end),
        stated: interval.basis == "stated",
    }
}

/// The timepoint a `timelink` names, by its timeline and its id, read from
/// the timelines held: where it stands, or why it has no position.
pub fn link(timelines: &[TimelineView], timeline_id: &str, timepoint_id: Option<&str>) -> Linked {
    let point = timepoint_id.and_then(|id| {
        timelines
            .iter()
            .filter(|t| t.timeline_id == timeline_id)
            .flat_map(|t| &t.timepoints)
            .find(|p| p.timepoint_id == id)
    });
    Linked {
        timepoint_id: timepoint_id.unwrap_or_default().to_owned(),
        label: point.and_then(|p| p.label.clone()),
        place: point.map_or(Err(Unplaced::NotInSnapshot), read_timepoint),
    }
}

/// Adds a linked timepoint to a claim's list, once per timepoint id; a
/// timelink with no id is added every time, since nothing says two of them
/// are the same.
pub fn add_link(list: &mut Vec<Linked>, entry: Linked, has_id: bool) {
    if !has_id || !list.iter().any(|l| l.timepoint_id == entry.timepoint_id) {
        list.push(entry);
    }
}

/// "Now": the first timeline's reference time, else the wall clock.
pub fn now_of(timelines: &[TimelineView], wall_clock: i64) -> (i64, NowSource) {
    timelines
        .iter()
        .find_map(|t| {
            let at = parse_timestamp(t.reference_time.as_deref()?)?;
            Some((at, NowSource::Reference(t.name.clone())))
        })
        .unwrap_or((wall_clock, NowSource::WallClock))
}

/// Builds the time model of a snapshot as the layout placed it, with the
/// wall clock at `wall_clock` microseconds since the Unix epoch.
pub fn build(snapshot: &Snapshot, layout: &Layout, wall_clock: i64) -> TimeModel {
    build_seeded(snapshot, layout, wall_clock, None)
}

/// [`build`], over a picture already drawn whose claim axis is
/// `claim_axis`: the claim axis keeps that start and scale and grows upward
/// to reach the recorded times of the discs the snapshot places, rather
/// than being chosen afresh, and a timepoint beyond it is drawn clipped.
pub fn build_seeded(
    snapshot: &Snapshot,
    layout: &Layout,
    wall_clock: i64,
    claim_axis: Option<TimeAxis>,
) -> TimeModel {
    let count = layout.nodes.len();
    let by_id: HashMap<&str, usize> = layout
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.node_id.as_str(), i))
        .collect();
    // The layout draws a repeated id once, as it first appears, so its
    // time is read from there too.
    let mut recorded = vec![0_i64; count];
    let mut seen = vec![false; count];
    for node in &snapshot.nodes {
        if let Some(&i) = by_id.get(node.node_id.as_str())
            && !seen[i]
        {
            seen[i] = true;
            recorded[i] = parse_timestamp(&node.created_at).unwrap_or_default();
        }
    }
    let points: HashMap<(&str, &str), &TimepointView> = snapshot
        .timelines
        .iter()
        .flat_map(|t| {
            t.timepoints
                .iter()
                .map(move |p| ((t.timeline_id.as_str(), p.timepoint_id.as_str()), p))
        })
        .collect();

    let mut linked: Vec<Vec<Linked>> = vec![Vec::new(); count];
    let mut validity: Vec<Vec<Validity>> = vec![Vec::new(); count];
    for edge in &snapshot.edges {
        let Some(&i) = by_id.get(edge.src_id.as_str()) else {
            continue;
        };
        match edge.edge_type.as_str() {
            "timelink" if layout.nodes[i].kind != Kind::Topic => {
                let id = edge.metadata.get("timepoint_id").and_then(|v| v.as_str());
                let point = id.and_then(|id| points.get(&(edge.dst_id.as_str(), id)));
                let entry = Linked {
                    timepoint_id: id.unwrap_or_default().to_owned(),
                    label: point.and_then(|p| p.label.clone()),
                    place: point.map_or(Err(Unplaced::NotInSnapshot), |p| read_timepoint(p)),
                };
                add_link(&mut linked[i], entry, id.is_some());
            }
            "sourced_from" => validity[i].extend(
                edge.validity
                    .iter()
                    .map(|v| read_validity(&edge.edge_id, &edge.dst_id, v)),
            ),
            _ => {}
        }
    }
    for list in &mut linked {
        list.sort_by(linked_order);
    }

    let recorded_times = ModeTimes {
        axis: layout.time,
        spans: layout
            .nodes
            .iter()
            .map(|n| Some(disc_at(n.position.y)))
            .collect(),
    };
    let placements: Vec<Option<Placement>> = (0..count)
        .map(|i| claim_placement(layout.nodes[i].kind, recorded[i], &linked[i]))
        .collect();
    let axis = match claim_axis {
        None => claim_axis_for(&placements, &layout.time),
        Some(seed) => placements
            .iter()
            .flatten()
            .filter_map(|p| match *p {
                Placement::Recorded(t) => Some(t),
                _ => None,
            })
            .fold(seed, |axis, t| extend_axis(&axis, t)),
    };
    let claim_times = ModeTimes {
        axis,
        spans: placements
            .iter()
            .map(|p| p.as_ref().map(|p| clipped_span(p, &axis)))
            .collect(),
    };
    let (now, now_source) = now_of(&snapshot.timelines, wall_clock);
    let counts = counts(&snapshot.timelines, &linked, &claim_times);
    TimeModel {
        recorded: recorded_times,
        claim: claim_times,
        linked,
        validity,
        claim_places: placements,
        now,
        now_source,
        counts,
    }
}

/// The claim axis chosen afresh for these placements, beside a recorded
/// axis: the recorded axis itself where every placement is a disc at its
/// recorded time, and otherwise the bulk of their instants
/// ([`claim_range`]), as high as [`axis_height`] makes it.
pub fn claim_axis_for(placements: &[Option<Placement>], recorded: &TimeAxis) -> TimeAxis {
    let timed = placements
        .iter()
        .flatten()
        .any(|p| !matches!(p, Placement::Recorded(_)));
    if !timed {
        return *recorded;
    }
    let all: Vec<i64> = placements.iter().flatten().flat_map(instants).collect();
    match claim_range(&all) {
        Some((start, end)) => TimeAxis {
            start,
            span: end - start,
            height: axis_height(end - start, recorded.span),
        },
        None => time_axis(std::iter::empty(), TIME_HEIGHT),
    }
}

/// What claim time makes of the graph: how each node is drawn, and how
/// many of the timelines' points some claim's column draws.
pub fn counts(
    timelines: &[TimelineView],
    linked: &[Vec<Linked>],
    claim: &ModeTimes,
) -> ClaimCounts {
    let mut counts = ClaimCounts::default();
    for (i, span) in claim.spans.iter().enumerate() {
        if let Some(clipped) = span.map(|s| s.clipped).filter(clip::is_clipped) {
            counts.clipped_nodes += 1;
            counts.clipped_below += usize::from(clipped.below);
            counts.clipped_above += usize::from(clipped.above);
        }
        match span.map(|s| s.profile) {
            Some(Profile::Disc) => counts.discs += 1,
            Some(Profile::Instant) => counts.instants += 1,
            Some(Profile::Solid) => counts.columns += 1,
            Some(Profile::FadeBoth | Profile::FadeTop | Profile::FadeBottom) => counts.smears += 1,
            None => counts.unplaced += 1,
        }
        counts.several += usize::from(linked[i].len() > 1);
    }
    let drawn: std::collections::HashSet<&str> = linked
        .iter()
        .zip(&claim.spans)
        .filter(|(_, span)| span.is_some())
        .filter_map(|(l, _)| l.first().filter(|l| l.place.is_ok()))
        .map(|l| l.timepoint_id.as_str())
        .collect();
    for point in timelines.iter().flat_map(|t| &t.timepoints) {
        let is_drawn = drawn.contains(point.timepoint_id.as_str());
        match point.kind.as_str() {
            "interval" => {
                counts.interval_points += 1;
                counts.interval_points_drawn += usize::from(is_drawn);
            }
            "vague" => {
                counts.vague_points += 1;
                counts.vague_points_with_bounds +=
                    usize::from(point.earliest.is_some() || point.latest.is_some());
                counts.vague_points_drawn += usize::from(is_drawn);
            }
            "instant" => {
                counts.instant_points += 1;
                counts.instant_points_drawn += usize::from(is_drawn);
            }
            _ => counts.other_points += 1,
        }
    }
    counts
}

/// A line of a tray entry: something about a claim's time that the axis
/// cannot show.
#[derive(Clone, Debug, PartialEq)]
pub enum TrayLine {
    /// A linked timepoint with no position, and why.
    Unplaced { timepoint: String, reason: Unplaced },
    /// A linked timepoint with a position that the column does not draw,
    /// because one that starts earlier is drawn.
    NotDrawn {
        timepoint: String,
        placement: Placement,
    },
}

/// A claim's entry in the tray: whether it is drawn at all in claim time,
/// and the lines that say what the axis leaves out.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayEntry {
    pub node: usize,
    pub placed: bool,
    pub lines: Vec<TrayLine>,
}

/// A timepoint's name in the tray: its label, or the start of its id.
pub fn timepoint_name(linked: &Linked) -> String {
    match &linked.label {
        Some(label) if !label.trim().is_empty() => label.clone(),
        _ if linked.timepoint_id.is_empty() => "a timepoint with no id".to_owned(),
        _ => linked.timepoint_id.chars().take(8).collect(),
    }
}

/// The lines of a node's linked timepoints that its column does not show.
fn linked_lines(linked: &[Linked], placed: bool) -> Vec<TrayLine> {
    let skip = usize::from(placed);
    linked
        .iter()
        .skip(skip)
        .map(|l| match &l.place {
            Ok(placement) => TrayLine::NotDrawn {
                timepoint: timepoint_name(l),
                placement: *placement,
            },
            Err(reason) => TrayLine::Unplaced {
                timepoint: timepoint_name(l),
                reason: reason.clone(),
            },
        })
        .collect()
}

/// The tray's standing entries: every claim linked to a timepoint with no
/// position, and every claim linked to more than one, in layout order.
pub fn standing_tray(model: &TimeModel) -> Vec<TrayEntry> {
    model
        .linked
        .iter()
        .enumerate()
        .filter(|(_, linked)| linked.len() > 1 || linked.iter().any(|l| l.place.is_err()))
        .map(|(node, linked)| {
            let placed = model.claim.spans.get(node).is_some_and(Option::is_some);
            TrayEntry {
                node,
                placed,
                lines: linked_lines(linked, placed),
            }
        })
        .collect()
}

/// A date for the tray and the side panel, in UTC, with the time of day
/// where it is not midnight.
pub fn date(micros: i64) -> String {
    chrono::DateTime::from_timestamp_micros(micros).map_or_else(
        || micros.to_string(),
        |t| {
            if t.time() == chrono::NaiveTime::MIN {
                t.format("%Y-%m-%d").to_string()
            } else {
                t.format("%Y-%m-%d %H:%M UTC").to_string()
            }
        },
    )
}

/// Why a timepoint has no position, in a few words.
pub fn reason_text(reason: &Unplaced) -> String {
    match reason {
        Unplaced::NoBounds => "vague, with no bounds".to_owned(),
        Unplaced::Contested => "contested".to_owned(),
        Unplaced::UnknownKind(kind) => format!("a kind this client does not know, {kind:?}"),
        Unplaced::NotInSnapshot => "not on a timeline in the snapshot".to_owned(),
        Unplaced::BadDates => "its dates are missing or out of order".to_owned(),
    }
}

/// A placement in a few words.
pub fn placement_text(placement: &Placement) -> String {
    match *placement {
        Placement::Recorded(t) => format!("recorded {}", date(t)),
        Placement::Instant(t) => format!("at {}", date(t)),
        Placement::Interval { start, end } => format!("from {} to {}", date(start), date(end)),
        Placement::Smear { earliest, latest } => match (earliest, latest) {
            (Some(e), Some(l)) => format!("between {} and {}", date(e), date(l)),
            (Some(e), None) => format!("after {}", date(e)),
            (None, Some(l)) => format!("before {}", date(l)),
            (None, None) => "with no bounds".to_owned(),
        },
    }
}

/// One end of a validity interval in a few words: its kind of instant, and
/// for a precise one its time; the others have no coordinate, and say so.
pub fn bound_text(bound: &Bound) -> String {
    match bound {
        Bound::At(t) => format!("precise, {}", date(*t)),
        Bound::Named(label) => format!("named {label:?}, no coordinate"),
        Bound::Unknown => "unknown, no coordinate".to_owned(),
        Bound::Unbounded => "unbounded, no coordinate".to_owned(),
        Bound::Unreadable => "unreadable, no coordinate".to_owned(),
    }
}

/// A tray line as the tray shows it.
pub fn line_text(line: &TrayLine) -> String {
    match line {
        TrayLine::Unplaced { timepoint, reason } => {
            format!("{timepoint}: no place, {}", reason_text(reason))
        }
        TrayLine::NotDrawn {
            timepoint,
            placement,
        } => format!("{timepoint}: also linked, {}", placement_text(placement)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_index::tests::{edge, node};
    use crate::hub::{EdgeView, NodeType, NodeView, TimelineView};
    use crate::layout::place;

    fn micros(text: &str) -> i64 {
        parse_timestamp(text).unwrap()
    }

    fn point(id: &str, kind: &str) -> TimepointView {
        TimepointView {
            timepoint_id: id.into(),
            start: None,
            end: None,
            label: None,
            kind: kind.into(),
            earliest: None,
            latest: None,
            contested: false,
            temporal_contradiction_id: None,
            metadata: Default::default(),
        }
    }

    fn dated(id: &str, kind: &str, start: Option<&str>, end: Option<&str>) -> TimepointView {
        TimepointView {
            start: start.map(Into::into),
            end: end.map(Into::into),
            ..point(id, kind)
        }
    }

    fn vague(id: &str, earliest: Option<&str>, latest: Option<&str>) -> TimepointView {
        TimepointView {
            earliest: earliest.map(Into::into),
            latest: latest.map(Into::into),
            ..point(id, "vague")
        }
    }

    fn timeline(points: Vec<TimepointView>) -> TimelineView {
        TimelineView {
            timeline_id: "line".into(),
            name: "The world".into(),
            description: String::new(),
            timepoints: points,
            reference_time: None,
            created_at: "2026-09-01T00:00:00Z".into(),
            graph: "test".into(),
        }
    }

    fn timelink(src: &str, timepoint: Option<&str>) -> EdgeView {
        let mut e = edge(src, "line", "timelink");
        e.edge_id = format!("{src}-{}", timepoint.unwrap_or("none"));
        if let Some(id) = timepoint {
            e.metadata.insert("timepoint_id".into(), id.into());
        }
        e
    }

    fn snapshot(
        nodes: Vec<NodeView>,
        edges: Vec<EdgeView>,
        points: Vec<TimepointView>,
    ) -> Snapshot {
        Snapshot {
            schema_version: Some(1),
            graph: "test".into(),
            nodes,
            edges,
            timelines: vec![timeline(points)],
            metacontexts: Vec::new(),
        }
    }

    const WALL: &str = "2026-10-20T18:00:00Z";

    fn model(snapshot: &Snapshot) -> (TimeModel, HashMap<String, usize>) {
        let layout = place(&snapshot.nodes, &snapshot.edges);
        let ids = layout
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.node_id.clone(), i))
            .collect();
        (build(snapshot, &layout, micros(WALL)), ids)
    }

    /// One topic and a claim for each row of DESIGN.md §4's table, recorded
    /// one a day from 1 September, with timepoints from 2026 back to 2016.
    fn every_row() -> Snapshot {
        let claims = [
            "interval",
            "instant",
            "both",
            "earliest",
            "latest",
            "neither",
            "contested",
            "odd",
            "missing",
            "unlinked",
        ];
        let nodes = std::iter::once(node("topic", NodeType::Topic, 1))
            .chain(
                claims
                    .iter()
                    .enumerate()
                    .map(|(i, id)| node(id, NodeType::Fact, 2 + i as u32)),
            )
            .collect();
        let mut edges: Vec<EdgeView> = [
            ("interval", "p-interval"),
            ("instant", "p-instant"),
            ("both", "p-both"),
            ("earliest", "p-earliest"),
            ("latest", "p-latest"),
            ("neither", "p-neither"),
            ("contested", "p-contested"),
            ("odd", "p-odd"),
            ("missing", "p-gone"),
            // A topic's timelink places nothing.
            ("topic", "p-interval"),
        ]
        .into_iter()
        .map(|(src, point)| timelink(src, Some(point)))
        .collect();
        edges.push(timelink("missing", None));
        let points = vec![
            dated(
                "p-interval",
                "interval",
                Some("2016-01-01T00:00:00Z"),
                Some("2020-01-01T00:00:00Z"),
            ),
            dated("p-instant", "instant", Some("2018-06-01T00:00:00Z"), None),
            vague(
                "p-both",
                Some("2017-01-01T00:00:00Z"),
                Some("2019-01-01T00:00:00Z"),
            ),
            vague("p-earliest", Some("2021-01-01T00:00:00Z"), None),
            vague("p-latest", None, Some("2022-01-01T00:00:00Z")),
            vague("p-neither", None, None),
            TimepointView {
                contested: true,
                ..dated(
                    "p-contested",
                    "interval",
                    Some("2016-01-01T00:00:00Z"),
                    Some("2017-01-01T00:00:00Z"),
                )
            },
            dated("p-odd", "recurring", Some("2016-01-01T00:00:00Z"), None),
        ];
        snapshot(nodes, edges, points)
    }

    #[test]
    fn every_row_of_the_column_table_draws_as_the_design_says() {
        let snapshot = every_row();
        let (model, at) = model(&snapshot);
        let axis = model.claim.axis;
        let y = |t: &str| time_y(&axis, micros(t));
        let span = |id: &str| model.claim.spans[at[id]];
        let fade = fade_distance(&axis);

        let interval = span("interval").unwrap();
        assert_eq!(interval.profile, Profile::Solid);
        assert_eq!(interval.lo, y("2016-01-01T00:00:00Z"));
        assert_eq!(interval.hi, y("2020-01-01T00:00:00Z"));
        assert_eq!(
            interval.lo, 0.0,
            "the earliest instant placed is the axis's foot"
        );

        let instant = span("instant").unwrap();
        assert_eq!(instant.profile, Profile::Instant);
        assert_eq!(
            (instant.lo, instant.hi),
            (y("2018-06-01T00:00:00Z"), y("2018-06-01T00:00:00Z"))
        );

        let both = span("both").unwrap();
        assert_eq!(both.profile, Profile::FadeBoth);
        assert_eq!(
            (both.lo, both.hi),
            (y("2017-01-01T00:00:00Z"), y("2019-01-01T00:00:00Z"))
        );

        let earliest = span("earliest").unwrap();
        assert_eq!(earliest.profile, Profile::FadeTop);
        assert_eq!(earliest.lo, y("2021-01-01T00:00:00Z"));
        assert!(
            (earliest.hi - earliest.lo - fade).abs() < 1e-4,
            "fades out over the fixed distance"
        );

        let latest = span("latest").unwrap();
        assert_eq!(latest.profile, Profile::FadeBottom);
        assert_eq!(latest.hi, y("2022-01-01T00:00:00Z"));
        assert!((latest.hi - latest.lo - fade).abs() < 1e-4);

        for id in ["neither", "contested", "odd", "missing"] {
            assert_eq!(span(id), None, "{id} has no position");
        }
        let reasons: Vec<&Result<Placement, Unplaced>> = ["neither", "contested", "odd"]
            .iter()
            .map(|id| &model.linked[at[*id]][0].place)
            .collect();
        assert_eq!(
            reasons,
            [
                &Err(Unplaced::NoBounds),
                &Err(Unplaced::Contested),
                &Err(Unplaced::UnknownKind("recurring".into()))
            ]
        );
        assert!(
            model.linked[at["missing"]]
                .iter()
                .all(|l| l.place == Err(Unplaced::NotInSnapshot)),
            "a timepoint not on the timeline, and a timelink with no timepoint id"
        );

        for id in ["unlinked", "topic"] {
            let disc = span(id).unwrap();
            assert_eq!(
                disc.profile,
                Profile::Disc,
                "{id} is a disc, never a column"
            );
            assert_eq!(disc.lo, disc.hi);
        }
        // At its recorded time, on the claim axis.
        let unlinked = &snapshot.nodes[10];
        assert_eq!(span("unlinked").unwrap().lo, y(&unlinked.created_at));
        assert_eq!(
            model.recorded.spans[at["interval"]].unwrap().profile,
            Profile::Disc
        );

        assert_eq!(
            model.counts,
            ClaimCounts {
                discs: 2,
                instants: 1,
                columns: 1,
                smears: 3,
                unplaced: 4,
                several: 1,
                interval_points: 2,
                interval_points_drawn: 1,
                vague_points: 4,
                vague_points_with_bounds: 3,
                vague_points_drawn: 3,
                instant_points: 1,
                instant_points_drawn: 1,
                other_points: 1,
                clipped_nodes: 0,
                clipped_below: 0,
                clipped_above: 0,
            }
        );
    }

    #[test]
    fn a_claim_with_several_timelinks_is_drawn_by_the_earliest_start() {
        let nodes = vec![node("t", NodeType::Topic, 1), node("c", NodeType::Fact, 2)];
        let edges = vec![
            timelink("c", Some("late")),
            timelink("c", Some("none")),
            timelink("c", Some("early")),
            timelink("c", Some("early")),
        ];
        let points = vec![
            dated(
                "late",
                "interval",
                Some("2024-01-01T00:00:00Z"),
                Some("2025-01-01T00:00:00Z"),
            ),
            vague("none", None, None),
            TimepointView {
                label: Some("the spring".into()),
                ..vague("early", Some("2023-03-01T00:00:00Z"), None)
            },
        ];
        let (model, at) = model(&snapshot(nodes, edges, points));
        let c = at["c"];
        let ids: Vec<&str> = model.linked[c]
            .iter()
            .map(|l| l.timepoint_id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["early", "late", "none"],
            "a repeated timepoint is listed once"
        );
        let span = model.claim.spans[c].unwrap();
        assert_eq!(span.profile, Profile::FadeTop);
        assert_eq!(
            span.lo,
            time_y(&model.claim.axis, micros("2023-03-01T00:00:00Z"))
        );
        let tray = standing_tray(&model);
        assert_eq!(
            tray,
            [TrayEntry {
                node: c,
                placed: true,
                lines: vec![
                    TrayLine::NotDrawn {
                        timepoint: "late".into(),
                        placement: Placement::Interval {
                            start: micros("2024-01-01T00:00:00Z"),
                            end: micros("2025-01-01T00:00:00Z"),
                        },
                    },
                    TrayLine::Unplaced {
                        timepoint: "none".into(),
                        reason: Unplaced::NoBounds,
                    },
                ],
            }]
        );
        assert_eq!(
            line_text(&tray[0].lines[0]),
            "late: also linked, from 2024-01-01 to 2025-01-01"
        );
        assert_eq!(
            line_text(&tray[0].lines[1]),
            "none: no place, vague, with no bounds"
        );
    }

    #[test]
    fn each_modes_axis_spans_what_it_places_and_the_claim_axis_is_capped() {
        let snapshot = every_row();
        let (model, _) = model(&snapshot);
        let recorded = model.recorded.axis;
        assert_eq!(recorded.height, TIME_HEIGHT);
        assert_eq!(recorded.start, micros("2026-09-01T12:00:00Z"));
        assert_eq!(recorded.span, 10 * crate::presets::DAY_MICROS);
        let claim = model.claim.axis;
        assert_eq!(
            claim.start,
            micros("2016-01-01T00:00:00Z"),
            "the earliest interval's start, on a year already"
        );
        assert_eq!(
            claim.start + claim.span,
            micros("2027-01-01T00:00:00Z"),
            "the latest disc, the unlinked claim on 11 September 2026, \
             widened to the end of its year: nine instants leave none out"
        );
        assert_eq!(model.counts.clipped_nodes, 0);
        assert_eq!(
            claim.height, MAX_TIME_HEIGHT,
            "ten years against ten days: capped"
        );

        // A claim span three halves of the recorded one keeps the scale.
        assert_eq!(axis_height(30, 20), TIME_HEIGHT * 1.5);
        assert_eq!(
            axis_height(10, 20),
            TIME_HEIGHT,
            "never shorter than the recorded axis"
        );
        assert_eq!(axis_height(1_000_000, 20), MAX_TIME_HEIGHT);
        assert_eq!(axis_height(0, 0), TIME_HEIGHT);
    }

    /// A topic and 198 claims recorded an hour apart from midnight on 1
    /// September 2026, and a claim `far` linked to an interval from 1372 to
    /// 1400: two hundred and one instants, which leave out two at each end.
    fn one_far_interval() -> Snapshot {
        let hourly = |k: u32| {
            let at = micros("2026-09-01T00:00:00Z") + i64::from(k) * 3_600_000_000;
            chrono::DateTime::from_timestamp_micros(at)
                .unwrap()
                .to_rfc3339()
        };
        let mut nodes = vec![NodeView {
            created_at: hourly(0),
            ..node("topic", NodeType::Topic, 1)
        }];
        nodes.extend((1..=198).map(|k| NodeView {
            created_at: hourly(k),
            ..node(&format!("c{k}"), NodeType::Fact, 1)
        }));
        nodes.push(NodeView {
            created_at: hourly(100),
            ..node("far", NodeType::Fact, 1)
        });
        let points = vec![TimepointView {
            label: Some("the Crown's first century".into()),
            ..dated(
                "p-far",
                "interval",
                Some("1372-01-01T00:00:00Z"),
                Some("1400-01-01T00:00:00Z"),
            )
        }];
        snapshot(nodes, vec![timelink("far", Some("p-far"))], points)
    }

    #[test]
    fn a_far_timepoint_is_drawn_clipped_and_the_claim_axis_spans_the_bulk() {
        let snapshot = one_far_interval();
        let (model, at) = model(&snapshot);
        let axis = model.claim.axis;
        assert_eq!(
            (axis.start, axis.start + axis.span),
            (
                micros("2026-09-01T00:00:00Z"),
                micros("2026-09-10T00:00:00Z")
            ),
            "the hours from 1 September to 6 a.m. on 9 September, in whole days"
        );
        let far = model.claim.spans[at["far"]].unwrap();
        assert_eq!((far.lo, far.hi), (0.0, 0.0), "flat at the foot");
        assert_eq!(far.profile, Profile::Solid);
        assert_eq!(far.clipped, Clipped { below: 2, above: 0 });
        assert_eq!(
            model.claim_places[at["far"]],
            Some(Placement::Interval {
                start: micros("1372-01-01T00:00:00Z"),
                end: micros("1400-01-01T00:00:00Z"),
            }),
            "the true dates are kept"
        );
        let last = model.claim.spans[at["c198"]].unwrap();
        assert!(
            last.lo < axis.height && !clip::is_clipped(&last.clipped),
            "widened to whole days, the latest hours are back on the axis"
        );
        assert_eq!(
            (
                model.counts.clipped_nodes,
                model.counts.clipped_below,
                model.counts.clipped_above
            ),
            (1, 2, 0)
        );
        assert_eq!(
            model.recorded.axis.start,
            micros("2026-09-01T00:00:00Z"),
            "recorded time is not clipped"
        );
        // "Now", the wall clock on 20 October, is after the axis, and
        // stands at its top in claim time.
        assert_eq!(now_height(&model, TimeMode::Claim), axis.height);
        assert_eq!(now_clipped(&model, TimeMode::Claim).above, 1);
        assert!(now_height(&model, TimeMode::Recorded) > TIME_HEIGHT);
        assert_eq!(now_clipped(&model, TimeMode::Recorded), Clipped::default());
        assert!(standing_tray(&model).is_empty());
        let beyond = clip::beyond_tray(&model);
        assert_eq!(beyond.len(), 1);
        assert_eq!(
            clip::beyond_line(&beyond[0]),
            "from 1372-01-01 to 1400-01-01, before the axis"
        );
    }

    #[test]
    fn read_again_the_claim_axis_grows_for_a_disc_and_clips_a_timepoint() {
        let first = one_far_interval();
        let (drawn, _) = model(&first);
        let mut again = first.clone();
        again.nodes.push(NodeView {
            created_at: "2026-09-20T00:00:00Z".into(),
            ..node("late", NodeType::Fact, 1)
        });
        again.nodes.push(node("future", NodeType::Fact, 2));
        again.edges.push(timelink("future", Some("p-future")));
        again.timelines[0].timepoints.push(dated(
            "p-future",
            "instant",
            Some("2100-01-01T00:00:00Z"),
            None,
        ));
        let layout = place(&again.nodes, &again.edges);
        let model = build_seeded(&again, &layout, micros(WALL), Some(drawn.claim.axis));
        let at = |id: &str| layout.nodes.iter().position(|n| n.node_id == id).unwrap();
        let axis = model.claim.axis;
        assert_eq!(axis.start, drawn.claim.axis.start, "the start is kept");
        assert_eq!(
            axis.start + axis.span,
            micros("2026-09-20T00:00:00Z"),
            "grown to the late disc"
        );
        let scale = |a: &TimeAxis| a.height as f64 / a.span as f64;
        assert!((scale(&axis) - scale(&drawn.claim.axis)).abs() < 1e-12);
        let future = model.claim.spans[at("future")].unwrap();
        assert_eq!((future.lo, future.hi), (axis.height, axis.height));
        assert_eq!(future.clipped, Clipped { below: 0, above: 1 });
    }

    #[test]
    fn a_graph_with_no_timelink_draws_the_same_in_both_modes() {
        let nodes = vec![node("t", NodeType::Topic, 1), node("f", NodeType::Fact, 3)];
        let snapshot = Snapshot {
            timelines: Vec::new(),
            ..snapshot(nodes, Vec::new(), Vec::new())
        };
        let (model, _) = model(&snapshot);
        assert_eq!(model.claim, model.recorded);
        assert!(standing_tray(&model).is_empty());
        assert_eq!(model.now, micros(WALL));
        assert_eq!(model.now_source, NowSource::WallClock);
    }

    #[test]
    fn now_is_the_timelines_reference_time_or_the_wall_clock() {
        let mut snapshot = every_row();
        snapshot.timelines[0].reference_time = Some("2077-01-01T00:00:00Z".into());
        let (model, _) = model(&snapshot);
        assert_eq!(model.now, micros("2077-01-01T00:00:00Z"));
        assert_eq!(model.now_source, NowSource::Reference("The world".into()));
        assert!(
            now_height(&model, TimeMode::Recorded) > TIME_HEIGHT,
            "after the latest node"
        );
    }

    #[test]
    fn dates_that_cannot_be_drawn_have_no_position() {
        let backwards = dated(
            "b",
            "interval",
            Some("2020-01-01T00:00:00Z"),
            Some("2019-01-01T00:00:00Z"),
        );
        assert_eq!(read_timepoint(&backwards), Err(Unplaced::BadDates));
        let open = dated("o", "interval", Some("2020-01-01T00:00:00Z"), None);
        assert_eq!(read_timepoint(&open), Err(Unplaced::BadDates));
        let garbled = dated("g", "instant", Some("soon"), None);
        assert_eq!(read_timepoint(&garbled), Err(Unplaced::BadDates));
        let crossed = vague(
            "c",
            Some("2020-01-01T00:00:00Z"),
            Some("2019-01-01T00:00:00Z"),
        );
        assert_eq!(read_timepoint(&crossed), Err(Unplaced::BadDates));
        let same = dated(
            "s",
            "interval",
            Some("2020-01-01T00:00:00Z"),
            Some("2020-01-01T00:00:00Z"),
        );
        assert!(matches!(
            read_timepoint(&same),
            Ok(Placement::Interval { .. })
        ));
    }

    #[test]
    fn validity_ends_are_coordinates_only_when_precise_and_the_rest_say_so() {
        use crate::hub::{ImpreciseInstant as I, ValidityInterval};
        let nodes = vec![node("t", NodeType::Topic, 1), node("f", NodeType::Fact, 2)];
        let mut sourced = edge("f", "doc-12345678", "sourced_from");
        let interval = |start, end| ValidityInterval {
            start,
            end,
            timeline_id: None,
            witnessed_at: None,
            basis: "stated".into(),
        };
        sourced.validity = vec![
            interval(
                I::Precise {
                    at: "2026-09-22T00:00:00Z".into(),
                    label: None,
                },
                I::Unknown,
            ),
            interval(
                I::Named {
                    label: "spring".into(),
                },
                I::Unbounded,
            ),
            interval(
                I::Precise {
                    at: "never".into(),
                    label: None,
                },
                I::Precise {
                    at: "2026-09-24T00:00:00Z".into(),
                    label: None,
                },
            ),
        ];
        let (model, at) = model(&snapshot(nodes, vec![sourced], Vec::new()));
        let f = at["f"];
        let ends: Vec<(&Bound, &Bound)> = model.validity[f]
            .iter()
            .map(|v| (&v.start, &v.end))
            .collect();
        assert_eq!(
            ends,
            [
                (&Bound::At(micros("2026-09-22T00:00:00Z")), &Bound::Unknown),
                (&Bound::Named("spring".into()), &Bound::Unbounded),
                (
                    &Bound::Unreadable,
                    &Bound::At(micros("2026-09-24T00:00:00Z"))
                ),
            ]
        );
        assert!(
            model.validity[f]
                .iter()
                .all(|v| v.stated && v.document == "doc-12345678")
        );
        assert!(
            standing_tray(&model).is_empty(),
            "validity is the side panel's, on focus"
        );
        let words: Vec<String> = model.validity[f]
            .iter()
            .flat_map(|v| [bound_text(&v.start), bound_text(&v.end)])
            .collect();
        assert_eq!(
            words,
            [
                "precise, 2026-09-22",
                "unknown, no coordinate",
                "named \"spring\", no coordinate",
                "unbounded, no coordinate",
                "unreadable, no coordinate",
                "precise, 2026-09-24",
            ]
        );
        assert!(model.validity[at["t"]].is_empty());
    }

    #[test]
    fn the_tray_lists_every_claim_with_a_timepoint_that_has_no_position() {
        let snapshot = every_row();
        let (model, at) = model(&snapshot);
        let tray = standing_tray(&model);
        let listed: Vec<(usize, bool, usize)> = tray
            .iter()
            .map(|e| (e.node, e.placed, e.lines.len()))
            .collect();
        assert_eq!(
            listed,
            [
                (at["neither"], false, 1),
                (at["contested"], false, 1),
                (at["odd"], false, 1),
                (at["missing"], false, 2),
            ]
        );
        let words: Vec<String> = tray.iter().map(|e| line_text(&e.lines[0])).collect();
        assert_eq!(
            words,
            [
                "p-neithe: no place, vague, with no bounds",
                "p-contes: no place, contested",
                "p-odd: no place, a kind this client does not know, \"recurring\"",
                "a timepoint with no id: no place, not on a timeline in the snapshot",
            ]
        );
    }

    #[test]
    fn modes_read_by_name_and_switch_to_each_other() {
        assert_eq!(parse_mode("claim"), Some(TimeMode::Claim));
        assert_eq!(parse_mode("Recorded"), Some(TimeMode::Recorded));
        assert_eq!(parse_mode("valid"), None);
        assert_eq!(other_mode(TimeMode::Claim), TimeMode::Recorded);
        assert_eq!(other_mode(TimeMode::Recorded), TimeMode::Claim);
    }
}
