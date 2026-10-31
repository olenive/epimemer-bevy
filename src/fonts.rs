//! The font every text in the window is drawn in.
//!
//! Bevy's built-in font covers ASCII only, and topic names and claims may
//! carry any character, so the bar and the labels are drawn in Inter, an
//! OFL font with full Latin coverage, kept in `assets/fonts/` with its
//! licence and a note of where it came from. It is loaded through Bevy's
//! asset server from the `assets/` directory, which under `cargo run` is the
//! one at the crate's root, and in a browser the one beside the page, which
//! trunk copies there (`index.html`) and the asset server fetches.
//!
//! Every text that appears with Bevy's default font, the bar's and the
//! labels' alike, is switched to Inter in the same update, before it is
//! laid out, so the bar's small bundles carry no font of their own. Should
//! the file fail to load, as when the binary runs away from the crate's
//! directory, the error is logged once and every text goes back to Bevy's
//! built-in font, which still draws ASCII.

use bevy::prelude::*;
use bevy::ui::UiSystems;

/// The font, within the assets directory.
pub const UI_FONT_PATH: &str = "fonts/Inter-Regular.ttf";

/// The font the texts are drawn in, and whether it failed to load.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct UiFont {
    pub handle: Handle<Font>,
    pub failed: bool,
}

/// Loads [`UI_FONT_PATH`] and puts every text in it.
pub struct FontPlugin;

impl Plugin for FontPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, load_ui_font).add_systems(
            PostUpdate,
            (fall_back_on_failure, use_ui_font)
                .chain()
                .in_set(UiSystems::Prepare),
        );
    }
}

/// The font a text should be drawn in: the UI font, or Bevy's own once the
/// UI font has failed.
pub fn font_to_use(font: &UiFont) -> Handle<Font> {
    if font.failed {
        Handle::default()
    } else {
        font.handle.clone()
    }
}

/// Whether a text is in Bevy's default font.
fn is_default(font: &TextFont) -> bool {
    matches!(&font.font, FontSource::Handle(handle) if *handle == Handle::default())
}

fn load_ui_font(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(UiFont {
        handle: assets.load(UI_FONT_PATH),
        failed: false,
    });
}

/// Switches every text that appeared in Bevy's default font to the UI font.
fn use_ui_font(font: Res<UiFont>, mut texts: Query<&mut TextFont, Added<TextFont>>) {
    let handle = font_to_use(&font);
    if handle == Handle::default() {
        return;
    }
    for mut text in &mut texts {
        if is_default(&text) {
            text.font = FontSource::Handle(handle.clone());
        }
    }
}

/// Once the UI font has failed to load, says so and puts every text in it
/// back in Bevy's default font.
fn fall_back_on_failure(
    mut font: ResMut<UiFont>,
    assets: Res<AssetServer>,
    mut texts: Query<&mut TextFont>,
) {
    if font.failed || !assets.load_state(&font.handle).is_failed() {
        return;
    }
    error!(
        "cannot load the font {UI_FONT_PATH} from the assets directory; drawing every text in Bevy's built-in font, which covers ASCII only"
    );
    font.failed = true;
    for mut text in &mut texts {
        if matches!(&text.font, FontSource::Handle(handle) if *handle == font.handle) {
            text.font = FontSource::Handle(Handle::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_in_the_default_font_is_switched_once_it_appears() {
        let mut app = App::new();
        let handle: Handle<Font> =
            bevy::asset::uuid_handle!("5a1e9f3c-0000-4000-8000-000000000001");
        app.add_plugins(MinimalPlugins)
            .insert_resource(UiFont {
                handle: handle.clone(),
                failed: false,
            })
            .add_systems(Update, use_ui_font);
        let plain = app.world_mut().spawn(TextFont::default()).id();
        let own = app
            .world_mut()
            .spawn(TextFont {
                font: FontSource::SansSerif,
                ..default()
            })
            .id();
        app.update();
        let font = |app: &App, e: Entity| app.world().get::<TextFont>(e).unwrap().font.clone();
        assert_eq!(font(&app, plain), FontSource::Handle(handle.clone()));
        assert_eq!(
            font(&app, own),
            FontSource::SansSerif,
            "a chosen font is kept"
        );
        assert_eq!(
            font_to_use(&UiFont {
                handle: handle.clone(),
                failed: true
            }),
            Handle::default()
        );
        assert_eq!(
            font_to_use(&UiFont {
                handle: handle.clone(),
                failed: false
            }),
            handle
        );
    }
}
