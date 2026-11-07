//! The lists the bar's pickers open: what each shows and the rows a click
//! chooses from.
//!
//! A list is spawned under its picker's button when it opens and despawned
//! when it closes, so it always shows the current lists. Long lists scroll
//! with the wheel; the topic list also narrows to what is typed. The
//! retrieval list is the session's records of the graph drawn, newest
//! first, under a row that chooses none.

use bevy::prelude::*;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui_widgets::ScrollArea;

use crate::pickers::{Pickers, short_id};
use crate::reload::Catalogue;
use crate::retrieval::Entry;

use super::{CheckMark, Control, Panel, Picker, Row, TopicCount, TopicField, TopicRow, style};

/// The tallest a list grows before it scrolls, in logical pixels.
const LIST_HEIGHT: f32 = 340.0;
/// How wide the topic and source lists are.
const LIST_WIDTH: f32 = 440.0;

/// What a list is made from.
pub struct Content<'a> {
    pub catalogue: &'a Catalogue,
    pub pickers: &'a Pickers,
    pub prefix: &'a str,
    pub reading_sessions: bool,
    pub reading_graphs: bool,
    pub notice: Option<&'a str>,
    /// The retrieval records of the graph drawn, newest first.
    pub records: &'a [Entry],
    pub reading_records: bool,
    /// Why the records could not be read.
    pub records_error: Option<&'a str>,
}

/// The topic list's count line.
pub fn topic_count(shown: usize, total: usize, prefix: &str) -> String {
    if prefix.trim().is_empty() {
        format!("{total} topics; type to narrow, Enter takes the first")
    } else {
        format!("{shown} of {total} topics start with {:?}", prefix.trim())
    }
}

/// Spawns the open picker's list under its anchor.
pub fn spawn(commands: &mut Commands, anchor: Entity, picker: Picker, content: &Content) {
    let panel = commands
        .spawn((
            Panel,
            Node {
                position_type: PositionType::Absolute,
                top: percent(100),
                left: px(0),
                margin: UiRect::top(px(4)),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                padding: UiRect::all(px(6)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(4)),
                min_width: px(220),
                ..default()
            },
            BackgroundColor(style::PANEL),
            BorderColor::all(style::PANEL_EDGE),
            GlobalZIndex(100),
            ChildOf(anchor),
        ))
        .id();
    match picker {
        Picker::Session => sessions(commands, panel, content),
        Picker::Graph => graphs(commands, panel, content),
        Picker::Metacontext => metacontexts(commands, panel, content),
        Picker::Topic => topics(commands, panel, content),
        Picker::Source => sources(commands, panel, content),
        Picker::Retrieval => retrievals(commands, panel, content),
    }
}

/// A row a click chooses.
fn row(control: Control, words: String, colour: Color) -> impl Bundle {
    (
        control,
        Row,
        Button,
        style::row_node(),
        BackgroundColor(style::ROW),
        children![style::text(words, colour)],
    )
}

/// A list that scrolls once it is taller than [`LIST_HEIGHT`].
fn scrolling(width: Val) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Column,
            max_height: px(LIST_HEIGHT),
            width,
            // A name too long for the list is cut at its edge rather than
            // drawn over the scene.
            overflow: Overflow {
                x: OverflowAxis::Clip,
                y: OverflowAxis::Scroll,
            },
            ..default()
        },
        ScrollArea,
    )
}

fn note(commands: &mut Commands, panel: Entity, words: String, colour: Color) {
    commands.spawn((style::text(words, colour), ChildOf(panel)));
}

fn sessions(commands: &mut Commands, panel: Entity, content: &Content) {
    let reading = if content.reading_sessions {
        " (reading...)"
    } else {
        ""
    };
    note(
        commands,
        panel,
        format!("Sessions the hub lists{reading}"),
        style::CAPTION,
    );
    let list = commands.spawn((scrolling(Val::Auto), ChildOf(panel))).id();
    for session in &content.catalogue.sessions {
        let words = format!(
            "{}  {}  active graph {}{}",
            short_id(&session.session_id),
            session.backend,
            session.active_graph,
            if session.connected {
                ""
            } else {
                "  (disconnected)"
            }
        );
        if session.connected {
            commands.spawn((
                row(
                    Control::Session(session.session_id.clone()),
                    words,
                    style::TEXT,
                ),
                ChildOf(list),
            ));
        } else {
            // The hub reads only from a connected session, so this row
            // chooses nothing.
            commands.spawn((
                style::row_node(),
                children![style::text(words, style::FAINT)],
                ChildOf(list),
            ));
        }
    }
}

fn graphs(commands: &mut Commands, panel: Entity, content: &Content) {
    let catalogue = content.catalogue;
    let backend = catalogue
        .sessions
        .iter()
        .chain([&catalogue.session])
        .find(|s| s.session_id == catalogue.browsing)
        .map_or("", |s| s.backend.as_str());
    note(
        commands,
        panel,
        format!(
            "Graphs in session {} ({backend})",
            short_id(&catalogue.browsing)
        ),
        style::CAPTION,
    );
    let Some(list) = &catalogue.graphs else {
        let words = match (content.reading_graphs, content.notice) {
            (true, _) => "Reading the list...".to_owned(),
            (false, Some(notice)) => notice.to_owned(),
            (false, None) => "No list read yet.".to_owned(),
        };
        let colour = if content.reading_graphs {
            style::CAPTION
        } else {
            style::ERROR
        };
        note(commands, panel, words, colour);
        return;
    };
    let rows = commands.spawn((scrolling(Val::Auto), ChildOf(panel))).id();
    for graph in &list.graphs {
        let words = if *graph == list.active_graph {
            format!("{graph}  (active in the session)")
        } else {
            graph.clone()
        };
        commands.spawn((
            row(Control::Graph(graph.clone()), words, style::TEXT),
            ChildOf(rows),
        ));
    }
}

fn metacontexts(commands: &mut Commands, panel: Entity, content: &Content) {
    commands.spawn((
        row(Control::AllMetacontexts, "All".into(), style::TEXT),
        ChildOf(panel),
    ));
    for entry in &content.pickers.metacontexts {
        commands.spawn((
            Control::Metacontext(entry.id.clone()),
            Row,
            Button,
            style::row_node(),
            BackgroundColor(style::ROW),
            ChildOf(panel),
            children![
                (
                    Node {
                        width: px(12),
                        height: px(12),
                        border: UiRect::all(px(1)),
                        padding: UiRect::all(px(2)),
                        border_radius: BorderRadius::all(px(2)),
                        ..default()
                    },
                    BorderColor::all(style::CAPTION),
                    Pickable::IGNORE,
                    children![(
                        CheckMark(entry.id.clone()),
                        Node {
                            width: percent(100),
                            height: percent(100),
                            border_radius: BorderRadius::all(px(1)),
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                        Pickable::IGNORE,
                    )],
                ),
                style::text(
                    format!("{}  {} nodes", entry.name, entry.nodes),
                    style::TEXT
                ),
            ],
        ));
    }
}

fn topics(commands: &mut Commands, panel: Entity, content: &Content) {
    commands.spawn((
        TopicField,
        Node {
            width: px(LIST_WIDTH),
            padding: UiRect::axes(px(5), px(3)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(3)),
            ..default()
        },
        EditableText::new(content.prefix),
        TextFont {
            font_size: FontSize::Px(style::FONT_SIZE),
            ..default()
        },
        TextColor(style::TEXT),
        TextLayout::no_wrap(),
        TextCursorStyle {
            color: style::TEXT,
            ..default()
        },
        BackgroundColor(style::BUTTON),
        BorderColor::all(style::PANEL_EDGE),
        ChildOf(panel),
    ));
    let total = content.pickers.topics.len();
    commands.spawn((
        TopicCount,
        style::text(topic_count(total, total, content.prefix), style::CAPTION),
        ChildOf(panel),
    ));
    let list = commands
        .spawn((scrolling(px(LIST_WIDTH)), ChildOf(panel)))
        .id();
    for (place, topic) in content.pickers.topics.iter().enumerate() {
        commands.spawn((
            row(
                Control::Topic(topic.id.clone()),
                topic.label.clone(),
                style::TEXT,
            ),
            TopicRow(place),
            ChildOf(list),
        ));
    }
}

fn sources(commands: &mut Commands, panel: Entity, content: &Content) {
    // The snapshot carries no document names, a known gap in the contract,
    // so a document is its short id and how many nodes were read from it.
    note(
        commands,
        panel,
        format!(
            "{} documents, by id: the snapshot names none",
            content.pickers.sources.len()
        ),
        style::CAPTION,
    );
    let list = commands
        .spawn((scrolling(px(LIST_WIDTH)), ChildOf(panel)))
        .id();
    for source in &content.pickers.sources {
        commands.spawn((
            row(
                Control::Source(source.id.clone()),
                source.label.clone(),
                style::TEXT,
            ),
            ChildOf(list),
        ));
    }
}

fn retrievals(commands: &mut Commands, panel: Entity, content: &Content) {
    let reading = if content.reading_records {
        " (reading...)"
    } else {
        ""
    };
    note(
        commands,
        panel,
        format!(
            "Retrieval records of {}, newest first{reading}",
            content.catalogue.graph
        ),
        style::CAPTION,
    );
    commands.spawn((
        row(
            Control::ClearRetrieval,
            "None: every node passes".into(),
            style::TEXT,
        ),
        ChildOf(panel),
    ));
    if content.records.is_empty() {
        let (words, colour) = match content.records_error {
            Some(error) if !content.reading_records => (
                format!("Could not read the retrieval records: {error}"),
                style::ERROR,
            ),
            _ => (
                "No retrieval record of this graph yet.".to_owned(),
                style::CAPTION,
            ),
        };
        note(commands, panel, words, colour);
        return;
    }
    let list = commands
        .spawn((scrolling(px(LIST_WIDTH)), ChildOf(panel)))
        .id();
    for entry in content.records {
        commands.spawn((
            row(
                Control::Retrieval(entry.record_id.clone()),
                entry.label.clone(),
                style::TEXT,
            ),
            ChildOf(list),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_topic_count_says_what_was_typed() {
        assert_eq!(
            topic_count(430, 430, ""),
            "430 topics; type to narrow, Enter takes the first"
        );
        assert_eq!(
            topic_count(3, 430, " vis "),
            "3 of 430 topics start with \"vis\""
        );
    }
}
