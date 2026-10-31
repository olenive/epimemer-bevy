//! Relaxing the projected topic map (DESIGN.md §7, step 3), placing a new
//! topic in free space, and the spread of claims the map leaves room for.
//!
//! Every topic needs room on the map: its disc and a margin,
//! [`TOPIC_ROOM`], and a share of the space its claims take around it
//! ([`room`]). [`relax`] runs a short pass of a spring and repulsion step
//! over the projected points: two topics nearer than the sum of their
//! rooms are pushed apart along the line between them, each by half of a
//! share of the overlap, and on a graph with `subtopic_of` edges a child
//! further from its parent than their two rooms is pulled toward it. A
//! topic with no overlap and no parent is not moved at all, so the pass
//! changes only what crowds. Each step is worked out from the places
//! before it, in node order, so the pass is deterministic, and it ends
//! when no topic moves further than [`SETTLED`] or after
//! [`MAX_ITERATIONS`]. Neighbours are found through a grid of cells as
//! wide as the largest room, so a step is linear in the topics.
//!
//! A topic that arrives after the fit is projected and then put in free
//! space by [`free_place`]: where its content puts it if that is clear of
//! every other topic's room, else the nearest clear point on a spiral out
//! from there. Nothing else moves.
//!
//! The claims of a topic spread around it by the hash rule of `layout.rs`.
//! At its full spacing a busy topic's claims reach further than the map's
//! topics stand apart, so the claims swallow the neighbours: on the
//! development graph the median distance from a topic to its nearest
//! neighbour is about 8 units, and the busiest topic's claims reach 20. So
//! the map sets a claim spread, the factor [`spread_for`] derives from the
//! median nearest-neighbour distance: the claims spread fully on a map
//! whose topics stand [`FULL_SPREAD_SPACING`] apart, the gap the topic
//! circle's rings kept, and proportionally closer on a denser one, down to
//! [`MIN_SPREAD`]. Only the spacing between ranks scales; the first claim
//! keeps its clearance from the topic's disc.

use std::collections::HashMap;

use bevy::math::Vec2;

use crate::layout::{CLAIM_CLEARANCE, CLAIM_SPACING, Kind};
use crate::scene::disc_radius;

/// The least room a topic takes: its disc's radius and a margin, so two
/// topics with no claims stand at least twice this apart.
pub const TOPIC_ROOM: f32 = 2.5;
/// The share of the reach of its claims, at the full spread, that a topic
/// adds to its room. A quarter keeps the projection's neighbourhoods,
/// moving the median topic of the development graph 1.2 units, and still
/// gives a busy topic room.
pub const CLAIM_ROOM_SHARE: f32 = 0.25;
/// The most steps of one pass.
pub const MAX_ITERATIONS: usize = 300;
/// The share of an overlap one step removes, split between the two topics.
pub const STEP: f32 = 0.5;
/// A pass has settled when no topic moved further than this in a step.
pub const SETTLED: f32 = 1e-3;
/// The share of its distance beyond rest a child closes on its parent in
/// one step.
pub const PARENT_PULL: f32 = 0.1;
/// The median distance between nearest topics at which claims spread at
/// the full spacing of `layout.rs`: the gap between the topic circle's
/// rings, which was tuned to keep their claims apart.
pub const FULL_SPREAD_SPACING: f32 = 16.0;
/// The least claim spread, so that a dense map still separates ranks.
pub const MIN_SPREAD: f32 = 0.25;
/// How far apart the candidates of [`free_place`]'s spiral are, roughly.
pub const SPIRAL_STEP: f32 = 0.5;
/// How many candidates the spiral tries: out to about 70 units.
pub const SPIRAL_CANDIDATES: usize = 20_000;
/// The angle between successive points of a sunflower spiral, in radians.
pub const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// How far from its topic the claims of a topic with `claims` of them
/// reach, at the claim spread `spread`: its last claim's distance and the
/// widest claim disc. A topic with none reaches nowhere.
pub fn claim_reach(claims: usize, spread: f32) -> f32 {
    if claims == 0 {
        return 0.0;
    }
    let last = (claims - 1) as f32;
    CLAIM_CLEARANCE
        + CLAIM_SPACING * spread * last.sqrt()
        + disc_radius(Kind::Fact).max(disc_radius(Kind::Inference))
}

/// The room a topic with `claims` claims takes on the map.
pub fn room(claims: usize) -> f32 {
    TOPIC_ROOM + CLAIM_ROOM_SHARE * claim_reach(claims, 1.0)
}

/// What a pass did.
#[derive(Clone, Debug, PartialEq)]
pub struct Relaxed {
    pub points: Vec<Vec2>,
    /// The steps taken.
    pub iterations: usize,
    /// Whether it settled before the last step.
    pub settled: bool,
}

/// Relaxes `start`: each point `i` keeps `rooms[i]` clear of every other's
/// room, and a point with a parent in `parents` is pulled toward it.
pub fn relax(start: &[Vec2], rooms: &[f32], parents: &[Option<usize>]) -> Relaxed {
    let n = start.len();
    let mut points = start.to_vec();
    let reach = 2.0 * rooms.iter().copied().fold(0.0_f32, f32::max);
    let has_parents = parents.iter().any(Option::is_some);
    if n < 2 || (reach <= 0.0 && !has_parents) {
        return Relaxed {
            points,
            iterations: 0,
            settled: true,
        };
    }
    let cell = reach.max(1e-3);
    let mut steps = vec![Vec2::ZERO; n];
    for iteration in 0..MAX_ITERATIONS {
        let grid = grid(&points, cell);
        let mut largest = 0.0_f32;
        for i in 0..n {
            let mut step = Vec2::ZERO;
            let (cx, cy) = cell_of(points[i], cell);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(members) = grid.get(&(cx + dx, cy + dy)) else {
                        continue;
                    };
                    for &j in members {
                        if j == i {
                            continue;
                        }
                        let need = rooms[i] + rooms[j];
                        let apart = points[i] - points[j];
                        let distance = apart.length();
                        if distance >= need {
                            continue;
                        }
                        let away = if distance > 1e-6 {
                            apart / distance
                        } else {
                            tie_direction(i, j)
                        };
                        step += away * ((need - distance) * STEP * 0.5);
                    }
                }
            }
            if let Some(parent) = parents
                .get(i)
                .copied()
                .flatten()
                .filter(|&p| p != i && p < n)
            {
                let toward = points[parent] - points[i];
                let distance = toward.length();
                let rest = rooms[i] + rooms[parent];
                if distance > rest {
                    step += toward / distance * ((distance - rest) * PARENT_PULL);
                }
            }
            steps[i] = step;
            largest = largest.max(step.length());
        }
        for (point, step) in points.iter_mut().zip(&steps) {
            *point += *step;
        }
        if largest < SETTLED {
            return Relaxed {
                points,
                iterations: iteration + 1,
                settled: true,
            };
        }
    }
    Relaxed {
        points,
        iterations: MAX_ITERATIONS,
        settled: false,
    }
}

/// The cell of the grid a point is in.
fn cell_of(point: Vec2, cell: f32) -> (i32, i32) {
    (
        (point.x / cell).floor() as i32,
        (point.y / cell).floor() as i32,
    )
}

/// The points in each cell, in index order.
fn grid(points: &[Vec2], cell: f32) -> HashMap<(i32, i32), Vec<usize>> {
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, &point) in points.iter().enumerate() {
        grid.entry(cell_of(point, cell)).or_default().push(i);
    }
    grid
}

/// The way two points in the same place part: a direction from the lower
/// index, which that point takes and the other the opposite of.
fn tie_direction(i: usize, j: usize) -> Vec2 {
    let direction = Vec2::from_angle(GOLDEN_ANGLE * i.min(j) as f32);
    if i < j { direction } else { -direction }
}

/// The nearest clear place to `wanted` for a topic with room `room`, among
/// topics at the places and with the rooms in `others`: `wanted` itself if
/// it is clear, else the first clear point of a sunflower spiral out from
/// it, whose points come at a roughly even spacing and in order of
/// distance. Past the spiral, a place beyond every other topic in the
/// direction of `wanted` from the middle.
pub fn free_place(wanted: Vec2, room: f32, others: &[(Vec2, f32)]) -> Vec2 {
    let clear = |p: Vec2| others.iter().all(|&(q, r)| p.distance(q) >= room + r);
    if clear(wanted) {
        return wanted;
    }
    if let Some(found) = (1..=SPIRAL_CANDIDATES)
        .map(|k| {
            wanted + Vec2::from_angle(GOLDEN_ANGLE * k as f32) * (SPIRAL_STEP * (k as f32).sqrt())
        })
        .find(|&p| clear(p))
    {
        return found;
    }
    let outer = others
        .iter()
        .map(|&(q, r)| q.length() + r)
        .fold(0.0_f32, f32::max);
    let direction = wanted.try_normalize().unwrap_or(Vec2::X);
    direction * (outer + 2.0 * room)
}

/// The median distance from a point to its nearest neighbour, or `None`
/// with fewer than two points.
pub fn median_nearest(points: &[Vec2]) -> Option<f32> {
    if points.len() < 2 {
        return None;
    }
    let mut nearest: Vec<f32> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            points
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, q)| p.distance(*q))
                .fold(f32::INFINITY, f32::min)
        })
        .collect();
    nearest.sort_by(f32::total_cmp);
    Some(nearest[nearest.len() / 2])
}

/// The claim spread for a map whose topics stand `median_nearest` from
/// their nearest neighbours, by the median.
pub fn spread_for(median_nearest: f32) -> f32 {
    if !median_nearest.is_finite() {
        return 1.0;
    }
    (median_nearest / FULL_SPREAD_SPACING).clamp(MIN_SPREAD, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_topics_are_pushed_apart_to_their_rooms() {
        let start = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.5, 0.5),
            // Two in the same place.
            Vec2::new(30.0, 30.0),
            Vec2::new(30.0, 30.0),
        ];
        let rooms = [2.5; 5];
        let relaxed = relax(&start, &rooms, &[None; 5]);
        assert!(relaxed.settled, "{relaxed:?}");
        for i in 0..5 {
            for j in i + 1..5 {
                let d = relaxed.points[i].distance(relaxed.points[j]);
                assert!(d > 5.0 - 0.01, "{i} and {j} are {d} apart");
            }
        }
        // The pair stays about where it was.
        let middle = (relaxed.points[3] + relaxed.points[4]) / 2.0;
        assert!(middle.distance(Vec2::new(30.0, 30.0)) < 1e-3, "{middle}");
    }

    #[test]
    fn topics_with_room_already_are_left_exactly_where_they_are() {
        let start = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, -10.0),
            Vec2::new(100.0, 0.5),
            Vec2::new(101.0, 0.5),
        ];
        let rooms = [2.5, 2.5, 2.5, 2.5, 2.5];
        let relaxed = relax(&start, &rooms, &[None; 5]);
        assert_eq!(&relaxed.points[..3], &start[..3], "clear of every room");
        assert_ne!(relaxed.points[3], start[3], "the crowded pair moves");
        let untouched = relax(&start[..3], &rooms[..3], &[None; 3]);
        assert_eq!(untouched.points, start[..3]);
        assert_eq!(untouched.iterations, 1);
    }

    #[test]
    fn a_bigger_room_keeps_its_neighbours_further_off() {
        let start = [Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0)];
        let relaxed = relax(&start, &[6.0, 2.5, 2.5], &[None; 3]);
        for j in [1, 2] {
            assert!(relaxed.points[0].distance(relaxed.points[j]) > 8.5 - 0.01);
        }
        assert!(room(100) > room(1) && room(1) > room(0) && room(0) == TOPIC_ROOM);
    }

    #[test]
    fn a_child_is_pulled_toward_its_parent_and_only_on_a_graph_with_subtopics() {
        let start = [Vec2::ZERO, Vec2::new(40.0, 0.0), Vec2::new(-40.0, 0.0)];
        let rooms = [2.5; 3];
        let flat = relax(&start, &rooms, &[None; 3]);
        assert_eq!(flat.points, start, "no subtopic_of, so nothing pulls");
        let nested = relax(&start, &rooms, &[None, Some(0), None]);
        let d = nested.points[1].distance(nested.points[0]);
        assert!(d < 6.0, "the child closed in to {d}");
        assert!(d > 5.0 - 0.01, "and keeps its room");
        assert_eq!(nested.points[2], start[2], "a topic with no parent stays");
    }

    #[test]
    fn a_pass_is_the_same_every_time() {
        let mut r = crate::projection::tests::noise(7);
        let start: Vec<Vec2> = (0..300)
            .map(|_| Vec2::new(r() * 40.0, r() * 40.0))
            .collect();
        let rooms: Vec<f32> = (0..300).map(|i| room(i % 13)).collect();
        let parents: Vec<Option<usize>> =
            (0..300).map(|i| (i % 50 == 7).then_some(i / 2)).collect();
        let once = relax(&start, &rooms, &parents);
        assert_eq!(once, relax(&start, &rooms, &parents));
        assert!(once.iterations > 1);
    }

    #[test]
    fn a_new_topic_takes_its_place_if_clear_and_else_the_nearest_clear_one() {
        let others = [(Vec2::ZERO, 2.5), (Vec2::new(20.0, 0.0), 2.5)];
        let clear = Vec2::new(10.0, 0.0);
        assert_eq!(free_place(clear, 2.5, &others), clear);
        let crowded = Vec2::new(1.0, 0.0);
        let found = free_place(crowded, 2.5, &others);
        assert!(found.distance(Vec2::ZERO) >= 5.0, "{found}");
        assert!(
            found.distance(crowded) < 5.0,
            "near where it wanted, {found}"
        );
        assert_eq!(free_place(crowded, 2.5, &others), found, "deterministic");
    }

    #[test]
    fn the_claim_spread_follows_the_median_spacing() {
        let row: Vec<Vec2> = (0..5).map(|i| Vec2::new(i as f32 * 4.0, 0.0)).collect();
        assert_eq!(median_nearest(&row), Some(4.0));
        assert_eq!(median_nearest(&row[..1]), None);
        assert_eq!(spread_for(4.0), 0.25);
        assert_eq!(spread_for(8.0), 0.5);
        assert_eq!(spread_for(40.0), 1.0);
        assert_eq!(spread_for(f32::INFINITY), 1.0);
        assert!(claim_reach(50, 0.5) < claim_reach(50, 1.0));
        assert_eq!(claim_reach(0, 1.0), 0.0);
    }
}
