//! The bar's colours and the small bundles its controls are made of.
//!
//! Every text in the bar is made in Bevy's default font and drawn in the
//! UI font, Inter, which `fonts.rs` puts every text in as it appears. Should
//! Inter fail to load, the texts fall back to Bevy's built-in font, which
//! covers ASCII only, so the bar's own words stay within ASCII: a checkbox
//! is a drawn square, not a glyph.

use bevy::prelude::*;

/// Behind the bar and the collapsed strip: the scene's dark blue, a little
/// lighter and nearly opaque, so the bar reads as a panel over the scene.
pub const BAR: Color = Color::srgba(0.05, 0.06, 0.09, 0.92);
/// Behind an open picker. Opaque: the bar's own controls are under an open
/// list, and Bevy UI lets even a nearly opaque colour show them through
/// far more than its alpha says.
pub const PANEL: Color = Color::srgb(0.07, 0.08, 0.12);
pub const PANEL_EDGE: Color = Color::srgb(0.22, 0.25, 0.33);
/// A button, at rest, under the pointer, and chosen.
pub const BUTTON: Color = Color::srgb(0.13, 0.15, 0.2);
pub const BUTTON_HOVER: Color = Color::srgb(0.2, 0.23, 0.31);
pub const SELECTED: Color = Color::srgb(0.17, 0.32, 0.55);
pub const SELECTED_HOVER: Color = Color::srgb(0.22, 0.39, 0.65);
/// A button that does nothing yet.
pub const DISABLED: Color = Color::srgb(0.09, 0.1, 0.13);
/// A row of a picker's list, at rest: no box, only its text.
pub const ROW: Color = Color::NONE;
/// The slider.
pub const RAIL: Color = Color::srgb(0.2, 0.22, 0.28);
pub const ACCENT: Color = Color::srgb(0.36, 0.6, 0.95);
pub const HANDLE: Color = Color::srgb(0.85, 0.88, 0.95);
pub const HANDLE_EDGE: Color = Color::srgb(0.3, 0.45, 0.75);
/// Text: ordinary, a caption, faint, and an error.
pub const TEXT: Color = Color::srgb(0.87, 0.89, 0.93);
pub const CAPTION: Color = Color::srgb(0.52, 0.57, 0.67);
pub const FAINT: Color = Color::srgb(0.4, 0.43, 0.5);
pub const ERROR: Color = Color::srgb(1.0, 0.5, 0.45);
/// The tint of a row that asks for a person, such as an advisory the agent
/// was asked to raise with the user.
pub const NOTIFY: Color = Color::srgba(1.0, 0.5, 0.45, 0.14);

/// The size of every text in the bar, in logical pixels.
pub const FONT_SIZE: f32 = 13.0;

/// A text in the bar's font, in a colour.
pub fn text(words: impl Into<String>, colour: Color) -> impl Bundle {
    (
        Text::new(words),
        TextFont {
            font_size: FontSize::Px(FONT_SIZE),
            ..default()
        },
        TextColor(colour),
        TextLayout::no_wrap(),
    )
}

/// The node of a button: its text in the middle, a little room around it.
pub fn button_node() -> Node {
    Node {
        padding: UiRect::axes(px(7), px(3)),
        align_items: AlignItems::Center,
        column_gap: px(5),
        border_radius: BorderRadius::all(px(3)),
        flex_shrink: 0.0,
        ..default()
    }
}

/// The node of a row in a picker's list: the list's whole width.
pub fn row_node() -> Node {
    Node {
        padding: UiRect::axes(px(6), px(2)),
        align_items: AlignItems::Center,
        column_gap: px(6),
        width: percent(100),
        flex_shrink: 0.0,
        border_radius: BorderRadius::all(px(2)),
        ..default()
    }
}

/// A group of controls with a caption before them.
pub fn group_node() -> Node {
    Node {
        align_items: AlignItems::Center,
        column_gap: px(4),
        flex_shrink: 0.0,
        ..default()
    }
}

/// A small square of a colour, such as a kind's beside its toggle.
pub fn swatch(colour: Color) -> impl Bundle {
    (
        Node {
            width: px(8),
            height: px(8),
            border_radius: BorderRadius::all(px(2)),
            ..default()
        },
        BackgroundColor(colour),
        Pickable::IGNORE,
    )
}

/// A button's colour: chosen or not, under the pointer or not, or unable
/// to do anything. `rest` is its colour at rest, a row's being clear.
pub fn button_colour(rest: Color, selected: bool, hovered: bool, disabled: bool) -> Color {
    match (disabled, selected, hovered) {
        (true, _, _) => DISABLED,
        (false, true, false) => SELECTED,
        (false, true, true) => SELECTED_HOVER,
        (false, false, true) => BUTTON_HOVER,
        (false, false, false) => rest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_button_looks_disabled_whatever_else_holds() {
        for selected in [false, true] {
            for hovered in [false, true] {
                assert_eq!(button_colour(BUTTON, selected, hovered, true), DISABLED);
            }
        }
        assert_eq!(button_colour(ROW, false, false, false), ROW);
        assert_eq!(button_colour(ROW, false, true, false), BUTTON_HOVER);
        assert_eq!(button_colour(BUTTON, true, false, false), SELECTED);
        assert_eq!(button_colour(BUTTON, true, true, false), SELECTED_HOVER);
    }
}
