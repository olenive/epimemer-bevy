//! Retrieval focus (DESIGN.md §10): the retrieval records of the session
//! followed, kept for the bar's Retrieval picker, and the filter criterion a
//! chosen record sets.
//!
//! The records come from two places. `GET /api/retrievals` is read in the
//! background for the session drawn when the first graph is drawn and again
//! after every switch of graph or session ([`RetrievalRead`]), since the
//! stream follows only the graph drawn and the route answers for every
//! graph. The stream's `retrieval_recorded` events, the replay's among them,
//! are handed to [`remember`] by `live.rs` as they come. [`Retrievals`]
//! keeps each record once, by its `record_id`, and the [`KEEP`] most recent
//! of the session followed, and forgets them on a switch to another session,
//! since ids are counted per session.
//!
//! What the picker lists ([`entries`]), the words on its button
//! ([`button_label`]) and the side panel's line ([`panel_line`]) are pure
//! functions of the store and the graph drawn. Choosing a record copies its
//! ids into the [`Filter`] ([`focus_of`]), so the filter decides without
//! the store and the choice holds after the record is no longer kept.
//! `--retrieval` chooses a record from the start, once the route has
//! answered ([`chosen`]).

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;

use crate::background::{Pending, Poll, in_background, poll};
use crate::filter::{Filter, RetrievalFocus};
use crate::graph_index::GraphIndex;
use crate::hub::{self, RetrievalRecord, parse_timestamp};
use crate::pickers::short_id;
use crate::reload::Catalogue;

/// How many records the client keeps, the most recent by `record_id`.
pub const KEEP: usize = 50;

/// What `--retrieval` takes for the newest record of the graph drawn.
pub const LATEST: &str = "latest";

/// The retrieval records of one session, by `record_id`, whose text order
/// is its numeric order (VIZ_CONTRACT.md §4).
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Retrievals {
    /// The session whose records these are.
    pub session: String,
    pub records: BTreeMap<String, RetrievalRecord>,
}

/// A read of the route, as it comes back.
pub type Answer = Result<Vec<RetrievalRecord>, String>;

/// The read of `GET /api/retrievals`: what it was last started for, the
/// read under way, why the last one failed, and the record `--retrieval`
/// asks for.
#[derive(Resource, Default)]
pub struct RetrievalRead {
    /// The session and graph the last read was started for.
    pub read_for: Option<(String, String)>,
    /// The read under way, and the session it asks.
    pub pending: Option<(String, Pending<Answer>)>,
    /// Why the last read failed.
    pub error: Option<String>,
    /// A record's number, or [`LATEST`], to choose once the route answers.
    pub wanted: Option<String>,
}

/// One row of the Retrieval picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub record_id: String,
    pub label: String,
}

/// Keeps the records of `session`, each once by its `record_id`, the
/// [`KEEP`] most recent of them; a session other than the one held starts
/// the store afresh. A record already held stays as it is, except that a
/// copy with its query and response text replaces one without, which is
/// what the stream sends on a hub bound beyond loopback. Whether anything
/// changed.
pub fn remember(
    store: &mut Retrievals,
    session: &str,
    incoming: impl IntoIterator<Item = RetrievalRecord>,
) -> bool {
    let mut changed = false;
    if store.session != session {
        store.session = session.to_owned();
        store.records.clear();
        changed = true;
    }
    for record in incoming {
        let held = store.records.get(&record.record_id);
        if held.is_some_and(|held| has_text(held) || !has_text(&record)) {
            continue;
        }
        let older_than_kept = store.records.len() >= KEEP
            && store
                .records
                .first_key_value()
                .is_some_and(|(oldest, _)| record.record_id < *oldest);
        if held.is_none() && older_than_kept {
            continue;
        }
        store.records.insert(record.record_id.clone(), record);
        changed = true;
    }
    while store.records.len() > KEEP {
        store.records.pop_first();
    }
    changed
}

/// Whether a copy carries the text the tool's call and answer are shown by.
fn has_text(record: &RetrievalRecord) -> bool {
    !record.query.is_empty() || !record.response_text.is_empty()
}

/// The ids a record retrieved, each once; `None` where it declares none.
pub fn retrieved_ids(record: &RetrievalRecord) -> Option<BTreeSet<String>> {
    record
        .retrieved
        .as_ref()
        .map(|nodes| nodes.iter().map(|n| n.node_id.clone()).collect())
}

/// What the filter keeps of a record chosen.
pub fn focus_of(record: &RetrievalRecord) -> RetrievalFocus {
    RetrievalFocus {
        record_id: record.record_id.clone(),
        retrieved: retrieved_ids(record),
    }
}

/// The tool's name without the prefix every Epimemer tool has.
pub fn short_tool(tool: &str) -> &str {
    tool.strip_prefix("epimemer.").unwrap_or(tool)
}

/// A record's number, its id without the padding.
pub fn record_number(record_id: &str) -> &str {
    match record_id.trim_start_matches('0') {
        "" if !record_id.is_empty() => "0",
        number => number,
    }
}

/// When a tool answered, to the second in UTC, in `format`, or as the hub
/// wrote it where it does not read.
fn when(at: &str, format: &str) -> String {
    parse_timestamp(at)
        .and_then(chrono::DateTime::from_timestamp_micros)
        .map_or_else(|| at.to_owned(), |t| t.format(format).to_string())
}

/// How many nodes a record retrieved, or that it declares none.
pub fn count_text(record: &RetrievalRecord) -> String {
    retrieved_ids(record).map_or_else(
        || "not declared".to_owned(),
        |ids| format!("{} retrieved", ids.len()),
    )
}

/// A picker row's words: the tool, when it answered, and how many nodes it
/// retrieved.
pub fn entry_label(record: &RetrievalRecord) -> String {
    format!(
        "{}  {}  {}",
        short_tool(&record.tool),
        when(&record.at, "%Y-%m-%d %H:%M:%S UTC"),
        count_text(record)
    )
}

/// The records the picker lists: those of the graph drawn, newest first.
/// The store of another session than the one drawn lists none.
pub fn entries(store: &Retrievals, session: &str, graph: &str) -> Vec<Entry> {
    if store.session != session {
        return Vec::new();
    }
    store
        .records
        .values()
        .rev()
        .filter(|record| record.graph == graph)
        .map(|record| Entry {
            record_id: record.record_id.clone(),
            label: entry_label(record),
        })
        .collect()
}

/// The picker's button: the record chosen by its tool and time while it is
/// kept, by its number after, or any.
pub fn button_label(focus: Option<&RetrievalFocus>, store: &Retrievals) -> String {
    let Some(focus) = focus else {
        return "any".to_owned();
    };
    store.records.get(&focus.record_id).map_or_else(
        || format!("record {}", record_number(&focus.record_id)),
        |record| {
            format!(
                "{} {}",
                short_tool(&record.tool),
                when(&record.at, "%H:%M:%S")
            )
        },
    )
}

/// How many of `ids` name no active node of the graph drawn.
pub fn absent_count(ids: &BTreeSet<String>, index: &GraphIndex) -> usize {
    ids.iter()
        .filter(|id| {
            !index
                .by_id
                .get(id.as_str())
                .is_some_and(|&i| index.active.get(i).copied().unwrap_or(false))
        })
        .count()
}

/// The side panel's line for the record chosen: how many of its ids the
/// graph drawn does not hold, or that it declares none.
pub fn panel_line(focus: &RetrievalFocus, index: &GraphIndex) -> String {
    let number = record_number(&focus.record_id);
    match &focus.retrieved {
        None => format!("record {number} declares no nodes, so every node passes"),
        Some(ids) => format!(
            "record {number}: {} of its {} retrieved ids are absent from the graph drawn",
            absent_count(ids, index),
            ids.len()
        ),
    }
}

/// The record `wanted` names among those of `graph`: by its number, with
/// or without the padding, or the newest for [`LATEST`].
pub fn chosen<'a>(store: &'a Retrievals, graph: &str, wanted: &str) -> Option<&'a RetrievalRecord> {
    let mut listed = store.records.values().rev().filter(|r| r.graph == graph);
    if wanted == LATEST {
        listed.next()
    } else {
        listed.find(|r| record_number(&r.record_id) == record_number(wanted))
    }
}

/// Reads the session's records on the first graph and after every switch,
/// and keeps what comes back.
pub struct RetrievalPlugin {
    /// What `--retrieval` asked for.
    pub wanted: Option<String>,
}

impl Plugin for RetrievalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Retrievals>()
            .insert_resource(RetrievalRead {
                wanted: self.wanted.clone(),
                ..RetrievalRead::default()
            })
            .add_systems(Update, (start_read, take_answer).chain());
    }
}

/// Starts a read of the route when the session or graph drawn is not the
/// one the last read was for. A read still under way is replaced, and its
/// answer dropped.
fn start_read(catalogue: Option<Res<Catalogue>>, mut read: ResMut<RetrievalRead>) {
    let Some(catalogue) = catalogue.filter(|c| c.is_changed()) else {
        return;
    };
    let key = (
        catalogue.session.session_id.clone(),
        catalogue.graph.clone(),
    );
    if read.read_for.as_ref() == Some(&key) {
        return;
    }
    let (hub, session) = (catalogue.hub.clone(), key.0.clone());
    info!(
        "retrievals: reading the retrieval records of session {}",
        short_id(&session)
    );
    let asked = session.clone();
    let work = move || async move {
        hub::retrievals(&hub, &asked)
            .await
            .map_err(|e| e.to_string())
    };
    match in_background("read the retrieval records", work) {
        Ok(pending) => read.pending = Some((session, pending)),
        Err(message) => read.error = Some(message),
    }
    read.read_for = Some(key);
}

/// Keeps the records the route answered with, when they are of the
/// session drawn, and chooses the record `--retrieval` asked for.
fn take_answer(
    mut read: ResMut<RetrievalRead>,
    catalogue: Option<Res<Catalogue>>,
    mut store: ResMut<Retrievals>,
    mut filter: ResMut<Filter>,
) {
    let Some((session, pending)) = read.pending.as_ref() else {
        return;
    };
    let answer = match poll(pending) {
        Poll::Waiting => return,
        Poll::Ready(answer) => answer,
        Poll::Lost => Err("the read stopped without an answer".to_owned()),
    };
    let session = session.clone();
    read.pending = None;
    let Some(catalogue) = catalogue.filter(|c| c.session.session_id == session) else {
        return;
    };
    let wanted = read.wanted.take();
    match answer {
        Ok(records) => {
            info!(
                "retrievals: {} records from session {}, {} of them of graph {:?}",
                records.len(),
                short_id(&session),
                records
                    .iter()
                    .filter(|r| r.graph == catalogue.graph)
                    .count(),
                catalogue.graph
            );
            read.error = None;
            if remember(store.bypass_change_detection(), &session, records) {
                store.set_changed();
            }
            if let Some(wanted) = wanted {
                match chosen(&store, &catalogue.graph, &wanted) {
                    Some(record) => {
                        info!(
                            "retrievals: chose record {} ({})",
                            record_number(&record.record_id),
                            entry_label(record)
                        );
                        filter.retrieval = Some(focus_of(record));
                    }
                    None => warn!(
                        "--retrieval: no retrieval record {wanted:?} of graph {:?} is kept",
                        catalogue.graph
                    ),
                }
            }
        }
        Err(message) => {
            warn!("retrievals: {message}");
            if let Some(wanted) = wanted {
                warn!("--retrieval {wanted}: no record could be read to choose from");
            }
            read.error = Some(message);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph_index::build;
    use crate::graph_index::tests::node;
    use crate::hub::{NodeType, RetrievedNode};
    use crate::layout::place;

    /// A record of `graph`, numbered `n`, naming `ids`, or none for `None`.
    pub(crate) fn record(n: u32, graph: &str, ids: Option<&[&str]>) -> RetrievalRecord {
        RetrievalRecord {
            record_id: format!("{n:012}"),
            at: format!("2026-11-07T10:{:02}:05.250000Z", n % 60),
            tool: "epimemer.search".into(),
            query: format!("query=q{n}"),
            graph: graph.into(),
            retrieved: ids.map(|ids| {
                ids.iter()
                    .map(|&id| RetrievedNode {
                        node_id: id.into(),
                        provenance: "lexical".into(),
                        score: Some(1.0),
                    })
                    .collect()
            }),
            truncated: false,
            response_text: "hits".into(),
        }
    }

    fn ids(store: &Retrievals) -> Vec<&str> {
        store.records.keys().map(String::as_str).collect()
    }

    #[test]
    fn a_record_that_comes_twice_is_kept_once_by_its_id() {
        let mut store = Retrievals::default();
        assert!(remember(
            &mut store,
            "s",
            [record(3, "g", None), record(1, "g", Some(&[]))]
        ));
        // The replay and the route bring the same records again.
        assert!(!remember(
            &mut store,
            "s",
            [record(1, "g", Some(&[])), record(3, "g", None)]
        ));
        assert_eq!(ids(&store), ["000000000001", "000000000003"]);
        // The stream's copy on a hub bound beyond loopback carries no text,
        // and the copy with text stays.
        let mut bare = record(3, "g", None);
        bare.query.clear();
        bare.response_text.clear();
        assert!(!remember(&mut store, "s", [bare.clone()]));
        assert_eq!(store.records["000000000003"].query, "query=q3");
        let mut held = Retrievals::default();
        remember(&mut held, "s", [bare]);
        assert!(remember(&mut held, "s", [record(3, "g", None)]));
        assert_eq!(held.records["000000000003"].response_text, "hits");
    }

    #[test]
    fn the_most_recent_are_kept_and_another_session_starts_afresh() {
        let mut store = Retrievals::default();
        remember(&mut store, "s", (1..=60).map(|n| record(n, "g", None)));
        assert_eq!(store.records.len(), KEEP);
        assert_eq!(
            store.records.keys().next().map(String::as_str),
            Some("000000000011")
        );
        assert!(
            !remember(&mut store, "s", [record(4, "g", None)]),
            "older than every record kept"
        );
        assert!(remember(&mut store, "s", [record(61, "g", None)]));
        assert_eq!(
            store.records.keys().next().map(String::as_str),
            Some("000000000012")
        );
        assert!(remember(&mut store, "other", [record(2, "g", None)]));
        assert_eq!(
            (store.session.as_str(), ids(&store)),
            ("other", vec!["000000000002"])
        );
    }

    #[test]
    fn the_picker_lists_the_graphs_records_newest_first() {
        let mut store = Retrievals::default();
        remember(
            &mut store,
            "s",
            [
                record(2, "g", Some(&["a", "b", "a"])),
                record(10, "g", None),
                record(9, "elsewhere", Some(&["a"])),
                record(5, "g", Some(&[])),
            ],
        );
        let listed = entries(&store, "s", "g");
        let numbers: Vec<&str> = listed.iter().map(|e| record_number(&e.record_id)).collect();
        assert_eq!(
            numbers,
            ["10", "5", "2"],
            "newest first, the graph drawn only"
        );
        assert_eq!(
            listed.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            [
                "search  2026-11-07 10:10:05 UTC  not declared",
                "search  2026-11-07 10:05:05 UTC  0 retrieved",
                "search  2026-11-07 10:02:05 UTC  2 retrieved",
            ],
            "an id named twice counts once"
        );
        assert!(entries(&store, "another", "g").is_empty());
        assert_eq!(entries(&store, "s", "elsewhere").len(), 1);
    }

    #[test]
    fn the_button_names_the_record_chosen() {
        let mut store = Retrievals::default();
        remember(&mut store, "s", [record(12, "g", Some(&["a"]))]);
        assert_eq!(button_label(None, &store), "any");
        let focus = focus_of(&store.records["000000000012"]);
        assert_eq!(button_label(Some(&focus), &store), "search 10:12:05");
        let gone = RetrievalFocus {
            record_id: "000000000007".into(),
            retrieved: None,
        };
        assert_eq!(button_label(Some(&gone), &store), "record 7");
        assert_eq!(record_number("000000000000"), "0");
        assert_eq!(short_tool("other.tool"), "other.tool");
    }

    #[test]
    fn the_panel_counts_the_ids_absent_from_the_graph_drawn() {
        let nodes = vec![
            node("t", NodeType::Topic, 1),
            node("a", NodeType::Fact, 2),
            node("b", NodeType::Fact, 3),
        ];
        let mut index = build(&nodes, &[], &place(&nodes, &[]));
        let named = record(4, "g", Some(&["a", "b", "gone", "never", "a"]));
        let focus = focus_of(&named);
        assert_eq!(absent_count(focus.retrieved.as_ref().unwrap(), &index), 2);
        // A node that has left the active set is absent too.
        index.active[index.by_id["b"]] = false;
        assert_eq!(
            panel_line(&focus, &index),
            "record 4: 3 of its 4 retrieved ids are absent from the graph drawn"
        );
        assert_eq!(
            panel_line(&focus_of(&record(5, "g", None)), &index),
            "record 5 declares no nodes, so every node passes"
        );
    }

    #[test]
    fn retrieval_names_a_record_by_its_number_or_the_latest() {
        let mut store = Retrievals::default();
        remember(
            &mut store,
            "s",
            [
                record(3, "g", None),
                record(8, "g", None),
                record(9, "elsewhere", None),
            ],
        );
        let number = |wanted| chosen(&store, "g", wanted).map(|r| record_number(&r.record_id));
        assert_eq!(number(LATEST), Some("8"), "the newest of the graph drawn");
        assert_eq!(number("3"), Some("3"));
        assert_eq!(number("000000000003"), Some("3"));
        assert_eq!(number("9"), None, "another graph's");
        assert_eq!(chosen(&Retrievals::default(), "g", LATEST), None);
    }
}
