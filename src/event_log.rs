//! The log (DESIGN.md §10): the acts, advisories and pipeline runs of the
//! session followed, newest first, in place of the side panel's node.
//!
//! `live.rs` hands [`remember`] every `graph_action_recorded`,
//! `advisory_raised` and pipeline event of the session followed as it
//! comes, the replay's among them. [`Log`] keeps an act or an advisory
//! once, by its `action_id`, the [`RECORDS_KEPT`] most recent of them, and
//! a pipeline line for each pipeline event but `tokens_updated`, the
//! [`PIPELINE_KEPT`] most recent, counted apart so that a busy pipeline
//! cannot push the acts out. A `transition_completed` adds its duration to
//! the line its `transition_fired` made, where that line is held. The log
//! keeps the session's entries across switches of graph and starts afresh
//! on a switch of session, since `action_id` is counted per session.
//!
//! What the log lists ([`listed`]) and the panel's lines for it
//! ([`log_lines`]) are pure functions of the log, the graph drawn and the
//! kind filter ([`LogKind`]). The panel draws them in place of the focused
//! node while [`LogOpen`] says so (`panel.rs`), and a subject named in an
//! entry is the panel's node reference, whose click focuses that node.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::graph_index::GraphIndex;
use crate::hub::parse_timestamp;
use crate::panel::{Line, NodeRef, node_label};
use crate::pickers::short_id;
use crate::stream::{Act, Advisory, Envelope, Event, Pipeline};

/// How many acts and advisories the log keeps: as many as the hub's ring.
pub const RECORDS_KEPT: usize = 512;
/// How many pipeline lines the log keeps.
pub const PIPELINE_KEPT: usize = 512;
/// How many entries the panel lists, as it lists at most 200 edges.
pub const MAX_LISTED: usize = 200;
/// How many subjects of one entry the panel names.
pub const MAX_SUBJECTS: usize = 12;

/// One line a pipeline event makes.
#[derive(Clone, Debug, PartialEq)]
pub enum PipelineLine {
    /// A run started, on a net of this many transitions and places.
    Started {
        pipeline: String,
        transitions: usize,
        places: usize,
    },
    /// A transition fired, and once it completed, how long it took.
    Transition {
        pipeline: String,
        transition: String,
        duration_ms: Option<f64>,
    },
    /// A run ended.
    Completed {
        pipeline: String,
        transitions_fired: u64,
        duration_ms: f64,
    },
    /// A run ended by raising, in a transition where one was running.
    Failed {
        pipeline: String,
        transition: Option<String>,
        error: String,
        transitions_fired: u64,
        duration_ms: f64,
    },
}

impl PipelineLine {
    /// The pipeline the line is about.
    pub fn pipeline(&self) -> &str {
        match self {
            PipelineLine::Started { pipeline, .. }
            | PipelineLine::Transition { pipeline, .. }
            | PipelineLine::Completed { pipeline, .. }
            | PipelineLine::Failed { pipeline, .. } => pipeline,
        }
    }
}

/// What an entry is.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Act(Act),
    Advisory(Advisory),
    Pipeline(PipelineLine),
}

/// One entry of the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// When the session made the event, as the hub wrote it.
    pub timestamp: String,
    /// The graph it is about; empty for a pipeline line.
    pub graph: String,
    pub body: Body,
}

/// Where a pipeline line stands: after the act or advisory with this
/// `action_id`, the one held last when it came, or before them all for an
/// empty one; then in the order the lines came.
pub type Position = (String, u64);

/// The log of one session.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Log {
    /// The session whose entries these are.
    pub session: String,
    /// Its acts and advisories, by `action_id`, whose text order is its
    /// numeric order.
    pub records: BTreeMap<String, Entry>,
    /// Its pipeline lines.
    pub pipeline: BTreeMap<Position, Entry>,
    /// How many pipeline lines have come.
    pub lines: u64,
}

/// What the log lists.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LogKind {
    #[default]
    All,
    Acts,
    Advisories,
    Pipelines,
    /// The advisories the agent was asked to raise with a person.
    Notify,
}

/// The kind filter's buttons, in order, with their words.
pub const LOG_KINDS: [(LogKind, &str); 5] = [
    (LogKind::All, "All"),
    (LogKind::Acts, "Acts"),
    (LogKind::Advisories, "Advisories"),
    (LogKind::Pipelines, "Pipelines"),
    (LogKind::Notify, "Notify user"),
];

/// Whether the side panel shows the log rather than the focused node.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogOpen(pub bool);

/// What `J` and the bar's Log button do, from whether the log is open and
/// the panel collapsed: the log shown, in a panel that was collapsed or
/// showing the node, or the panel back on the node.
pub fn toggle_log(open: bool, collapsed: bool) -> (bool, bool) {
    if open && !collapsed {
        (false, false)
    } else {
        (true, false)
    }
}

/// Keeps an act, an advisory or a pipeline line from an event of the
/// session followed; any other event is no entry. An event of another
/// session than the one held starts the log afresh. Whether anything
/// changed.
pub fn remember(log: &mut Log, envelope: &Envelope) -> bool {
    if !matches!(
        envelope.event,
        Event::ActRecorded(_) | Event::AdvisoryRaised(_) | Event::Pipeline(_)
    ) {
        return false;
    }
    let mut changed = false;
    if log.session != envelope.session_id {
        *log = Log {
            session: envelope.session_id.clone(),
            ..Log::default()
        };
        changed = true;
    }
    let entry = |body| Entry {
        timestamp: envelope.timestamp.clone(),
        graph: envelope.graph.clone(),
        body,
    };
    let kept = match &envelope.event {
        Event::ActRecorded(act) => {
            keep_record(log, &act.action_id, entry(Body::Act((**act).clone())))
        }
        Event::AdvisoryRaised(advisory) => keep_record(
            log,
            &advisory.action_id,
            entry(Body::Advisory((**advisory).clone())),
        ),
        Event::Pipeline(event) => match pipeline_line(log, event) {
            Made::Line(line) => keep_line(log, entry(Body::Pipeline(line))),
            Made::Duration => true,
            Made::Nothing => false,
        },
        _ => false,
    };
    changed || kept
}

/// Keeps an act or an advisory, once by its `action_id`, unless the log is
/// full and it is older than every one kept.
fn keep_record(log: &mut Log, id: &str, entry: Entry) -> bool {
    if log.records.contains_key(id) {
        return false;
    }
    let older_than_kept = log.records.len() >= RECORDS_KEPT
        && log
            .records
            .first_key_value()
            .is_some_and(|(oldest, _)| id < oldest.as_str());
    if older_than_kept {
        return false;
    }
    log.records.insert(id.to_owned(), entry);
    while log.records.len() > RECORDS_KEPT {
        log.records.pop_first();
    }
    true
}

/// Keeps a pipeline line after the act or advisory held last.
fn keep_line(log: &mut Log, entry: Entry) -> bool {
    log.lines += 1;
    let after = log
        .records
        .last_key_value()
        .map(|(id, _)| id.clone())
        .unwrap_or_default();
    log.pipeline.insert((after, log.lines), entry);
    while log.pipeline.len() > PIPELINE_KEPT {
        log.pipeline.pop_first();
    }
    true
}

/// What a pipeline event does to the log.
enum Made {
    /// A new line.
    Line(PipelineLine),
    /// A duration given to the line of a transition's fire.
    Duration,
    Nothing,
}

/// The line a pipeline event makes, if it makes one. A completed
/// transition gives its duration to the oldest line of its fire in the
/// pipeline's current run instead, where one is held.
fn pipeline_line(log: &mut Log, event: &Pipeline) -> Made {
    let made = match event {
        Pipeline::Started {
            pipeline_name,
            place_names,
            transition_names,
            ..
        } => Some(PipelineLine::Started {
            pipeline: pipeline_name.clone(),
            transitions: transition_names.len(),
            places: place_names.len(),
        }),
        Pipeline::TransitionFired {
            pipeline_name,
            transition_name,
            ..
        } => Some(PipelineLine::Transition {
            pipeline: pipeline_name.clone(),
            transition: transition_name.clone(),
            duration_ms: None,
        }),
        Pipeline::TransitionCompleted {
            pipeline_name,
            transition_name,
            duration_ms,
            ..
        } => {
            if let Some(position) = open_fire(log, pipeline_name, transition_name)
                && let Some(Entry {
                    body:
                        Body::Pipeline(PipelineLine::Transition {
                            duration_ms: held, ..
                        }),
                    ..
                }) = log.pipeline.get_mut(&position)
            {
                *held = Some(*duration_ms);
                return Made::Duration;
            }
            Some(PipelineLine::Transition {
                pipeline: pipeline_name.clone(),
                transition: transition_name.clone(),
                duration_ms: Some(*duration_ms),
            })
        }
        Pipeline::TokensUpdated { .. } => None,
        Pipeline::Completed {
            pipeline_name,
            transitions_fired,
            duration_ms,
        } => Some(PipelineLine::Completed {
            pipeline: pipeline_name.clone(),
            transitions_fired: *transitions_fired,
            duration_ms: *duration_ms,
        }),
        Pipeline::Failed {
            pipeline_name,
            error,
            transition_name,
            transitions_fired,
            duration_ms,
        } => Some(PipelineLine::Failed {
            pipeline: pipeline_name.clone(),
            transition: transition_name.clone(),
            error: error.clone(),
            transitions_fired: *transitions_fired,
            duration_ms: *duration_ms,
        }),
    };
    made.map_or(Made::Nothing, Made::Line)
}

/// The oldest line of a fire of `transition` still without a duration in
/// `pipeline`'s current run: after the run's start or the last run's end.
fn open_fire(log: &Log, pipeline: &str, transition: &str) -> Option<Position> {
    let mut found = None;
    for (position, entry) in log.pipeline.iter().rev() {
        let Body::Pipeline(line) = &entry.body else {
            continue;
        };
        if line.pipeline() != pipeline {
            continue;
        }
        match line {
            PipelineLine::Transition {
                transition: t,
                duration_ms: None,
                ..
            } if t == transition => found = Some(position.clone()),
            PipelineLine::Transition { .. } => {}
            _ => break,
        }
    }
    found
}

/// Whether an entry passes the kind filter.
pub fn passes(kind: LogKind, entry: &Entry) -> bool {
    match (kind, &entry.body) {
        (LogKind::All, _)
        | (LogKind::Acts, Body::Act(_))
        | (LogKind::Advisories, Body::Advisory(_))
        | (LogKind::Pipelines, Body::Pipeline(_)) => true,
        (LogKind::Notify, Body::Advisory(advisory)) => advisory.notify_user,
        _ => false,
    }
}

/// The entries of the graph drawn and the pipeline lines, newest first:
/// acts and advisories by `action_id`, each pipeline line after the one
/// held when it came. A log of another session than the one drawn lists
/// none.
pub fn listed<'a>(log: &'a Log, session: &str, graph: &str) -> Vec<&'a Entry> {
    if log.session != session {
        return Vec::new();
    }
    let mut entries: Vec<(Position, &Entry)> = log
        .records
        .iter()
        .map(|(id, entry)| ((id.clone(), 0), entry))
        .chain(log.pipeline.iter().map(|(p, entry)| (p.clone(), entry)))
        .filter(|(_, entry)| entry.graph.is_empty() || entry.graph == graph)
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

/// A duration in milliseconds, in the unit that reads best.
pub fn duration_text(ms: f64) -> String {
    match ms {
        ms if ms < 10.0 => format!("{ms:.1} ms"),
        ms if ms < 1000.0 => format!("{ms:.0} ms"),
        ms if ms < 60_000.0 => format!("{:.1} s", ms / 1000.0),
        ms => {
            let seconds = (ms / 1000.0).round() as u64;
            format!("{} min {} s", seconds / 60, seconds % 60)
        }
    }
}

/// "1 transition" or "3 transitions".
fn transitions(n: u64) -> String {
    format!("{n} transition{}", if n == 1 { "" } else { "s" })
}

/// A pipeline line's words.
pub fn pipeline_text(line: &PipelineLine) -> String {
    match line {
        PipelineLine::Started {
            transitions: t,
            places,
            ..
        } => format!("started: {} and {places} places", transitions(*t as u64)),
        PipelineLine::Transition {
            transition,
            duration_ms: None,
            ..
        } => transition.clone(),
        PipelineLine::Transition {
            transition,
            duration_ms: Some(ms),
            ..
        } => format!("{transition}, {}", duration_text(*ms)),
        PipelineLine::Completed {
            transitions_fired,
            duration_ms,
            ..
        } => format!(
            "completed: {} fired in {}",
            transitions(*transitions_fired),
            duration_text(*duration_ms)
        ),
        PipelineLine::Failed {
            transition,
            error,
            transitions_fired,
            duration_ms,
            ..
        } => format!(
            "failed{} after {} fired, in {}: {error}",
            transition
                .as_deref()
                .map_or_else(String::new, |t| format!(" in {t}")),
            transitions(*transitions_fired),
            duration_text(*duration_ms)
        ),
    }
}

/// An entry's kind and words: an act's verb and summary, an advisory's
/// kind and its tool and message, a pipeline's name and its line.
pub fn kind_and_text(body: &Body) -> (String, String) {
    match body {
        Body::Act(act) => (act.verb.clone(), act.summary.clone()),
        Body::Advisory(advisory) => (
            advisory.kind.clone(),
            format!("{}: {}", advisory.tool, advisory.message),
        ),
        Body::Pipeline(line) => (line.pipeline().to_owned(), pipeline_text(line)),
    }
}

/// The fainter row under an entry: an act's counts by their keys, an
/// advisory's action and whether the agent was shown it, and who judged;
/// none for a pipeline line or an act with neither.
pub fn detail_text(body: &Body) -> Option<String> {
    let judged = |judge: &Option<String>| {
        judge
            .as_deref()
            .map(|j| format!("judged by {}", short_id(j)))
    };
    let parts: Vec<String> = match body {
        Body::Act(act) => {
            let counts = act
                .counts
                .iter()
                .map(|(key, n)| format!("{key} {n}"))
                .collect::<Vec<_>>()
                .join(", ");
            [
                (!counts.is_empty()).then_some(counts),
                judged(&act.judged_by),
            ]
            .into_iter()
            .flatten()
            .collect()
        }
        Body::Advisory(advisory) => [
            Some(advisory.action.clone()),
            Some(
                if advisory.surfaced {
                    "shown to the agent"
                } else {
                    "not shown to the agent"
                }
                .to_owned(),
            ),
            judged(&advisory.judged_by),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Body::Pipeline(_) => Vec::new(),
    };
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The ids an entry is about.
fn subjects(body: &Body) -> &[String] {
    match body {
        Body::Act(act) => &act.subjects,
        Body::Advisory(advisory) => &advisory.subjects,
        Body::Pipeline(_) => &[],
    }
}

/// A subject as the panel names a node: by its label, focusing it on a
/// click, faint where it is not visible; or by its short id, faint and
/// not clickable, where the graph drawn does not hold it.
pub fn subject_ref(id: &str, index: &GraphIndex, visible: &[bool]) -> NodeRef {
    match index.by_id.get(id) {
        Some(&node) => NodeRef {
            text: node_label(index, node),
            node: Some(node),
            faint: !visible.get(node).copied().unwrap_or(false),
        },
        None => NodeRef {
            text: short_id(id).to_owned(),
            node: None,
            faint: true,
        },
    }
}

/// The subjects' references, at most [`MAX_SUBJECTS`] and a count of the
/// rest.
pub fn subject_refs(ids: &[String], index: &GraphIndex, visible: &[bool]) -> Vec<NodeRef> {
    let mut refs: Vec<NodeRef> = ids
        .iter()
        .take(MAX_SUBJECTS)
        .map(|id| subject_ref(id, index, visible))
        .collect();
    if ids.len() > MAX_SUBJECTS {
        refs.push(NodeRef {
            text: format!("and {} more", ids.len() - MAX_SUBJECTS),
            node: None,
            faint: true,
        });
    }
    refs
}

/// An instant to the second in UTC, in `format`, or as the hub wrote it
/// where it does not read.
fn when(timestamp: &str, format: &str) -> Option<String> {
    parse_timestamp(timestamp)
        .and_then(chrono::DateTime::from_timestamp_micros)
        .map(|t| t.format(format).to_string())
}

/// What the panel's log reads.
pub struct LogContext<'a> {
    pub log: &'a Log,
    /// The session and graph drawn.
    pub session: &'a str,
    pub graph: &'a str,
    pub kind: LogKind,
    pub index: &'a GraphIndex,
    pub visible: &'a [bool],
}

/// The panel's title and lines for the log: a heading for each day in UTC,
/// and under it each entry that passes, newest first, at most
/// [`MAX_LISTED`]: its row, its detail and its subjects.
pub fn log_lines(context: &LogContext) -> (String, Vec<Line>) {
    let all = listed(context.log, context.session, context.graph);
    let passing: Vec<&Entry> = all
        .iter()
        .copied()
        .filter(|entry| passes(context.kind, entry))
        .collect();
    let title = if passing.len() == all.len() {
        format!("Log: {} entries", all.len())
    } else {
        format!("Log: {} of {} entries", passing.len(), all.len())
    };
    let mut lines = Vec::new();
    if passing.is_empty() {
        lines.push(Line::Note(if all.is_empty() {
            "Nothing yet. The graph's acts and advisories come when the stream connects, and a pipeline's lines as it runs.".to_owned()
        } else {
            "No entry of this kind.".to_owned()
        }));
    }
    let mut day = None;
    for entry in passing.iter().take(MAX_LISTED) {
        let today = when(&entry.timestamp, "%Y-%m-%d");
        if today.is_some() && today != day {
            lines.push(Line::Heading(format!(
                "{}, UTC",
                today.clone().unwrap_or_default()
            )));
            day = today;
        }
        let (kind, text) = kind_and_text(&entry.body);
        lines.push(Line::Entry {
            time: when(&entry.timestamp, "%H:%M:%S").unwrap_or_else(|| entry.timestamp.clone()),
            kind,
            text,
            marked: matches!(&entry.body, Body::Advisory(a) if a.notify_user),
        });
        if let Some(detail) = detail_text(&entry.body) {
            lines.push(Line::Detail(detail));
        }
        let ids = subjects(&entry.body);
        if !ids.is_empty() {
            lines.push(Line::Refs(subject_refs(
                ids,
                context.index,
                context.visible,
            )));
        }
    }
    if passing.len() > MAX_LISTED {
        lines.push(Line::Note(format!(
            "and {} older entries",
            passing.len() - MAX_LISTED
        )));
    }
    (title, lines)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph_index::build;
    use crate::graph_index::tests::node;
    use crate::hub::NodeType;
    use crate::layout::place;

    const SESSION: &str = "s";

    fn envelope(graph: &str, second: u32, event: Event) -> Envelope {
        Envelope {
            event_type: String::new(),
            session_id: SESSION.into(),
            graph: graph.into(),
            timestamp: format!("2026-11-07T10:00:{second:02}.250000Z"),
            event,
        }
    }

    /// An act numbered `n`, about `subjects`.
    pub(crate) fn act(n: u32, graph: &str, subjects: &[&str]) -> Envelope {
        envelope(
            graph,
            n % 60,
            Event::ActRecorded(Box::new(Act {
                action_id: format!("{n:012}"),
                verb: "stored".into(),
                subjects: subjects.iter().map(|&s| s.into()).collect(),
                counts: [("nodes".to_owned(), 1), ("edges".to_owned(), 2)]
                    .into_iter()
                    .collect(),
                summary: format!("act {n}"),
                judged_by: None,
            })),
        )
    }

    /// An advisory numbered `n`.
    pub(crate) fn advisory(n: u32, graph: &str, notify_user: bool) -> Envelope {
        envelope(
            graph,
            n % 60,
            Event::AdvisoryRaised(Box::new(Advisory {
                action_id: format!("{n:012}"),
                tool: "store_decomposition".into(),
                kind: "tag_joined_topic".into(),
                message: format!("advisory {n}"),
                subjects: vec!["a".into()],
                detail: serde_json::Map::new(),
                action: "proceed".into(),
                surfaced: true,
                notify_user,
                judged_by: Some("a9e00000-0000-4000-8000-000000000001".into()),
            })),
        )
    }

    pub(crate) fn pipeline(event: Pipeline) -> Envelope {
        envelope("", 30, Event::Pipeline(Box::new(event)))
    }

    fn started(name: &str) -> Envelope {
        pipeline(Pipeline::Started {
            pipeline_name: name.into(),
            place_names: vec!["start".into(), "after_a".into(), "after_b".into()],
            transition_names: vec!["a".into(), "b".into()],
            edges: Vec::new(),
        })
    }

    fn fired(name: &str, transition: &str) -> Envelope {
        pipeline(Pipeline::TransitionFired {
            pipeline_name: name.into(),
            transition_name: transition.into(),
            input_places: Vec::new(),
        })
    }

    fn completed_transition(name: &str, transition: &str, ms: f64) -> Envelope {
        pipeline(Pipeline::TransitionCompleted {
            pipeline_name: name.into(),
            transition_name: transition.into(),
            output_places: Vec::new(),
            duration_ms: ms,
        })
    }

    fn tokens(name: &str) -> Envelope {
        pipeline(Pipeline::TokensUpdated {
            pipeline_name: name.into(),
            place_token_counts: BTreeMap::new(),
        })
    }

    fn completed(name: &str, fired: u64, ms: f64) -> Envelope {
        pipeline(Pipeline::Completed {
            pipeline_name: name.into(),
            transitions_fired: fired,
            duration_ms: ms,
        })
    }

    fn feed(log: &mut Log, events: &[Envelope]) -> Vec<bool> {
        events.iter().map(|e| remember(log, e)).collect()
    }

    /// The listed entries' kind and words, newest first.
    fn texts(log: &Log, graph: &str) -> Vec<String> {
        listed(log, SESSION, graph)
            .into_iter()
            .map(|entry| {
                let (kind, text) = kind_and_text(&entry.body);
                format!("{kind}: {text}")
            })
            .collect()
    }

    #[test]
    fn entries_are_listed_newest_first_by_action_id_with_pipeline_lines_where_they_came() {
        let mut log = Log::default();
        feed(
            &mut log,
            &[
                started("store"),
                act(2, "g", &["a"]),
                advisory(3, "g", false),
                fired("store", "embed"),
                // The replay's interleaving can bring an older act late; its
                // id puts it in its place.
                act(1, "g", &[]),
                completed("store", 1, 20.0),
                act(4, "g", &[]),
            ],
        );
        assert_eq!(
            texts(&log, "g"),
            [
                "stored: act 4",
                "store: completed: 1 transition fired in 20 ms",
                "store: embed",
                "tag_joined_topic: store_decomposition: advisory 3",
                "stored: act 2",
                "stored: act 1",
                "store: started: 2 transitions and 3 places",
            ]
        );
    }

    #[test]
    fn an_entry_is_kept_once_by_its_action_id_and_another_session_starts_afresh() {
        let mut log = Log::default();
        assert_eq!(
            feed(&mut log, &[act(1, "g", &[]), advisory(2, "g", true)]),
            [true, true]
        );
        // The replay after a reconnect brings both again.
        assert_eq!(
            feed(&mut log, &[act(1, "g", &[]), advisory(2, "g", true)]),
            [false, false]
        );
        assert_eq!(log.records.len(), 2);
        // Events that make no entry change nothing.
        assert!(!remember(
            &mut log,
            &envelope(
                "g",
                1,
                Event::ReflectCounterUpdated(crate::hub::Reflect {
                    count: 1,
                    threshold: 2,
                    suggested: false
                })
            )
        ));
        assert!(!remember(&mut log, &tokens("store")));
        let mut other = act(1, "g", &[]);
        other.session_id = "another".into();
        assert!(remember(&mut log, &other));
        assert_eq!((log.session.as_str(), log.records.len()), ("another", 1));
        assert!(
            listed(&log, SESSION, "g").is_empty(),
            "the log of another session"
        );
    }

    #[test]
    fn the_log_keeps_the_most_recent_records_and_lines_counted_apart() {
        let mut log = Log::default();
        let many: Vec<Envelope> = (1..=600).map(|n| act(n, "g", &[])).collect();
        feed(&mut log, &many);
        assert_eq!(log.records.len(), RECORDS_KEPT);
        assert_eq!(
            log.records.keys().next().map(String::as_str),
            Some("000000000089")
        );
        assert!(
            !remember(&mut log, &act(5, "g", &[])),
            "older than every act kept"
        );
        assert!(remember(&mut log, &advisory(601, "g", false)));
        assert_eq!(
            log.records.keys().next().map(String::as_str),
            Some("000000000090")
        );
        let lines: Vec<Envelope> = (0..700).map(|_| fired("store", "embed")).collect();
        feed(&mut log, &lines);
        assert_eq!(
            (log.pipeline.len(), log.records.len()),
            (PIPELINE_KEPT, RECORDS_KEPT),
            "a busy pipeline pushes out no act"
        );
        assert_eq!(listed(&log, SESSION, "g").len(), 1024);
    }

    #[test]
    fn the_log_lists_the_graph_drawn_and_the_pipeline_lines() {
        let mut log = Log::default();
        feed(
            &mut log,
            &[
                act(1, "g", &[]),
                act(2, "other", &[]),
                fired("reflect", "a"),
            ],
        );
        assert_eq!(texts(&log, "g"), ["reflect: a", "stored: act 1"]);
        assert_eq!(texts(&log, "other"), ["reflect: a", "stored: act 2"]);
    }

    #[test]
    fn the_kind_filter_passes_its_kind() {
        let mut log = Log::default();
        feed(
            &mut log,
            &[
                act(1, "g", &[]),
                advisory(2, "g", false),
                advisory(3, "g", true),
                started("reflect"),
            ],
        );
        let count = |kind| {
            listed(&log, SESSION, "g")
                .into_iter()
                .filter(|e| passes(kind, e))
                .count()
        };
        assert_eq!(
            LOG_KINDS.map(|(kind, _)| count(kind)),
            [4, 1, 2, 1, 1],
            "all, acts, advisories, pipelines, notify user"
        );
    }

    #[test]
    fn a_run_makes_a_line_for_its_start_each_transition_and_its_end() {
        let mut log = Log::default();
        let changed = feed(
            &mut log,
            &[
                started("reflect"),
                fired("reflect", "a"),
                completed_transition("reflect", "a", 340.0),
                tokens("reflect"),
                fired("reflect", "b"),
                completed_transition("reflect", "b", 4.4),
                completed("reflect", 2, 1530.0),
            ],
        );
        assert_eq!(changed, [true, true, true, false, true, true, true]);
        assert_eq!(
            texts(&log, "g"),
            [
                "reflect: completed: 2 transitions fired in 1.5 s",
                "reflect: b, 4.4 ms",
                "reflect: a, 340 ms",
                "reflect: started: 2 transitions and 3 places",
            ]
        );
    }

    #[test]
    fn a_run_joined_midway_shows_from_its_next_event() {
        let mut log = Log::default();
        // The client connects while "b" is in flight: its fire, and the
        // start before it, were never seen.
        feed(
            &mut log,
            &[
                tokens("store"),
                completed_transition("store", "b", 12.0),
                fired("store", "c"),
                pipeline(Pipeline::Failed {
                    pipeline_name: "store".into(),
                    error: "the embedder did not answer".into(),
                    transition_name: Some("c".into()),
                    transitions_fired: 2,
                    duration_ms: 90_500.0,
                }),
            ],
        );
        assert_eq!(
            texts(&log, "g"),
            [
                "store: failed in c after 2 transitions fired, in 1 min 31 s: the embedder did not answer",
                "store: c",
                "store: b, 12 ms",
            ]
        );
        // The next run's completion of "c" names a fire of its own run,
        // not the one the failure left without a duration.
        feed(
            &mut log,
            &[
                started("store"),
                fired("store", "c"),
                completed_transition("store", "c", 1.0),
            ],
        );
        assert_eq!(
            texts(&log, "g")[..3],
            [
                "store: c, 1.0 ms",
                "store: started: 2 transitions and 3 places",
                "store: failed in c after 2 transitions fired, in 1 min 31 s: the embedder did not answer",
            ]
        );
        assert!(texts(&log, "g").contains(&"store: c".to_owned()));
    }

    #[test]
    fn two_pipelines_running_at_once_keep_their_own_transitions() {
        let mut log = Log::default();
        feed(
            &mut log,
            &[
                fired("retrieval", "x"),
                fired("store", "x"),
                completed_transition("store", "x", 5.0),
                completed_transition("retrieval", "x", 7.0),
            ],
        );
        assert_eq!(
            texts(&log, "g"),
            ["store: x, 5.0 ms", "retrieval: x, 7.0 ms"]
        );
    }

    #[test]
    fn j_shows_the_log_and_then_returns_to_the_node() {
        assert_eq!(toggle_log(false, false), (true, false));
        assert_eq!(toggle_log(true, false), (false, false));
        assert_eq!(
            toggle_log(true, true),
            (true, false),
            "a collapsed panel opens on the log"
        );
        assert_eq!(toggle_log(false, true), (true, false));
    }

    #[test]
    fn the_panel_lists_each_entry_with_its_detail_and_subjects() {
        let nodes = vec![node("a", NodeType::Topic, 1), node("b", NodeType::Fact, 2)];
        let index = build(&nodes, &[], &place(&nodes, &[]));
        let mut log = Log::default();
        feed(
            &mut log,
            &[
                act(1, "g", &["a", "b", "gone"]),
                advisory(2, "g", true),
                fired("store", "embed"),
            ],
        );
        let mut visible = vec![false; 2];
        visible[index.by_id["a"]] = true;
        let context = LogContext {
            log: &log,
            session: SESSION,
            graph: "g",
            kind: LogKind::All,
            index: &index,
            visible: &visible,
        };
        let (title, lines) = log_lines(&context);
        assert_eq!(title, "Log: 3 entries");
        let a = NodeRef {
            text: node_label(&index, index.by_id["a"]),
            node: Some(index.by_id["a"]),
            faint: false,
        };
        assert_eq!(
            lines,
            [
                Line::Heading("2026-11-07, UTC".into()),
                Line::Entry {
                    time: "10:00:30".into(),
                    kind: "store".into(),
                    text: "embed".into(),
                    marked: false
                },
                Line::Entry {
                    time: "10:00:02".into(),
                    kind: "tag_joined_topic".into(),
                    text: "store_decomposition: advisory 2".into(),
                    marked: true
                },
                Line::Detail("proceed; shown to the agent; judged by a9e00000".into()),
                Line::Refs(vec![a.clone()]),
                Line::Entry {
                    time: "10:00:01".into(),
                    kind: "stored".into(),
                    text: "act 1".into(),
                    marked: false
                },
                Line::Detail("edges 2, nodes 1".into()),
                Line::Refs(vec![
                    a,
                    NodeRef {
                        text: node_label(&index, index.by_id["b"]),
                        node: Some(index.by_id["b"]),
                        faint: true,
                    },
                    NodeRef {
                        text: "gone".into(),
                        node: None,
                        faint: true,
                    },
                ]),
            ]
        );
        let notify = LogContext {
            kind: LogKind::Notify,
            ..context
        };
        let (title, lines) = log_lines(&notify);
        assert_eq!(title, "Log: 1 of 3 entries");
        assert_eq!(lines.len(), 4);
        let empty = Log::default();
        let (title, lines) = log_lines(&LogContext {
            log: &empty,
            ..notify
        });
        assert_eq!(title, "Log: 0 entries");
        assert!(matches!(&lines[..], [Line::Note(n)] if n.starts_with("Nothing yet")));
    }

    #[test]
    fn many_subjects_are_named_up_to_a_limit_and_many_entries_listed_up_to_one() {
        let index = GraphIndex::default();
        let ids: Vec<String> = (0..15).map(|n| format!("id-{n}")).collect();
        let refs = subject_refs(&ids, &index, &[]);
        assert_eq!(refs.len(), MAX_SUBJECTS + 1);
        assert_eq!(refs[MAX_SUBJECTS].text, "and 3 more");
        let mut log = Log::default();
        let many: Vec<Envelope> = (1..=230).map(|n| act(n, "g", &[])).collect();
        feed(&mut log, &many);
        let (_, lines) = log_lines(&LogContext {
            log: &log,
            session: SESSION,
            graph: "g",
            kind: LogKind::Acts,
            index: &index,
            visible: &[],
        });
        assert_eq!(
            lines
                .iter()
                .filter(|l| matches!(l, Line::Entry { .. }))
                .count(),
            MAX_LISTED
        );
        assert_eq!(
            lines.last(),
            Some(&Line::Note("and 30 older entries".into()))
        );
    }

    #[test]
    fn durations_read_in_the_unit_that_suits_them() {
        assert_eq!(duration_text(0.4), "0.4 ms");
        assert_eq!(duration_text(148.25), "148 ms");
        assert_eq!(duration_text(1520.5), "1.5 s");
        assert_eq!(duration_text(125_000.0), "2 min 5 s");
    }
}
