//! The topic map (DESIGN.md §7) as a value, and the decisions about it,
//! all pure: what is saved, how a map is fitted and extended, which model
//! it reads, and when it is fitted again. `relayout.rs` does them to the
//! running scene and `persist.rs` saves them.
//!
//! A [`TopicMap`] is one graph's map: the fitted [`Projection`], with the
//! embedding model it reads; each topic's place by id; the claim spread
//! and the outer radius the fit left (`relax.rs`); and how many topics the
//! fit placed, against which the topics put on it since are counted. It is
//! frozen once fitted: only [`extend_map`] adds to it, and only
//! [`fit_map`] makes a new one.
//!
//! A fit ([`fit_map`]) projects every topic with an embedding by PCA,
//! scaled to the size of the topic circle for the graph's topics
//! (`layout.rs`), relaxes the projected points, and takes the claim spread
//! from the result. When the embeddings arrive, [`plan`] says what to do
//! with them: fit a new map when there is none, when it is asked for, or
//! when the map's model is not the model the hub now reports, since a
//! changed embedder changes every vector; otherwise put on the map the
//! topics it does not know. [`choose_model`] keeps the map's model while
//! the hub reports vectors under it.
//!
//! A topic stored live has its embedding stored after it, so the client
//! waits for a burst of new topics to end before it asks: [`Debounce`]
//! holds the moment the fetch is due, two seconds after the latest new
//! topic.

use std::collections::{BTreeMap, HashMap};

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};

use crate::hub::{Embeddings, NodeType};
use crate::layout::MapPlaces;
use crate::projection::{self, Projection};
use crate::relax;

/// The version of the saved map's shape.
pub const FORMAT: u32 = 1;

/// How long after the latest new topic of a burst the embeddings are read.
pub const DEBOUNCE_SECONDS: f64 = 2.0;

/// When the topics put on the map since the fit pass this share of those
/// fitted, and at least [`DRIFT_LEAST`] of them, the bar suggests
/// Recalculate (DESIGN.md §15).
pub const DRIFT_SHARE: f32 = 0.25;
pub const DRIFT_LEAST: usize = 10;

/// One graph's topic map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopicMap {
    pub format: u32,
    pub graph: String,
    pub projection: Projection,
    /// The claim spread around a topic on this map (`relax.rs`).
    pub spread: f32,
    /// The map's outer radius as fitted, its claims' reach included: the
    /// ring of topics it does not name and the unfiled region stand beyond.
    pub radius: f32,
    /// How many topics the fit placed.
    pub fitted: usize,
    /// Each topic's place on the map by its id: x and z.
    pub topics: BTreeMap<String, [f32; 2]>,
}

impl TopicMap {
    /// The model the map reads.
    pub fn model(&self) -> &str {
        &self.projection.model_id
    }

    /// How many topics were put on the map after the fit.
    pub fn placed_since(&self) -> usize {
        self.topics.len().saturating_sub(self.fitted)
    }

    /// A topic's place on the map.
    pub fn place(&self, id: &str) -> Option<Vec2> {
        self.topics.get(id).map(|&[x, z]| Vec2::new(x, z))
    }
}

/// The map's places, as the layout reads them.
pub fn places(map: &TopicMap) -> MapPlaces<'_> {
    MapPlaces {
        topics: &map.topics,
        spread: map.spread,
        radius: map.radius,
    }
}

/// Whether enough topics were put on the map since the fit that the map is
/// worth recalculating.
pub fn drifted(map: &TopicMap) -> bool {
    let since = map.placed_since();
    since >= DRIFT_LEAST && since as f32 > DRIFT_SHARE * map.fitted as f32
}

/// One topic as a fit reads it.
#[derive(Clone, Copy, Debug)]
pub struct TopicInput<'a> {
    pub id: &'a str,
    pub vector: &'a [f32],
    /// How many claims stand beside it.
    pub claims: usize,
    /// Its parent by `subtopic_of`, by its place among the inputs.
    pub parent: Option<usize>,
}

/// A fitted map, and how its relaxation went.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub map: TopicMap,
    /// Each input's place, in input order.
    pub points: Vec<Vec2>,
    pub iterations: usize,
    pub settled: bool,
    /// The median distance between nearest topics, which set the spread.
    pub median_nearest: Option<f32>,
}

/// Fits a map of `graph` to the topics, all embedded under `model`, scaled
/// so that the bulk of them stand within `radius`: projected by PCA,
/// relaxed, and with the claim spread the relaxed map leaves room for.
pub fn fit_map(
    graph: &str,
    model: &str,
    topics: &[TopicInput],
    radius: f32,
) -> Result<Fitted, String> {
    let vectors: Vec<&[f32]> = topics.iter().map(|t| t.vector).collect();
    let projection = projection::fit(model, &vectors, radius)?;
    let projected: Vec<Vec2> = topics
        .iter()
        .map(|t| projection::project(&projection, t.vector).unwrap_or_default())
        .collect();
    let rooms: Vec<f32> = topics.iter().map(|t| relax::room(t.claims)).collect();
    let parents: Vec<Option<usize>> = topics
        .iter()
        .map(|t| t.parent.filter(|&p| p < topics.len()))
        .collect();
    let relaxed = relax::relax(&projected, &rooms, &parents);
    let median_nearest = relax::median_nearest(&relaxed.points);
    let spread = median_nearest.map_or(1.0, relax::spread_for);
    let outer = topics
        .iter()
        .zip(&relaxed.points)
        .map(|(t, p)| p.length() + relax::claim_reach(t.claims, spread))
        .fold(0.0_f32, f32::max);
    let map = TopicMap {
        format: FORMAT,
        graph: graph.to_owned(),
        projection,
        spread,
        radius: outer,
        fitted: topics.len(),
        topics: topics
            .iter()
            .zip(&relaxed.points)
            .map(|(t, p)| (t.id.to_owned(), [p.x, p.y]))
            .collect(),
    };
    Ok(Fitted {
        map,
        points: relaxed.points,
        iterations: relaxed.iterations,
        settled: relaxed.settled,
        median_nearest,
    })
}

/// A topic the map does not know, to be put on it.
#[derive(Clone, Copy, Debug)]
pub struct Newcomer<'a> {
    pub id: &'a str,
    pub vector: &'a [f32],
    pub claims: usize,
}

/// Puts the newcomers on the map, in order: each projected through the
/// map's projection and put in free space among `others`, the places and
/// rooms of the topics drawn, and the newcomers placed before it. Nothing
/// already on the map moves. Gives each newcomer's place, or `None` for one
/// whose vector the projection cannot take.
pub fn extend_map(
    map: &mut TopicMap,
    newcomers: &[Newcomer],
    others: &[(Vec2, f32)],
) -> Vec<Option<Vec2>> {
    let mut taken = others.to_vec();
    newcomers
        .iter()
        .map(|newcomer| {
            let wanted = projection::project(&map.projection, newcomer.vector)?;
            let room = relax::room(newcomer.claims);
            let place = relax::free_place(wanted, room, &taken);
            taken.push((place, room));
            map.topics
                .insert(newcomer.id.to_owned(), [place.x, place.y]);
            Some(place)
        })
        .collect()
}

/// Why the embeddings are read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// The graph was drawn: put on the map the topics it does not know, or
    /// fit one where there is none.
    Start,
    /// New topics arrived after the map was made.
    Newcomers,
    /// A new map is asked for, for this reason.
    Recalculate(String),
}

/// What to do with the embeddings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Fit a new map, for this reason.
    Fit(String),
    /// Put on the map the topics it does not know.
    Extend,
}

/// What to do with embeddings under `model`, of `dimension`, given the map
/// drawn and why they were read.
pub fn plan(map: Option<&TopicMap>, purpose: &Purpose, model: &str, dimension: usize) -> Plan {
    match (map, purpose) {
        (_, Purpose::Recalculate(why)) => Plan::Fit(why.clone()),
        (None, _) => Plan::Fit("there was no topic map for this graph".to_owned()),
        (Some(map), _) if map.model() != model => Plan::Fit(format!(
            "the embedder changed from {} to {model}, which changes every vector",
            map.model()
        )),
        (Some(map), _) if map.projection.dimension() != dimension => Plan::Fit(format!(
            "the vectors of {model} are {dimension} long now, not {}",
            map.projection.dimension()
        )),
        (Some(_), _) => Plan::Extend,
    }
}

/// The model to read, and its vectors' length: the map's own while the hub
/// reports topic vectors under it, else the model with the most topic
/// vectors, the first by name on a tie. `None` when no topic has a vector.
pub fn choose_model(answer: &Embeddings, wanted: Option<&str>) -> Option<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in &answer.embeddings {
        if entry.node_type == NodeType::Topic && !entry.vector.is_empty() {
            *counts.entry(entry.model_id.as_str()).or_default() += 1;
        }
    }
    let model = wanted.filter(|m| counts.contains_key(m)).or_else(|| {
        counts
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(&m, _)| m)
    })?;
    let dimension = answer.models.get(model).copied().or_else(|| {
        answer
            .embeddings
            .iter()
            .find(|e| e.model_id == model)
            .map(|e| e.vector.len())
    })?;
    Some((model.to_owned(), dimension))
}

/// Each topic's vector under `model` of `dimension`, by id: the first the
/// answer lists for it.
pub fn topic_vectors<'a>(
    answer: &'a Embeddings,
    model: &str,
    dimension: usize,
) -> HashMap<&'a str, &'a [f32]> {
    let mut vectors = HashMap::new();
    for entry in &answer.embeddings {
        if entry.node_type == NodeType::Topic
            && entry.model_id == model
            && entry.vector.len() == dimension
        {
            vectors
                .entry(entry.node_id.as_str())
                .or_insert(entry.vector.as_slice());
        }
    }
    vectors
}

/// When the embeddings are next due to be read for new topics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Debounce {
    /// The moment, in seconds on the app's clock, or `None` when nothing
    /// waits.
    pub due: Option<f64>,
}

/// The debounce after a new topic at `now`: due [`DEBOUNCE_SECONDS`] after
/// it, whatever was due before, so a burst is read once, after its last.
pub fn note_topic(now: f64) -> Debounce {
    Debounce {
        due: Some(now + DEBOUNCE_SECONDS),
    }
}

/// Whether the read is due at `now`, and the debounce after taking it.
pub fn take_due(debounce: Debounce, now: f64) -> (Debounce, bool) {
    match debounce.due {
        Some(due) if due <= now => (Debounce { due: None }, true),
        _ => (debounce, false),
    }
}

/// Where a topic stands on the map, in words for the side panel.
pub fn topic_note(
    map: Option<&TopicMap>,
    id: &str,
    missing: bool,
    reading: bool,
    unavailable: Option<&str>,
) -> String {
    match map {
        Some(map) if map.topics.contains_key(id) => {
            format!("placed by its embedding under {}", map.model())
        }
        _ if missing => "on the ring: the hub has no embedding for it".to_owned(),
        _ if reading => "on the ring until its embedding is read".to_owned(),
        Some(_) => {
            "on the ring until its embedding is read, after the next new topic or Recalculate"
                .to_owned()
        }
        None => match unavailable {
            Some(why) => format!("on the circle: there is no topic map, as {why}"),
            None => "on the circle until the topic map is fitted".to_owned(),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hub::EmbeddingView;
    use crate::projection::tests::noise;

    /// Vectors of `dimension` for `count` topics in three loose groups.
    pub(crate) fn grouped(count: usize, dimension: usize) -> Vec<(String, Vec<f32>)> {
        let mut r = noise(11);
        let centres: Vec<Vec<f32>> = (0..3)
            .map(|_| (0..dimension).map(|_| r()).collect())
            .collect();
        (0..count)
            .map(|i| {
                let centre = &centres[i % 3];
                let v = centre.iter().map(|c| c + 0.1 * r()).collect();
                (format!("t{i}"), v)
            })
            .collect()
    }

    fn inputs(topics: &[(String, Vec<f32>)]) -> Vec<TopicInput<'_>> {
        topics
            .iter()
            .enumerate()
            .map(|(i, (id, v))| TopicInput {
                id,
                vector: v,
                claims: i % 5,
                parent: None,
            })
            .collect()
    }

    pub(crate) fn answer(model: &str, topics: &[(String, Vec<f32>)]) -> Embeddings {
        Embeddings {
            schema_version: Some(1),
            graph: "g".into(),
            node_type: Some("topic".into()),
            models: [(model.to_owned(), topics.first().map_or(0, |t| t.1.len()))].into(),
            embeddings: topics
                .iter()
                .map(|(id, v)| EmbeddingView {
                    node_id: id.clone(),
                    node_type: NodeType::Topic,
                    model_id: model.to_owned(),
                    vector: v.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_fitted_map_names_every_topic_keeps_them_apart_and_is_the_same_every_time() {
        let topics = grouped(60, 16);
        let fitted = fit_map("g", "m", &inputs(&topics), 50.0).unwrap();
        assert_eq!(fitted.map.topics.len(), 60);
        assert_eq!(fitted.map.fitted, 60);
        assert_eq!(fitted.map.model(), "m");
        assert!(fitted.settled);
        let points: Vec<Vec2> = topics
            .iter()
            .map(|(id, _)| fitted.map.place(id).unwrap())
            .collect();
        for i in 0..60 {
            for j in i + 1..60 {
                let need = relax::room(i % 5) + relax::room(j % 5);
                assert!(points[i].distance(points[j]) > need - 0.01);
            }
        }
        let outer = points.iter().map(|p| p.length()).fold(0.0, f32::max);
        assert!(fitted.map.radius >= outer);
        assert!(fitted.map.spread >= relax::MIN_SPREAD && fitted.map.spread <= 1.0);
        assert_eq!(fit_map("g", "m", &inputs(&topics), 50.0).unwrap(), fitted);
        let text = serde_json::to_string(&fitted.map).unwrap();
        assert_eq!(serde_json::from_str::<TopicMap>(&text).unwrap(), fitted.map);
    }

    #[test]
    fn a_newcomer_lands_near_its_kind_and_moves_nothing() {
        let topics = grouped(61, 16);
        let (fitted_topics, newcomer) = topics.split_at(60);
        let mut map = fit_map("g", "m", &inputs(fitted_topics), 50.0).unwrap().map;
        let before = map.topics.clone();
        let others: Vec<(Vec2, f32)> = fitted_topics
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (map.place(id).unwrap(), relax::room(i % 5)))
            .collect();
        let placed = extend_map(
            &mut map,
            &[Newcomer {
                id: &newcomer[0].0,
                vector: &newcomer[0].1,
                claims: 0,
            }],
            &others,
        );
        let place = placed[0].unwrap();
        for (id, at) in &before {
            assert_eq!(map.topics[id], *at, "{id} did not move");
        }
        assert_eq!(map.place("t60"), Some(place));
        assert_eq!(map.placed_since(), 1);
        for &(q, r) in &others {
            assert!(place.distance(q) >= r + relax::room(0) - 1e-4);
        }
        // t60 is in the group of t0, t3, ...: its nearest fitted topic is
        // one of them.
        let nearest = fitted_topics
            .iter()
            .min_by(|a, b| {
                let d = |id: &str| map.place(id).unwrap().distance(place);
                d(&a.0).total_cmp(&d(&b.0))
            })
            .unwrap();
        let index: usize = nearest.0[1..].parse().unwrap();
        assert_eq!(index % 3, 60 % 3, "landed beside {}", nearest.0);
        let wrong = extend_map(
            &mut map,
            &[Newcomer {
                id: "short",
                vector: &[1.0],
                claims: 0,
            }],
            &others,
        );
        assert_eq!(wrong, [None]);
        assert!(!map.topics.contains_key("short"));
    }

    #[test]
    fn the_saved_map_wins_unless_its_model_changed_or_a_new_one_is_asked_for() {
        let topics = grouped(20, 8);
        let map = fit_map("g", "m", &inputs(&topics), 50.0).unwrap().map;
        assert_eq!(plan(Some(&map), &Purpose::Start, "m", 8), Plan::Extend);
        assert_eq!(plan(Some(&map), &Purpose::Newcomers, "m", 8), Plan::Extend);
        assert!(matches!(plan(None, &Purpose::Start, "m", 8), Plan::Fit(_)));
        match plan(Some(&map), &Purpose::Start, "other", 8) {
            Plan::Fit(why) => assert!(why.contains("from m to other"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            plan(Some(&map), &Purpose::Start, "m", 9),
            Plan::Fit(_)
        ));
        assert_eq!(
            plan(Some(&map), &Purpose::Recalculate("asked".into()), "m", 8),
            Plan::Fit("asked".into())
        );
    }

    #[test]
    fn the_maps_model_is_kept_while_the_hub_reports_it() {
        let topics = grouped(4, 3);
        let mut both = answer("small", &topics);
        let more = answer("large", &grouped(6, 5));
        both.models.extend(more.models.clone());
        both.embeddings.extend(more.embeddings.clone());
        assert_eq!(
            choose_model(&both, Some("small")),
            Some(("small".into(), 3))
        );
        assert_eq!(
            choose_model(&both, None),
            Some(("large".into(), 5)),
            "the most vectors"
        );
        assert_eq!(choose_model(&both, Some("gone")), Some(("large".into(), 5)));
        let vectors = topic_vectors(&both, "small", 3);
        assert_eq!(vectors.len(), 4);
        assert_eq!(vectors["t1"], topics[1].1.as_slice());
        let empty = answer("m", &[]);
        assert_eq!(choose_model(&empty, None), None);
    }

    #[test]
    fn the_read_is_due_two_seconds_after_the_last_new_topic_of_a_burst() {
        let mut debounce = Debounce::default();
        assert_eq!(
            take_due(debounce, 100.0),
            (debounce, false),
            "nothing waits"
        );
        for now in [10.0, 11.0, 11.5] {
            debounce = note_topic(now);
        }
        assert_eq!(debounce.due, Some(13.5));
        assert!(!take_due(debounce, 13.4).1);
        let (after, due) = take_due(debounce, 13.5);
        assert!(due);
        assert_eq!(after.due, None);
        assert!(!take_due(after, 20.0).1, "once");
    }

    #[test]
    fn enough_topics_put_on_since_the_fit_suggest_a_recalculation() {
        let topics = grouped(40, 8);
        let mut map = fit_map("g", "m", &inputs(&topics), 50.0).unwrap().map;
        assert!(!drifted(&map));
        for k in 0..11 {
            map.topics.insert(format!("new{k}"), [0.0, 0.0]);
        }
        assert!(drifted(&map), "11 of 40 is more than a quarter");
        assert!(topic_note(Some(&map), "t1", false, false, None).contains("embedding under m"));
        assert!(topic_note(Some(&map), "x", true, false, None).contains("no embedding"));
        assert!(
            topic_note(None, "x", false, false, Some("the hub is old")).contains("the hub is old")
        );
    }
}
