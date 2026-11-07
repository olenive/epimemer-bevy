//! The reflect counter the bar shows (DESIGN.md §10): the stores since a
//! graph's last reflect, against the threshold at which reflect is
//! suggested.
//!
//! Two things set it. The graph list's `reflect` describes the session's
//! active graph only (VIZ_CONTRACT.md §4), so each graph list of the session
//! drawn that comes, with the first graph, a switch, a read again or the
//! graph picker opening, sets the counter of its active graph
//! ([`from_list`]), once per list. A `reflect_counter_updated` of the
//! session and graph drawn, which `live.rs` hands on as it comes, sets the
//! counter of the graph it names ([`from_event`]). The bar shows the counter
//! while it is the graph drawn's, and a dash for any other graph, whose
//! counter the hub does not report ([`label`]).

use bevy::prelude::*;

use crate::hub::{GraphList, Reflect};
use crate::reload::Catalogue;

/// The last reflect counter heard of, and whose it is.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ReflectCounter {
    /// The session and graph it is the counter of.
    pub session: String,
    pub graph: String,
    pub reflect: Option<Reflect>,
    /// The graph list last read into it, with its session, so that the
    /// same list sets it once and an event since is not undone.
    pub list: Option<(String, GraphList)>,
}

/// The counter after a graph list of `session`: the list's active graph's,
/// unless this list has been read into it already.
pub fn from_list(counter: &ReflectCounter, session: &str, list: &GraphList) -> ReflectCounter {
    if counter
        .list
        .as_ref()
        .is_some_and(|(read, held)| read == session && held == list)
    {
        return counter.clone();
    }
    ReflectCounter {
        session: session.to_owned(),
        graph: list.active_graph.clone(),
        reflect: Some(list.reflect),
        list: Some((session.to_owned(), list.clone())),
    }
}

/// The counter after a `reflect_counter_updated` of `session` about `graph`.
pub fn from_event(
    counter: &ReflectCounter,
    session: &str,
    graph: &str,
    reflect: Reflect,
) -> ReflectCounter {
    ReflectCounter {
        session: session.to_owned(),
        graph: graph.to_owned(),
        reflect: Some(reflect),
        list: counter.list.clone(),
    }
}

/// The bar's words for the counter of the graph drawn, and whether reflect
/// is suggested: `reflect N/M`, or a dash where the counter is another
/// graph's or none has come.
pub fn label(counter: &ReflectCounter, session: &str, graph: &str) -> (String, bool) {
    match counter.reflect {
        Some(reflect) if counter.session == session && counter.graph == graph => {
            let words = format!("reflect {}/{}", reflect.count, reflect.threshold);
            if reflect.suggested {
                (format!("{words}, suggested"), true)
            } else {
                (words, false)
            }
        }
        _ => ("reflect -".to_owned(), false),
    }
}

/// Reads every graph list of the session drawn into the counter.
pub struct ReflectPlugin;

impl Plugin for ReflectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReflectCounter>()
            .add_systems(Update, follow_graph_list);
    }
}

/// Sets the counter from the catalogue's graph list when it is a new list
/// of the session drawn.
fn follow_graph_list(catalogue: Option<Res<Catalogue>>, mut counter: ResMut<ReflectCounter>) {
    let Some(catalogue) = catalogue.filter(|c| c.is_changed()) else {
        return;
    };
    if catalogue.browsing != catalogue.session.session_id {
        return;
    }
    let Some(list) = catalogue.graphs.as_ref() else {
        return;
    };
    let next = from_list(&counter, &catalogue.browsing, list);
    counter.set_if_neq(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reflect(count: i64, suggested: bool) -> Reflect {
        Reflect {
            count,
            threshold: 10,
            suggested,
        }
    }

    fn list(active: &str, count: i64) -> GraphList {
        GraphList {
            graphs: vec!["g".into(), "h".into()],
            active_graph: active.into(),
            backend: "surrealdb".into(),
            reflect: reflect(count, false),
        }
    }

    #[test]
    fn the_graph_list_sets_the_active_graphs_counter_once() {
        let counter = from_list(&ReflectCounter::default(), "s", &list("g", 8));
        assert_eq!(label(&counter, "s", "g"), ("reflect 8/10".into(), false));
        assert_eq!(
            label(&counter, "s", "h"),
            ("reflect -".into(), false),
            "the list says nothing of a graph the session is not on"
        );
        assert_eq!(label(&counter, "another", "g"), ("reflect -".into(), false));
        // An event moves it; the same list read again does not undo that,
        // and a new list sets it again.
        let moved = from_event(&counter, "s", "g", reflect(9, false));
        assert_eq!(label(&moved, "s", "g").0, "reflect 9/10");
        assert_eq!(from_list(&moved, "s", &list("g", 8)), moved);
        assert_eq!(
            label(&from_list(&moved, "s", &list("g", 10)), "s", "g").0,
            "reflect 10/10"
        );
        assert_eq!(
            label(&ReflectCounter::default(), "s", "g"),
            ("reflect -".into(), false)
        );
    }

    #[test]
    fn the_event_sets_the_counter_of_the_graph_it_names() {
        let counter = from_list(&ReflectCounter::default(), "s", &list("g", 3));
        let other = from_event(&counter, "s", "h", reflect(10, true));
        assert_eq!(
            label(&other, "s", "h"),
            ("reflect 10/10, suggested".into(), true)
        );
        assert_eq!(label(&other, "s", "g").0, "reflect -");
    }

    fn catalogue(browsing: &str, graphs: Option<GraphList>) -> Catalogue {
        let session = crate::hub::SessionInfo {
            session_id: "s".into(),
            pid: 1,
            backend: "surrealdb".into(),
            active_graph: "g".into(),
            started_at: "2026-11-07T10:00:00Z".into(),
            connected: true,
            last_event_at: None,
            schema_version: Some(1),
        };
        Catalogue {
            hub: "127.0.0.1:1".into(),
            sessions: vec![session.clone()],
            session,
            graph: "g".into(),
            browsing: browsing.into(),
            graphs,
        }
    }

    #[test]
    fn the_counter_follows_the_graph_lists_of_the_session_drawn() {
        let mut app = App::new();
        app.add_plugins(ReflectPlugin)
            .insert_resource(catalogue("s", Some(list("g", 4))));
        app.update();
        let shown = |app: &App| label(app.world().resource::<ReflectCounter>(), "s", "g").0;
        assert_eq!(shown(&app), "reflect 4/10", "on connect");
        // A list of another session, browsed in the graph picker, is not
        // the session drawn's.
        app.insert_resource(catalogue("t", Some(list("g", 7))));
        app.update();
        assert_eq!(shown(&app), "reflect 4/10");
        app.insert_resource(catalogue("s", Some(list("g", 5))));
        app.update();
        assert_eq!(shown(&app), "reflect 5/10", "a read of the list again");
    }
}
