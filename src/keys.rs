//! The key map: every key and button the window answers to, in one place,
//! and the rows made from it, which both the help text of `--help` and the
//! side panel's cheatsheet (`cheatsheet.rs`) list, so the two cannot differ.
//!
//! The bar's view buttons (`bar.rs`) are made from [`VIEW_KEYS`] too, and
//! ask the camera for the same [`ViewRequest`] the key does, so a key and
//! its button take one path. No key reaches the camera, the focus, the bar
//! or the side panel while a text field in the bar is taking the keyboard,
//! except Escape, Enter and Shift+Enter, which the search field reads.

use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};

use crate::camera::ViewRequest;
use crate::presets::Preset;

/// A direction the free camera moves in while its key is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Forward,
    Back,
    Left,
    Right,
    Down,
    Up,
}

/// The movement keys.
pub const MOVE_KEYS: [(KeyCode, Move); 6] = [
    (KeyCode::KeyW, Move::Forward),
    (KeyCode::KeyS, Move::Back),
    (KeyCode::KeyA, Move::Left),
    (KeyCode::KeyD, Move::Right),
    (KeyCode::KeyQ, Move::Down),
    (KeyCode::KeyE, Move::Up),
];

/// A key that changes the view: the key, its name in the help, its
/// button's name in the bar, what it asks the camera for, and what that
/// does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewKey {
    pub code: KeyCode,
    pub label: &'static str,
    pub button: &'static str,
    pub request: ViewRequest,
    pub what: &'static str,
}

/// The key that shows or hides the bar, and its name in the help.
pub const BAR_KEY: (KeyCode, &str) = (KeyCode::KeyH, "H");

/// The key that shows or hides every claim-to-claim edge between visible
/// nodes, the edge toggle, and its name in the help. `E` moves the camera
/// up, so the toggle is `L`, for lines.
pub const EDGE_KEY: (KeyCode, &str) = (KeyCode::KeyL, "L");

/// The key that switches the time axis between recorded time and claim
/// time, and its name in the help: `C`, for claim.
pub const MODE_KEY: (KeyCode, &str) = (KeyCode::KeyC, "C");

/// The key that clears the focus, and its name in the help. With a list of
/// the bar open it closes the list instead, and with the search field
/// taking the keyboard it gives the keyboard back to the scene.
pub const CLEAR_FOCUS_KEY: (KeyCode, &str) = (KeyCode::Escape, "Escape");

/// The key that shows or hides the side panel, and its name in the help:
/// `P`, for panel.
pub const PANEL_KEY: (KeyCode, &str) = (KeyCode::KeyP, "P");

/// The key that shows the log in the side panel, or returns the panel to
/// the focused node, and its name in the help: `L`, for log, is the edge
/// toggle, so `J`, for journal.
pub const LOG_KEY: (KeyCode, &str) = (KeyCode::KeyJ, "J");

/// The key that focuses the node focused before the current one, from the
/// focus history, and its name in the help: `B`, for back.
pub const BACK_KEY: (KeyCode, &str) = (KeyCode::KeyB, "B");

/// The key that puts the keyboard into the bar's search field, and its name
/// in the help: `/`, as in many programs that search. With Shift held it is
/// [`HELP_KEY`] instead.
pub const SEARCH_KEY: (KeyCode, &str) = (KeyCode::Slash, "/");

/// The key that shows the cheatsheet in the side panel, or returns the
/// panel to what it showed before, and its name in the help: `?`, as in
/// many programs, which is Shift and the key of [`SEARCH_KEY`] where the
/// keyboard is laid out as in the US, and any key whose character is `?`
/// on any other layout ([`question_mark_typed`]).
pub const HELP_KEY: (KeyCode, &str) = (KeyCode::Slash, "?");

/// Whether either Shift key is held: `?` is Shift and `/`, and the wheel
/// with Shift changes the camera's speed rather than zooming.
pub fn shift_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
}

/// Whether a key went down in this update, not by repeating, whose
/// character on the keyboard's own layout is `?`. The keys are read as
/// they come rather than as held, since a key that goes down as `?` with
/// Shift may come up as `/` without it.
pub fn question_mark_typed<'a>(inputs: impl IntoIterator<Item = &'a KeyboardInput>) -> bool {
    inputs.into_iter().any(|input| {
        input.state == ButtonState::Pressed
            && !input.repeat
            && matches!(&input.logical_key, Key::Character(c) if c.as_str() == "?")
    })
}

/// Whether `/` was pressed this update without Shift, which opens search,
/// and was not `?` on the keyboard's layout (`typed_question_mark`).
pub fn search_pressed(keys: &ButtonInput<KeyCode>, typed_question_mark: bool) -> bool {
    keys.just_pressed(SEARCH_KEY.0) && !shift_held(keys) && !typed_question_mark
}

/// Whether `?` was pressed this update, as Shift and `/` or as a key whose
/// character is `?` (`typed_question_mark`), which shows or puts away the
/// cheatsheet: once, where a key is both.
pub fn help_pressed(keys: &ButtonInput<KeyCode>, typed_question_mark: bool) -> bool {
    (keys.just_pressed(HELP_KEY.0) && shift_held(keys)) || typed_question_mark
}

/// The key that fits the current search hit's topic, as the bar's Go
/// button does, and its name in the help: `G`, for go.
pub const GO_KEY: (KeyCode, &str) = (KeyCode::KeyG, "G");

/// The key that recalculates the topic map, as the bar's Recalculate
/// layout button does, and its name in the help: `U`, for update, away
/// from the movement keys so that it is not pressed by chance.
pub const RECALCULATE_KEY: (KeyCode, &str) = (KeyCode::KeyU, "U");

/// The keys that walk the search hits while the search field takes the
/// keyboard, and their name in the help.
pub const WALK_KEYS: [KeyCode; 2] = [KeyCode::Enter, KeyCode::NumpadEnter];

/// The keys that change the view.
pub const VIEW_KEYS: [ViewKey; 8] = [
    ViewKey {
        code: KeyCode::Digit1,
        label: "1",
        button: "Graph",
        request: ViewRequest::Preset(Preset::Graph),
        what: "graph: straight down on the topic map, orthographic",
    },
    ViewKey {
        code: KeyCode::Digit2,
        label: "2",
        button: "Timeline",
        request: ViewRequest::Preset(Preset::Timeline),
        what: "timeline: side-on with time up, orthographic",
    },
    ViewKey {
        code: KeyCode::Digit3,
        label: "3",
        button: "3D",
        request: ViewRequest::Preset(Preset::Free),
        what: "3D: the whole graph from above and to one side, perspective",
    },
    ViewKey {
        code: KeyCode::KeyN,
        label: "N",
        button: "Now",
        request: ViewRequest::Preset(Preset::Now),
        what: "now: the latest day's layer, from the 3D direction",
    },
    ViewKey {
        code: KeyCode::KeyR,
        label: "R",
        button: "Reset",
        request: ViewRequest::Reset,
        what: "reset: the 3D view of the whole graph, at the starting speed",
    },
    ViewKey {
        code: KeyCode::KeyF,
        label: "F",
        button: "Fit",
        request: ViewRequest::FitAll,
        what: "fit the whole graph, keeping the direction and projection",
    },
    ViewKey {
        code: KeyCode::KeyM,
        label: "M",
        button: "Metacontext",
        request: ViewRequest::NextMetacontext,
        what: "fit the next metacontext, keeping the direction and projection",
    },
    ViewKey {
        code: KeyCode::KeyT,
        label: "T",
        button: "Topic",
        request: ViewRequest::FocusedTopic,
        what: "fit the focused node's topic and its claims, keeping the direction and projection",
    },
];

/// One row of the help: the mouse action or the keys, and what it does.
pub type Row = (&'static str, &'static str);

/// What the mouse does, in the order the help lists it.
pub fn mouse_rows() -> Vec<Row> {
    vec![
        ("left click", LEFT_CLICK_WHAT),
        ("left drag", LEFT_DRAG_WHAT),
        ("right drag", "look around"),
        ("wheel", WHEEL_WHAT),
        ("Shift+wheel", SHIFT_WHEEL_WHAT),
    ]
}

/// What every key does, in the order the help lists them.
pub fn key_rows() -> Vec<Row> {
    let mut rows = vec![
        (
            "W A S D",
            "move along the view; in an orthographic view W and S zoom",
        ),
        ("Q E", "move down and up"),
    ];
    rows.extend(VIEW_KEYS.iter().map(|key| (key.label, key.what)));
    rows.extend([
        (CLEAR_FOCUS_KEY.1, CLEAR_FOCUS_WHAT),
        (BAR_KEY.1, BAR_KEY_WHAT),
        (EDGE_KEY.1, EDGE_KEY_WHAT),
        (MODE_KEY.1, MODE_KEY_WHAT),
        (PANEL_KEY.1, PANEL_KEY_WHAT),
        (LOG_KEY.1, LOG_KEY_WHAT),
        (HELP_KEY.1, HELP_KEY_WHAT),
        (BACK_KEY.1, BACK_KEY_WHAT),
        (SEARCH_KEY.1, SEARCH_KEY_WHAT),
        ("Enter", WALK_WHAT),
        ("Shift+Enter", WALK_BACK_WHAT),
        (GO_KEY.1, GO_KEY_WHAT),
        (RECALCULATE_KEY.1, RECALCULATE_KEY_WHAT),
    ]);
    rows
}

/// What the help says after the rows.
pub const RETARGET_NOTE: &str = "A view key during a change of view retargets it.";

/// The controls, for `--help` and the page's `?help`: the mouse, then the
/// keys.
pub fn help() -> String {
    let mut lines = vec!["In the window:".to_owned()];
    lines.extend(
        mouse_rows()
            .into_iter()
            .chain(key_rows())
            .map(|(keys, what)| row(keys, what)),
    );
    lines.push(String::new());
    lines.push(RETARGET_NOTE.to_owned());
    lines.join("\n")
}

/// What a left click does, for the help.
const LEFT_CLICK_WHAT: &str = "on a disc, or a node the side panel names: focus it; on the scene where no disc is: clear the focus";
/// What a left drag does, for the help.
const LEFT_DRAG_WHAT: &str =
    "pan: the scene follows the pointer; a drag never focuses or clears the focus";
/// What the wheel does, for the help.
const WHEEL_WHAT: &str = "zoom towards the pointer; over the bar or the side panel, scroll it";
/// What the wheel does with Shift held, for the help.
const SHIFT_WHEEL_WHAT: &str = "make the keys move the camera slower or faster";
/// What the bar's key does, for the help.
const BAR_KEY_WHAT: &str = "show or hide the bar of filters and views";
/// What the help key does, for the help.
const HELP_KEY_WHAT: &str = "show a cheatsheet of the controls, the colours and the concepts in the side panel, or return the panel to what it showed";
/// What Escape does, for the help.
const CLEAR_FOCUS_WHAT: &str =
    "clear the focus, close the bar's open list, or leave the search field";
/// What the panel's key does, for the help.
const PANEL_KEY_WHAT: &str = "show or hide the side panel of the focused node";
/// What the log's key does, for the help.
const LOG_KEY_WHAT: &str = "show the log of acts, advisories and pipeline runs in the side panel, or return it to the focused node";
/// What the back key does, for the help.
const BACK_KEY_WHAT: &str = "focus the node focused before this one";
/// What the search key does, for the help.
const SEARCH_KEY_WHAT: &str = "type into the search field, which then takes the keyboard";
/// What Enter and Shift+Enter do in the search field, for the help.
const WALK_WHAT: &str = "in the search field: focus the next hit";
const WALK_BACK_WHAT: &str = "in the search field: focus the hit before";
/// What the go key does, for the help.
const GO_KEY_WHAT: &str = "fit the current search hit's topic and its claims, as T does";

/// What the recalculate key does, for the help.
pub const RECALCULATE_KEY_WHAT: &str = "recalculate the topic map from the topics' embeddings and save it, as the bar's Recalculate layout button does; every topic moves";

/// What the edge toggle's key does, for the help.
const EDGE_KEY_WHAT: &str = "show or hide every claim-to-claim edge between visible nodes";
/// What the time mode's key does, for the help.
const MODE_KEY_WHAT: &str =
    "switch the time axis between recorded time and claim time, each claim over its timepoint";

/// One row as the help prints it: the keys in a column, then what they do.
pub fn row(keys: &str, what: &str) -> String {
    format!("  {keys:<12} {what}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn no_key_does_two_things() {
        // `?` is Shift and `/`, so a key is told apart with Shift held or not.
        let plain = |code: KeyCode| (code, false);
        let mut codes: Vec<(KeyCode, bool)> =
            MOVE_KEYS.iter().map(|&(code, _)| plain(code)).collect();
        codes.extend(VIEW_KEYS.iter().map(|key| plain(key.code)));
        codes.extend(
            [
                BAR_KEY.0,
                EDGE_KEY.0,
                MODE_KEY.0,
                CLEAR_FOCUS_KEY.0,
                PANEL_KEY.0,
                LOG_KEY.0,
                BACK_KEY.0,
                SEARCH_KEY.0,
                GO_KEY.0,
                RECALCULATE_KEY.0,
            ]
            .map(plain),
        );
        codes.push((HELP_KEY.0, true));
        codes.extend(WALK_KEYS.map(plain));
        let count = codes.len();
        codes.sort_by_key(|code| format!("{code:?}"));
        codes.dedup();
        assert_eq!(codes.len(), count);
    }

    #[test]
    fn question_mark_is_shift_and_slash_and_slash_alone_is_search() {
        let held = |keys: &[KeyCode]| {
            let mut input = ButtonInput::<KeyCode>::default();
            for &key in keys {
                input.press(key);
            }
            input
        };
        let slash = held(&[KeyCode::Slash]);
        assert!(search_pressed(&slash, false) && !help_pressed(&slash, false));
        for shift in [KeyCode::ShiftLeft, KeyCode::ShiftRight] {
            let question = held(&[shift, KeyCode::Slash]);
            assert!(help_pressed(&question, false) && !search_pressed(&question, false));
            assert!(help_pressed(&question, true), "both at once is one ?");
        }
        assert!(!help_pressed(&held(&[KeyCode::ShiftLeft]), false));
        // A layout with `?` elsewhere, and one with it on the `/` key.
        let elsewhere = held(&[KeyCode::ShiftLeft, KeyCode::Minus]);
        assert!(help_pressed(&elsewhere, true) && !search_pressed(&elsewhere, true));
        assert!(help_pressed(&slash, true) && !search_pressed(&slash, true));
    }

    /// A key going down, `repeat` or not, with the character it has on the
    /// keyboard's layout.
    pub(crate) fn typed(key_code: KeyCode, character: &str, repeat: bool) -> KeyboardInput {
        KeyboardInput {
            key_code,
            logical_key: Key::Character(character.into()),
            state: ButtonState::Pressed,
            text: Some(character.into()),
            repeat,
            window: bevy::ecs::entity::Entity::PLACEHOLDER,
        }
    }

    #[test]
    fn a_question_mark_is_any_key_that_types_one_once_as_it_goes_down() {
        assert!(question_mark_typed(&[typed(KeyCode::Minus, "?", false)]));
        assert!(question_mark_typed(&[
            typed(KeyCode::ShiftLeft, "a", false),
            typed(KeyCode::Slash, "?", false),
        ]));
        assert!(!question_mark_typed(&[typed(KeyCode::Slash, "/", false)]));
        assert!(
            !question_mark_typed(&[typed(KeyCode::Minus, "?", true)]),
            "a repeat"
        );
        let up = KeyboardInput {
            state: ButtonState::Released,
            ..typed(KeyCode::Minus, "?", false)
        };
        assert!(!question_mark_typed(&[up]));
        assert!(!question_mark_typed(&[]));
    }

    #[test]
    fn the_help_lists_the_mouse_then_every_key_each_as_one_row() {
        let help = help();
        let listed: Vec<&str> = help.lines().filter(|l| l.starts_with("  ")).collect();
        let rows: Vec<String> = mouse_rows()
            .into_iter()
            .chain(key_rows())
            .map(|(keys, what)| row(keys, what))
            .collect();
        assert_eq!(listed, rows);
        let names: Vec<&str> = mouse_rows().into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            [
                "left click",
                "left drag",
                "right drag",
                "wheel",
                "Shift+wheel"
            ]
        );
        assert!(help.ends_with(RETARGET_NOTE));
        assert!(help.contains(&row("?", HELP_KEY_WHAT)));
    }

    #[test]
    fn the_help_names_every_view_key() {
        let help = help();
        for key in &VIEW_KEYS {
            let line = row(key.label, key.what);
            assert!(help.contains(&line), "{line:?} missing");
            assert!(format!("{:?}", key.code).ends_with(key.label), "{key:?}");
        }
        assert!(help.contains(&row("H", "show or hide the bar of filters and views")));
        assert!(help.contains(&row("Escape", CLEAR_FOCUS_WHAT)));
        assert!(format!("{:?}", BAR_KEY.0).ends_with(BAR_KEY.1));
        for (key, what) in [
            (PANEL_KEY, PANEL_KEY_WHAT),
            (LOG_KEY, LOG_KEY_WHAT),
            (BACK_KEY, BACK_KEY_WHAT),
            (GO_KEY, GO_KEY_WHAT),
            (RECALCULATE_KEY, RECALCULATE_KEY_WHAT),
        ] {
            assert!(help.contains(&row(key.1, what)), "{what}");
            assert!(format!("{:?}", key.0).ends_with(key.1), "{key:?}");
        }
        assert!(help.contains(&row("/", SEARCH_KEY_WHAT)));
        assert_eq!(SEARCH_KEY.0, KeyCode::Slash);
        assert!(help.contains(&row("Enter", WALK_WHAT)));
        assert!(help.contains(&row("Shift+Enter", WALK_BACK_WHAT)));
        assert!(help.contains(&row("L", EDGE_KEY_WHAT)));
        assert!(format!("{:?}", EDGE_KEY.0).ends_with(EDGE_KEY.1));
        assert!(help.contains(&row("C", MODE_KEY_WHAT)));
        assert!(format!("{:?}", MODE_KEY.0).ends_with(MODE_KEY.1));
        let buttons: Vec<&str> = VIEW_KEYS.iter().map(|key| key.button).collect();
        assert_eq!(
            buttons,
            [
                "Graph",
                "Timeline",
                "3D",
                "Now",
                "Reset",
                "Fit",
                "Metacontext",
                "Topic"
            ]
        );
    }
}
