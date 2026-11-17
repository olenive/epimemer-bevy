//! The cheatsheet (DESIGN.md §10): the controls, what the colours mean and
//! the concepts the scene draws, in the side panel in place of the node.
//!
//! `?`, or the bar's Help button, shows it, in a panel collapsed or not, and
//! the same key or button puts it away, which leaves the panel on what it
//! showed before: the panel (`panel.rs`) shows the cheatsheet while
//! [`HelpOpen`] says so, over the log or the node that [`LogOpen`] chooses
//! between. `J` or the Log button shows the log in its place.
//! `--ui-state help` opens it from the start.
//!
//! What it says is plain lines ([`lines`]), the same every time, in three
//! parts:
//!
//! - the controls: the mouse, then every key, the rows of the key map that
//!   `--help` prints (`keys.rs`), so the two cannot differ;
//! - the colours: each entry written in the colour it explains, taken from
//!   the function or constant the scene draws with (`scene.rs`). The discs,
//!   the lines, the connectors and "now" carry an alpha meant for the
//!   scene, so on the panel they are drawn whole, as the bar draws a kind's
//!   swatch. The two brackets differ only in their alpha, so they keep it,
//!   and over the panel's dark background look as they do over the scene's;
//! - the concepts, a few words each, for somebody who has not read the
//!   design, saying nothing the scene does not draw.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::edge_index::{Relation, Succession};
use crate::event_log::{LogOpen, toggle_log};
use crate::keys::{RETARGET_NOTE, Row, key_rows, mouse_rows};
use crate::layout::Kind;
use crate::panel::{Line, PanelCollapsed};
use crate::scene::{
    BRACKET_COLOUR, INFERRED_BRACKET_COLOUR, NOW_COLOUR, kind_colour, relation_colour,
    succession_colour,
};

/// The panel's title while it shows the cheatsheet.
pub const TITLE: &str = "Cheatsheet";

/// Whether the side panel shows the cheatsheet, in place of the log or the
/// node it would show otherwise.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HelpOpen(pub bool);

/// What the side panel is asked to show: the log or the node, the
/// cheatsheet over either, and whether it is collapsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelChoice {
    pub log: bool,
    pub help: bool,
    pub collapsed: bool,
}

/// What `?` and the bar's Help button do: the cheatsheet shown, in a panel
/// that was collapsed or showing the node or the log; or, where the panel
/// showed it, put away, which leaves the panel on what it showed before.
pub fn press_help(choice: PanelChoice) -> PanelChoice {
    let shown = choice.help && !choice.collapsed;
    PanelChoice {
        help: !shown,
        collapsed: false,
        ..choice
    }
}

/// What `J` and the bar's Log button do: the log shown, in place of the
/// cheatsheet too; or, where the panel showed the log, the panel back on
/// the node ([`toggle_log`]).
pub fn press_log(choice: PanelChoice) -> PanelChoice {
    let (log, collapsed) = toggle_log(choice.log && !choice.help, choice.collapsed);
    PanelChoice {
        log,
        help: false,
        collapsed,
    }
}

/// The resources that say what the side panel shows, for the keys and the
/// buttons that change it.
#[derive(SystemParam)]
pub struct PanelShows<'w> {
    log: ResMut<'w, LogOpen>,
    help: ResMut<'w, HelpOpen>,
    collapsed: ResMut<'w, PanelCollapsed>,
}

impl PanelShows<'_> {
    pub fn choice(&self) -> PanelChoice {
        PanelChoice {
            log: self.log.0,
            help: self.help.0,
            collapsed: self.collapsed.0,
        }
    }

    /// Writes a choice back, changing only what differs.
    pub fn set(&mut self, choice: PanelChoice) {
        self.log.set_if_neq(LogOpen(choice.log));
        self.help.set_if_neq(HelpOpen(choice.help));
        self.collapsed.set_if_neq(PanelCollapsed(choice.collapsed));
    }
}

/// One entry of the colours: its words, and the colour they are written in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatch {
    pub words: &'static str,
    pub colour: Color,
}

/// A group of the colours: the caption over it, where it has one, and its
/// entries.
#[derive(Clone, Debug, PartialEq)]
pub struct ColourGroup {
    pub caption: Option<&'static str>,
    pub entries: Vec<Swatch>,
}

/// The caption over the connectors.
pub const CONNECTORS: &str = "Connectors between versions, always drawn";

/// What a connector joins, in the colour of an edge that is not evidence,
/// and in the colour of one that is.
pub const SUCCESSION_WORDS: &str =
    "an older version to the claim that replaced it or absorbed it in a merge";
pub const EVIDENCE_WORDS: &str = "a corrected or merged fact to an inference it supported";

/// What the line in the caption colour after the colours says.
pub const DIMMED: &str = "A dimmed node does not pass the filters, or was not returned by the retrieval chosen in the bar. With a node focused, the rest also fade with their distance from it.";

/// A colour the scene adds onto its dark background, whole, so that it
/// reads on the panel as it does in the scene.
fn whole(colour: Color) -> Color {
    colour.with_alpha(1.0)
}

/// The colours, group by group, each from the function or constant the
/// scene draws with.
pub fn colour_groups() -> Vec<ColourGroup> {
    let entry = |words, colour| Swatch { words, colour };
    let relation = |words, relation| entry(words, whole(relation_colour(relation)));
    vec![
        ColourGroup {
            caption: Some("Discs"),
            entries: vec![
                entry("topic node", whole(kind_colour(Kind::Topic))),
                entry("fact", whole(kind_colour(Kind::Fact))),
                entry("inference", whole(kind_colour(Kind::Inference))),
            ],
        },
        ColourGroup {
            caption: Some(
                "Lines between claims: shown for the node you hover or focus, or for every node with L",
            ),
            entries: vec![
                relation("supports", Relation::Supports),
                relation("assessed", Relation::Assessed),
                relation("related", Relation::Related),
                relation("similarity", Relation::Similarity),
                relation("contradiction", Relation::Contradiction),
                relation("any other relation", Relation::Other),
            ],
        },
        ColourGroup {
            caption: Some(CONNECTORS),
            entries: vec![
                entry(
                    SUCCESSION_WORDS,
                    whole(succession_colour(Succession::Superseded)),
                ),
                entry(
                    EVIDENCE_WORDS,
                    whole(succession_colour(Succession::EvidenceSuperseded)),
                ),
            ],
        },
        ColourGroup {
            caption: None,
            entries: vec![entry(
                "now: its tick on the time axis, and the ring around the map at its height",
                whole(NOW_COLOUR),
            )],
        },
        ColourGroup {
            caption: Some("Brackets beside a focused column, in claim time"),
            entries: vec![
                entry("a period a source states", BRACKET_COLOUR),
                entry(
                    "a period read from tense or context",
                    INFERRED_BRACKET_COLOUR,
                ),
            ],
        },
    ]
}

/// The concepts, each a name and a sentence or two.
pub const CONCEPTS: [Row; 12] = [
    (
        "Topic node",
        "A subject the memory holds material about. The largest discs.",
    ),
    ("Fact", "A statement taken from a source document."),
    (
        "Inference",
        "A conclusion drawn from facts. It is kept apart from them because it is provisional.",
    ),
    (
        "The map",
        "Topics are placed by meaning, so topics about related things sit near each other. A topic's facts and inferences sit around it.",
    ),
    (
        "Height is time",
        "In recorded time every node sits at the moment the graph learned it. In claim time (C) a claim linked to a timepoint becomes a column over its period, or a smear where the period is vague.",
    ),
    (
        "Succession",
        "Nothing is deleted. When a claim is corrected, or the world changes, the old version is kept and a connector joins it to the new one. Only current versions are drawn, so most connectors are a short stub that ends in a cross.",
    ),
    (
        "Metacontext",
        "The world a claim is made in: the real one, or a novel's. M fits the view to each in turn.",
    ),
    (
        "Focus",
        "Click a node to read it in full in this panel, with its edges and its periods. B goes back.",
    ),
    (
        "Filters",
        "The bar narrows the graph by metacontext, kind, status, topic, source, retrieval and time window. What does not pass is dimmed or hidden, whichever the bar's Filtered out button says.",
    ),
    (
        "Retrieval",
        "Choose one of the session's recent tool calls in the bar's Retrieval list to see the nodes it handed the agent.",
    ),
    (
        "Log",
        "What the session has done: its acts, the advisories it was given and its pipeline runs. J shows it.",
    ),
    (
        "Session and graph",
        "The hub lists the MCP sessions that have connected to it. The bar's first two pickers, Session and Graph, choose a session and one of its graphs.",
    ),
];

/// A row of the key map as a line of the panel.
fn control_line((keys, what): Row) -> Line {
    Line::Field {
        name: keys.to_owned(),
        value: what.to_owned(),
    }
}

/// The cheatsheet's lines: the controls, the colours, the concepts.
pub fn lines() -> Vec<Line> {
    let mut lines = vec![
        Line::Heading("Controls".to_owned()),
        Line::Note("Mouse".to_owned()),
    ];
    lines.extend(mouse_rows().into_iter().map(control_line));
    lines.push(Line::Note("Keys".to_owned()));
    lines.extend(key_rows().into_iter().map(control_line));
    lines.push(Line::Note(RETARGET_NOTE.to_owned()));

    lines.push(Line::Heading("Colours".to_owned()));
    for group in colour_groups() {
        lines.extend(group.caption.map(|c| Line::Note(c.to_owned())));
        lines.extend(group.entries.iter().map(|swatch| Line::Colour {
            colour: swatch.colour,
            text: swatch.words.to_owned(),
        }));
    }
    lines.push(Line::Note(DIMMED.to_owned()));

    lines.push(Line::Heading("Concepts".to_owned()));
    lines.extend(CONCEPTS.iter().map(|&(name, text)| Line::Term {
        name: name.to_owned(),
        text: text.to_owned(),
    }));
    lines
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::keys::{help, row};

    #[test]
    fn the_cheatsheet_lists_every_row_of_the_help_and_the_help_every_control_it_lists() {
        let lines = lines();
        let fields: Vec<(&str, &str)> = lines
            .iter()
            .filter_map(|line| match line {
                Line::Field { name, value } => Some((name.as_str(), value.as_str())),
                _ => None,
            })
            .collect();
        let help = help();
        // Every row the help prints is a row of the cheatsheet, in its order.
        let printed: Vec<&str> = help.lines().filter(|l| l.starts_with("  ")).collect();
        let shown: Vec<String> = fields.iter().map(|&(keys, what)| row(keys, what)).collect();
        assert!(!printed.is_empty());
        assert_eq!(printed, shown);
        // And every row of the cheatsheet is in the help.
        for &(keys, what) in &fields {
            assert!(
                help.lines().any(|l| l == row(keys, what)),
                "{keys} missing from the help"
            );
        }
        assert!(lines.contains(&Line::Note(RETARGET_NOTE.to_owned())));
        assert!(help.contains(RETARGET_NOTE));
        let first_key = fields.iter().position(|(keys, _)| *keys == "W A S D");
        let last_mouse = fields.iter().position(|(keys, _)| *keys == "Shift+wheel");
        assert!(last_mouse < first_key, "the mouse, then the keys");
        assert!(fields.iter().any(|(keys, _)| *keys == "?"));
    }

    #[test]
    fn the_parts_come_in_order_controls_colours_concepts() {
        let headings: Vec<String> = lines()
            .into_iter()
            .filter_map(|line| match line {
                Line::Heading(words) => Some(words),
                _ => None,
            })
            .collect();
        assert_eq!(headings, ["Controls", "Colours", "Concepts"]);
        let terms: Vec<String> = lines()
            .into_iter()
            .filter_map(|line| match line {
                Line::Term { name, .. } => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(terms.len(), CONCEPTS.len());
        assert_eq!(terms[0], "Topic node");
        assert!(lines().contains(&Line::Note(DIMMED.to_owned())));
    }

    fn colour_of(words: &str) -> Color {
        colour_groups()
            .into_iter()
            .flat_map(|group| group.entries)
            .find(|swatch| swatch.words == words)
            .unwrap_or_else(|| panic!("no entry {words:?}"))
            .colour
    }

    #[test]
    fn each_colour_is_the_one_the_scene_draws_with_whole_where_the_scene_adds_it() {
        for (words, kind) in [
            ("topic node", Kind::Topic),
            ("fact", Kind::Fact),
            ("inference", Kind::Inference),
        ] {
            assert_eq!(colour_of(words), kind_colour(kind).with_alpha(1.0));
        }
        for (words, relation) in [
            ("supports", Relation::Supports),
            ("assessed", Relation::Assessed),
            ("related", Relation::Related),
            ("similarity", Relation::Similarity),
            ("contradiction", Relation::Contradiction),
            ("any other relation", Relation::Other),
        ] {
            assert_eq!(colour_of(words), relation_colour(relation).with_alpha(1.0));
        }
        for succession in [
            Succession::Superseded,
            Succession::FollowedBy,
            Succession::Merged,
        ] {
            assert_eq!(
                colour_of(SUCCESSION_WORDS),
                succession_colour(succession).with_alpha(1.0)
            );
        }
        for succession in [Succession::EvidenceSuperseded, Succession::EvidenceMerged] {
            assert_eq!(
                colour_of(EVIDENCE_WORDS),
                succession_colour(succession).with_alpha(1.0)
            );
        }
        assert_eq!(
            colour_of("now: its tick on the time axis, and the ring around the map at its height"),
            NOW_COLOUR.with_alpha(1.0)
        );
        assert_eq!(colour_of("a period a source states"), BRACKET_COLOUR);
        assert_eq!(
            colour_of("a period read from tense or context"),
            INFERRED_BRACKET_COLOUR
        );
        assert_ne!(
            colour_of("a period a source states"),
            colour_of("a period read from tense or context"),
            "the brackets keep the alpha that tells them apart"
        );
        // The panel draws each entry in its colour.
        for group in colour_groups() {
            for swatch in group.entries {
                assert!(lines().contains(&Line::Colour {
                    colour: swatch.colour,
                    text: swatch.words.to_owned(),
                }));
            }
        }
    }

    #[test]
    fn the_connectors_have_one_entry_per_colour_the_scene_gives_them() {
        let all = [
            Succession::Superseded,
            Succession::FollowedBy,
            Succession::Merged,
            Succession::EvidenceSuperseded,
            Succession::EvidenceMerged,
        ];
        let colours: HashSet<[u8; 4]> = all
            .iter()
            .map(|&s| succession_colour(s).to_srgba().to_u8_array())
            .collect();
        let connectors = colour_groups()
            .into_iter()
            .find(|group| group.caption == Some(CONNECTORS))
            .unwrap();
        assert_eq!(connectors.entries.len(), colours.len());
    }

    #[test]
    fn question_mark_shows_the_cheatsheet_and_again_returns_to_what_the_panel_showed() {
        let node = PanelChoice::default();
        let help = press_help(node);
        assert_eq!(help, PanelChoice { help: true, ..node });
        assert_eq!(press_help(help), node, "back on the node");

        let log = press_log(node);
        assert!(log.log && !log.help);
        let over_log = press_help(log);
        assert!(over_log.help && over_log.log);
        assert_eq!(press_help(over_log), log, "back on the log");
        // J from the cheatsheet shows the log, and again returns to the node.
        let from_help = press_log(help);
        assert_eq!(from_help, log);
        assert_eq!(press_log(over_log), log, "the log, uncovered");
        assert_eq!(press_log(log), node);

        // With the panel collapsed, ? opens it on the cheatsheet.
        let collapsed = PanelChoice {
            collapsed: true,
            ..help
        };
        assert_eq!(press_help(collapsed), help);
        let folded_node = PanelChoice {
            collapsed: true,
            ..node
        };
        assert_eq!(press_help(folded_node), help);
    }
}
