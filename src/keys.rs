//! The key map: every key and button the window answers to, in one place,
//! and the help text made from it.
//!
//! The bar's view buttons (`bar.rs`) are made from [`VIEW_KEYS`] too, and
//! ask the camera for the same [`ViewRequest`] the key does, so a key and
//! its button take one path. No key reaches the camera, the focus, the bar
//! or the side panel while a text field in the bar is taking the keyboard,
//! except Escape, Enter and Shift+Enter, which the search field reads.

use bevy::input::keyboard::KeyCode;

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

/// The key that focuses the node focused before the current one, from the
/// focus history, and its name in the help: `B`, for back.
pub const BACK_KEY: (KeyCode, &str) = (KeyCode::KeyB, "B");

/// The key that puts the keyboard into the bar's search field, and its name
/// in the help: `/`, as in many programs that search.
pub const SEARCH_KEY: (KeyCode, &str) = (KeyCode::Slash, "/");

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

/// The controls, for `--help`.
pub fn help() -> String {
    let mut lines = vec![
        "In the window:".to_owned(),
        row("right drag", "look around"),
        row(
            "W A S D",
            "move along the view; in an orthographic view W and S zoom",
        ),
        row("Q E", "move down and up"),
        row(
            "scroll",
            "move slower or faster; over the bar or the side panel, scroll it",
        ),
    ];
    lines.extend(VIEW_KEYS.iter().map(|key| row(key.label, key.what)));
    lines.push(row(
        "left click",
        "on a disc, or a node the side panel names: focus it; on the scene where no disc is: clear the focus",
    ));
    lines.push(row(CLEAR_FOCUS_KEY.1, CLEAR_FOCUS_WHAT));
    lines.push(row(BAR_KEY.1, "show or hide the bar of filters and views"));
    lines.push(row(EDGE_KEY.1, EDGE_KEY_WHAT));
    lines.push(row(MODE_KEY.1, MODE_KEY_WHAT));
    lines.push(row(PANEL_KEY.1, PANEL_KEY_WHAT));
    lines.push(row(BACK_KEY.1, BACK_KEY_WHAT));
    lines.push(row(SEARCH_KEY.1, SEARCH_KEY_WHAT));
    lines.push(row("Enter", WALK_WHAT));
    lines.push(row("Shift+Enter", WALK_BACK_WHAT));
    lines.push(row(GO_KEY.1, GO_KEY_WHAT));
    lines.push(row(RECALCULATE_KEY.1, RECALCULATE_KEY_WHAT));
    lines.push(String::new());
    lines.push("A view key during a change of view retargets it.".to_owned());
    lines.join("\n")
}

/// What Escape does, for the help.
const CLEAR_FOCUS_WHAT: &str =
    "clear the focus, close the bar's open list, or leave the search field";
/// What the panel's key does, for the help.
const PANEL_KEY_WHAT: &str = "show or hide the side panel of the focused node";
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

fn row(keys: &str, what: &str) -> String {
    format!("  {keys:<12} {what}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_does_two_things() {
        let mut codes: Vec<KeyCode> = MOVE_KEYS.iter().map(|&(code, _)| code).collect();
        codes.extend(VIEW_KEYS.iter().map(|key| key.code));
        codes.push(BAR_KEY.0);
        codes.push(EDGE_KEY.0);
        codes.push(MODE_KEY.0);
        codes.push(CLEAR_FOCUS_KEY.0);
        codes.extend([
            PANEL_KEY.0,
            BACK_KEY.0,
            SEARCH_KEY.0,
            GO_KEY.0,
            RECALCULATE_KEY.0,
        ]);
        codes.extend(WALK_KEYS);
        let count = codes.len();
        codes.sort_by_key(|code| format!("{code:?}"));
        codes.dedup();
        assert_eq!(codes.len(), count);
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
