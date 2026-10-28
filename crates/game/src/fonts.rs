//! The UI font (art pass slice 5, owner decision 2): Inter 4.1
//! (`assets/fonts/InterVariable.ttf`, OFL-1.1, its licence beside it as
//! `assets/fonts/OFL-Inter.txt`) replaces Bevy's built-in Fira Mono subset
//! as the font every `TextFont` without a handle draws with.
//!
//! [`install_ui_font`] runs in `Startup`: it reads the file with `std::fs`
//! (no asset load, so nothing waits on the asset server) and inserts it
//! into `Assets<Font>` at `AssetId::default()`, the id `bevy_text`'s
//! `TextPlugin` put Fira Mono at and the id `TextFont::default()`'s handle
//! points to. Bevy 0.19.1's `load_font_assets_into_font_collection`
//! (`PostUpdate`) registers each font id once, in a `Local` set, so the
//! replacement must land before the first `PostUpdate`; `Startup` does
//! (`install_ui_font_replaces_the_default_before_the_first_post_update`).
//! A missing, unreadable or unparsable file is a `warn!` and Fira Mono
//! stays, so the game still shows text: `Font::from_bytes` never parses,
//! so the bytes are first registered in a throwaway `FontCx` the way
//! `load_font_assets_into_font_collection` will register them, and a file
//! that yields no font family (empty, truncated, not a font) is refused.
//!
//! Every HUD, overlay and menu text takes its `TextFont` from
//! [`hud_font`], which turns on Inter's tabular figures (`tnum`): Inter's
//! default figures are proportional ('1' is 5.7 px and '4' 9.0 px at
//! 14 px), so a stock or a countdown that changed by a digit would move
//! every readout beside it; with `tnum` every digit is 1328/2048 em
//! (9.08 px at 14 px) and a number keeps its width while its digit count
//! does.
//!
//! The HUD's fit tests measure text with Inter's real advance widths (the
//! test-only `measure` module, `skrifa` 0.42.1, the version parley
//! already locks): the sum of the nominal advances at the default
//! instance (weight 400, the weight `TextFont::default()` asks for), each
//! digit through Inter's `tnum` substitution as [`hud_font`] draws it,
//! with no kerning, which in Inter narrows far more pairs than it widens.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::text::{FontCx, FontFeatureTag};

/// The UI font under the assets directory.
pub const UI_FONT_FILE: &str = "fonts/InterVariable.ttf";

/// Where [`install_ui_font`] reads the UI font from.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct UiFontFile(pub PathBuf);

/// Installs Inter as the default font in `Startup`.
pub struct FontsPlugin {
    /// The resolved assets directory (`Cli::resolve_assets_dir`).
    pub assets: PathBuf,
}

impl Plugin for FontsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(UiFontFile(ui_font_path(&self.assets)))
            .add_systems(Startup, install_ui_font);
    }
}

/// The UI font's path under `assets`.
pub fn ui_font_path(assets: &Path) -> PathBuf {
    assets.join(UI_FONT_FILE)
}

/// The `TextFont` of every HUD, overlay and menu text: the default font
/// (Inter once [`install_ui_font`] ran) at `px` logical px with tabular
/// figures, so a changing number keeps its width.
pub fn hud_font(px: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(px),
        font_features: [FontFeatureTag::TABULAR_FIGURES].into(),
        ..default()
    }
}

/// Read [`UiFontFile`] and put it in `Assets<Font>` at `AssetId::default()`
/// in place of Bevy's Fira Mono. Logs `ui font: <path>` once installed, or
/// warns and keeps Fira Mono when the file cannot be read or holds no
/// font family Bevy could register.
pub fn install_ui_font(file: Res<UiFontFile>, fonts: Option<ResMut<Assets<Font>>>) {
    let Some(mut fonts) = fonts else {
        return;
    };
    let path = file.0.display();
    let bytes = match std::fs::read(&file.0) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!("ui font {path}: {err}; keeping Bevy's built-in FiraMono");
            return;
        }
    };
    let font = Font::from_bytes(bytes);
    // `load_font_assets_into_font_collection` registers the asset's bytes
    // with fontique, which skips data it cannot parse (no family, no
    // alias), and the default `TextFont` would then name a family that
    // does not exist. Register them the same way first.
    if FontCx::default()
        .collection
        .register_fonts(font.data.clone(), None)
        .is_empty()
    {
        warn!("ui font {path}: not a font file; keeping Bevy's built-in FiraMono");
        return;
    }
    match fonts.insert(AssetId::default(), font) {
        Ok(()) => info!("ui font: {path}"),
        Err(err) => warn!("ui font {path}: {err}; keeping Bevy's built-in FiraMono"),
    }
}

/// Text widths with Inter's advances, for the HUD fit tests.
#[cfg(test)]
pub(crate) mod measure {
    use std::sync::OnceLock;

    use std::collections::BTreeMap;

    use skrifa::instance::{LocationRef, Size};
    use skrifa::raw::TableProvider;
    use skrifa::raw::tables::gsub::{SingleSubst, SubstitutionSubtables};
    use skrifa::raw::types::Tag;
    use skrifa::{FontRef, GlyphId, MetadataProvider};

    /// The repository's `assets/fonts/InterVariable.ttf`, read once.
    pub fn inter() -> &'static [u8] {
        static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
        BYTES.get_or_init(|| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets")
                .join(super::UI_FONT_FILE);
            std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        })
    }

    /// Inter's `tnum` feature as a glyph map: every single substitution of
    /// the GSUB lookups the feature names (Inter's figures map to their
    /// tabular forms there).
    pub fn tnum_map() -> &'static BTreeMap<GlyphId, GlyphId> {
        static MAP: OnceLock<BTreeMap<GlyphId, GlyphId>> = OnceLock::new();
        MAP.get_or_init(|| {
            let font = FontRef::new(inter()).expect("Inter parses");
            let gsub = font.gsub().expect("Inter has GSUB");
            let features = gsub.feature_list().expect("a feature list");
            let lookups = gsub.lookup_list().expect("a lookup list");
            let mut map = BTreeMap::new();
            for record in features.feature_records() {
                if record.feature_tag() != Tag::new(b"tnum") {
                    continue;
                }
                let feature = record
                    .feature(features.offset_data())
                    .expect("the tnum feature");
                for index in feature.lookup_list_indices() {
                    let lookup = lookups
                        .lookups()
                        .get(usize::from(index.get()))
                        .expect("a tnum lookup");
                    let Ok(SubstitutionSubtables::Single(subtables)) = lookup.subtables() else {
                        continue;
                    };
                    for subtable in subtables.iter() {
                        match subtable.expect("a single substitution") {
                            SingleSubst::Format1(t) => {
                                let delta = i32::from(t.delta_glyph_id());
                                for g in t.coverage().expect("coverage").iter() {
                                    let to = (i32::from(g.to_u16()) + delta).rem_euclid(65536);
                                    map.insert(
                                        GlyphId::from(g),
                                        GlyphId::new(u32::try_from(to).unwrap()),
                                    );
                                }
                            }
                            SingleSubst::Format2(t) => {
                                let coverage = t.coverage().expect("coverage");
                                for (g, to) in coverage.iter().zip(t.substitute_glyph_ids()) {
                                    map.insert(GlyphId::from(g), GlyphId::from(to.get()));
                                }
                            }
                        }
                    }
                }
            }
            map
        })
    }

    /// The width of `text` on one line at `font_px`, logical px, as
    /// `super::hud_font` draws it: the sum of Inter's advance widths at its
    /// default instance, each glyph `tnum` substitutes (the figures) taken
    /// in its tabular form. Panics on a character Inter has no glyph for
    /// (it would draw as a fallback).
    pub fn text_width_px(text: &str, font_px: f32) -> f32 {
        let font = FontRef::new(inter()).expect("Inter parses");
        let charmap = font.charmap();
        let metrics = font.glyph_metrics(Size::new(font_px), LocationRef::default());
        let tnum = tnum_map();
        text.chars()
            .map(|c| {
                let glyph = charmap
                    .map(c)
                    .unwrap_or_else(|| panic!("Inter has no glyph for {c:?} in {text:?}"));
                let glyph = tnum.get(&glyph).copied().unwrap_or(glyph);
                metrics
                    .advance_width(glyph)
                    .unwrap_or_else(|| panic!("no advance for {c:?}"))
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::measure::text_width_px;
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::text::{FontCx, load_font_assets_into_font_collection};

    /// An app with the asset server, `Assets<Font>` holding Fira Mono at the
    /// default id and `bevy_text`'s font registration in `PostUpdate`, as
    /// `TextPlugin` sets them up (without its layout and atlas systems).
    fn text_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Font>()
            .init_resource::<FontCx>()
            .add_systems(PostUpdate, load_font_assets_into_font_collection);
        app.world_mut()
            .resource_mut::<Assets<Font>>()
            .insert(
                AssetId::default(),
                Font::from_bytes(bevy::text::DEFAULT_FONT_DATA.to_vec()),
            )
            .unwrap();
        app
    }

    fn has_family(app: &mut App, name: &str) -> bool {
        app.world_mut()
            .resource_mut::<FontCx>()
            .collection
            .family_id(name)
            .is_some()
    }

    fn repo_assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    /// One update: `Startup` puts Inter at the default id before the
    /// first `PostUpdate` registers it, so the default family is Inter and
    /// Fira Mono is never registered. Without the plugin (the positive
    /// control) Fira Mono is.
    #[test]
    fn install_ui_font_replaces_the_default_before_the_first_post_update() {
        let mut control = text_app();
        control.update();
        assert!(has_family(&mut control, "Fira Mono"), "the control");
        assert!(!has_family(&mut control, "Inter Variable"));

        let mut app = text_app();
        app.add_plugins(FontsPlugin {
            assets: repo_assets(),
        });
        app.update();
        assert!(has_family(&mut app, "Inter Variable"));
        assert!(!has_family(&mut app, "Fira Mono"));
        let fonts = app.world().resource::<Assets<Font>>();
        let installed = fonts.get(AssetId::default()).expect("the default font");
        assert_eq!(installed.data.data(), measure::inter());
    }

    /// A missing file keeps Fira Mono (a `warn!`, no panic).
    #[test]
    fn a_missing_font_file_keeps_fira_mono() {
        let mut app = text_app();
        app.add_plugins(FontsPlugin {
            assets: PathBuf::from("/nonexistent/eonmark-assets"),
        });
        app.update();
        assert!(has_family(&mut app, "Fira Mono"));
        assert!(!has_family(&mut app, "Inter Variable"));
    }

    /// A file that reads but is no font (empty, truncated or garbage)
    /// keeps Fira Mono too: `Font::from_bytes` never parses, so installing
    /// such bytes would replace Fira Mono with a font Bevy then registers
    /// no family for, and every default `TextFont` would draw nothing.
    #[test]
    fn an_unparsable_font_file_keeps_fira_mono() {
        let truncated = measure::inter()[..4096].to_vec();
        for (name, bytes) in [
            ("empty", Vec::new()),
            ("garbage", b"not a font file at all".repeat(64)),
            ("truncated", truncated),
        ] {
            let dir = std::env::temp_dir()
                .join(format!("eonmark-font-test-{}-{name}", std::process::id()));
            let file = ui_font_path(&dir);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, &bytes).unwrap();
            let mut app = text_app();
            app.add_plugins(FontsPlugin {
                assets: dir.clone(),
            });
            app.update();
            std::fs::remove_dir_all(&dir).unwrap();
            assert!(has_family(&mut app, "Fira Mono"), "{name}");
            assert!(!has_family(&mut app, "Inter Variable"), "{name}");
            let fonts = app.world().resource::<Assets<Font>>();
            let installed = fonts.get(AssetId::default()).expect("the default font");
            assert_eq!(
                installed.data.data(),
                bevy::text::DEFAULT_FONT_DATA,
                "{name}"
            );
        }
    }

    /// Every text the HUD, the outcome overlay and the main menu spawn
    /// asks for Inter's tabular figures (`tnum`), so a number that changes
    /// keeps its width and nothing beside it moves.
    #[test]
    fn every_hud_text_uses_tabular_figures() {
        use bevy::text::{FontFeatureTag, FontFeatures};
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let rules = rules::Rules::load(&data).expect("rules load");
        let strings = rules::strings::Strings::load(&data, "en").expect("en.ron loads");
        let (setup, _) = sim::scenarios::economy_walkthrough(&rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(crate::economy_hud::UiStrings(strings))
            .add_systems(
                Startup,
                (
                    crate::hud::spawn_hud,
                    crate::economy_hud::spawn_economy_hud,
                    crate::age_hud::spawn_age_hud,
                    crate::command_card::spawn_card,
                    crate::markers::spawn_annex_readout,
                    crate::outcome::spawn_outcome_overlay,
                    crate::menu::main_menu::spawn_main_menu,
                )
                    .chain(),
            );
        app.world_mut()
            .insert_non_send(crate::sim_driver::SimHandle::from_setup(setup, rules));
        app.update();
        let tnum = FontFeatures::from([FontFeatureTag::TABULAR_FIGURES]);
        let mut texts = app
            .world_mut()
            .query_filtered::<(Entity, &TextFont), With<Text>>();
        let texts: Vec<(Entity, TextFont)> = texts
            .iter(app.world())
            .map(|(e, f)| (e, f.clone()))
            .collect();
        assert!(texts.len() > 15, "{} texts", texts.len());
        for (e, font) in texts {
            assert_eq!(
                font.font_features,
                tnum,
                "text {e} ({:?})",
                app.world().get::<Text>(e).map(|t| t.0.clone())
            );
        }
    }

    /// The measure is Inter's, not a monospace one: a narrow glyph is
    /// narrower than a wide one, a space adds width, and the HUD's longest
    /// lines (`command_card::INFO_WIDTH_PX`, `CARD_BUTTON_PX`) fit with
    /// room to spare. The fit tests in `command_card` and `age_hud` use
    /// this measure.
    #[test]
    fn hud_text_fits_with_inter_advances() {
        let (i, w) = (text_width_px("i", 14.0), text_width_px("W", 14.0));
        assert!(i > 0.0 && w > 2.0 * i, "i={i} W={w}");
        // Figures are tabular, as `hud_font` draws them: one width each,
        // 1328/2048 em, wider than any of Inter's proportional figures.
        let one = text_width_px("1", 14.0);
        for d in '0'..='9' {
            assert!(
                (text_width_px(&d.to_string(), 14.0) - one).abs() < 1e-4,
                "{d}"
            );
        }
        assert!((one - 14.0 * 1328.0 / 2048.0).abs() < 1e-3, "{one}");
        assert!(super::measure::tnum_map().len() >= 10);
        assert!(text_width_px("a b", 14.0) > text_width_px("ab", 14.0));
        // Twice the size, twice the width.
        let one = text_width_px("Lumber", 12.0);
        assert!((text_width_px("Lumber", 24.0) - 2.0 * one).abs() < 1e-3);
        let queue = "Queue: Statecraft III 99%, Letters III, Trade III, Arms III, Scribe";
        let panel = text_width_px(queue, crate::hud::HUD_FONT_PX);
        assert!(
            panel < crate::command_card::INFO_WIDTH_PX,
            "{panel} px of {}",
            crate::command_card::INFO_WIDTH_PX
        );
        let button = text_width_px("Needs Statecraft II", crate::command_card::CARD_FONT_PX);
        assert!(
            button < crate::command_card::CARD_BUTTON_PX.x,
            "{button} px"
        );
    }
}
