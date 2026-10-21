//! Schema for `data/visuals.ron`: how each unit kind is drawn.
//!
//! The simulation never reads this file; it is loaded by `Rules::load` so
//! that it is validated with everything else (every unit kind must have a
//! visual) and so that it is part of `rules_hash`. The game crate maps a
//! [`Visual`] to a Bevy mesh and material in `present.rs`.
//!
//! M2 ships primitives only. [`Visual::Scene`] is parsed and validated so the
//! M7 glTF switch is a data change, but the presenter ignores it until then.
//! Every number is an integer: fields ending in `_x100` are hundredths and
//! `yaw_deg` is whole degrees, following `docs/DATA_FORMAT.md`.

use crate::{Error, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A built-in mesh shape. The presenter sizes it from the unit kind's
/// `radius_tiles_x100` so a visual carries no dimensions of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    /// Upright capsule (the M2 look for every unit).
    Capsule,
    /// Axis-aligned box.
    Cuboid,
    /// Upright cylinder.
    Cylinder,
    /// Sphere.
    Sphere,
}

/// How a kind is drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Visual {
    /// One shared primitive mesh, tinted with the owner's team colour.
    Primitive(Primitive),
    /// A glTF scene under `assets/` (M7). Parsed and validated at M2, unused
    /// by the presenter until M7.
    Scene {
        /// Path relative to `assets/`, ending in `.glb` or `.gltf`.
        path: String,
        /// Uniform scale in hundredths (`100` is 1.0).
        scale_x100: u32,
        /// Vertical offset in tiles, hundredths (`-5` sinks the model 5 cm).
        y_offset_tiles_x100: i32,
        /// Extra yaw applied to the model so it faces +X at rest, degrees.
        yaw_deg: i32,
    },
}

/// Contents of `data/visuals.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Visuals {
    /// Visual per unit kind, keyed by the kind's `id` from `units.ron`.
    pub units: BTreeMap<String, Visual>,
}

/// Upper bound on `scale_x100` (a 100x model is a data error).
pub const MAX_SCALE_X100: u32 = 10_000;

impl Visuals {
    /// Validate against the loaded unit kinds: every kind has a visual, every
    /// key names a kind, every `Scene` is well formed. Errors name `path`
    /// and the field (`units["yeoman"]` or `units["yeoman"].scale_x100`).
    pub fn validate(&self, path: &Path, kinds: &[UnitKind]) -> Result<(), Error> {
        for kind in kinds {
            if !self.units.contains_key(&kind.id) {
                return Err(Error::invalid(
                    path,
                    &format!("units[{:?}]", kind.id),
                    "missing: every unit kind in units.ron needs a visual",
                ));
            }
        }
        for (id, visual) in &self.units {
            let field = |name: &str| {
                if name.is_empty() {
                    format!("units[{id:?}]")
                } else {
                    format!("units[{id:?}].{name}")
                }
            };
            if !kinds.iter().any(|k| &k.id == id) {
                return Err(Error::invalid(
                    path,
                    &field(""),
                    "no unit kind with this id in units.ron",
                ));
            }
            if let Visual::Scene {
                path: scene,
                scale_x100,
                yaw_deg,
                ..
            } = visual
            {
                if !(scene.ends_with(".glb") || scene.ends_with(".gltf")) {
                    return Err(Error::invalid(
                        path,
                        &field("path"),
                        format!("{scene:?} must end in .glb or .gltf"),
                    ));
                }
                if *scale_x100 == 0 || *scale_x100 > MAX_SCALE_X100 {
                    return Err(Error::invalid(
                        path,
                        &field("scale_x100"),
                        format!("must be in 1..={MAX_SCALE_X100}"),
                    ));
                }
                if !(-360..=360).contains(yaw_deg) {
                    return Err(Error::invalid(
                        path,
                        &field("yaw_deg"),
                        "must be within -360..=360",
                    ));
                }
            }
        }
        Ok(())
    }

    /// The visual for a unit kind id, if listed.
    pub fn get(&self, id: &str) -> Option<&Visual> {
        self.units.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> Vec<UnitKind> {
        vec![UnitKind {
            id: "yeoman".into(),
            name: "Yeoman".into(),
            speed_tiles_per_s_x100: 180,
            radius_tiles_x100: 35,
            arrive_radius_tiles_x100: 25,
            arrive_slowdown_radius_tiles_x100: 50,
            waypoint_radius_tiles_x100: 50,
            separation_tiles_x100: 10,
            return_to_post_radius_tiles_x100: 100,
            class: crate::UnitClass::Civilian,
            cost: BTreeMap::new(),
            ramp: crate::UnitRamp::Flat,
            pop: 1,
            train_time_ds: 120,
            trainer: "town".into(),
            builder: true,
            vision_tiles: 8,
        }]
    }

    fn parse(text: &str) -> Result<Visuals, Error> {
        crate::parse_ron::<Visuals>(Path::new("v.ron"), text)
    }

    #[test]
    fn primitive_parses_and_validates() {
        let v = parse(r#"(units: { "yeoman": Primitive(Capsule) })"#).unwrap();
        v.validate(Path::new("v.ron"), &kinds()).unwrap();
        assert_eq!(
            v.get("yeoman"),
            Some(&Visual::Primitive(Primitive::Capsule))
        );
        assert!(v.get("scribe").is_none());
    }

    #[test]
    fn scene_parses_and_validates() {
        let v = parse(
            r#"(units: { "yeoman": Scene(path: "models/yeoman.glb", scale_x100: 100, y_offset_tiles_x100: -5, yaw_deg: 90) })"#,
        )
        .unwrap();
        v.validate(Path::new("v.ron"), &kinds()).unwrap();
    }

    #[test]
    fn missing_kind_names_file_and_field() {
        let v = parse("(units: {})").unwrap();
        let err = v
            .validate(Path::new("v.ron"), &kinds())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("v.ron") && err.contains(r#"units["yeoman"]"#),
            "{err}"
        );
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let v = parse(r#"(units: { "yeoman": Primitive(Capsule), "dragon": Primitive(Sphere) })"#)
            .unwrap();
        let err = v
            .validate(Path::new("v.ron"), &kinds())
            .unwrap_err()
            .to_string();
        assert!(err.contains(r#"units["dragon"]"#), "{err}");
    }

    #[test]
    fn bad_scene_fields_are_rejected_with_field() {
        let bad = [
            (
                r#"(units: { "yeoman": Scene(path: "models/yeoman.obj", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0) })"#,
                "path",
            ),
            (
                r#"(units: { "yeoman": Scene(path: "a.glb", scale_x100: 0, y_offset_tiles_x100: 0, yaw_deg: 0) })"#,
                "scale_x100",
            ),
            (
                r#"(units: { "yeoman": Scene(path: "a.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 400) })"#,
                "yaw_deg",
            ),
        ];
        for (text, field) in bad {
            let v = parse(text).unwrap();
            let err = v
                .validate(Path::new("v.ron"), &kinds())
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(&format!(r#"units["yeoman"].{field}"#)),
                "{err}"
            );
        }
    }

    #[test]
    fn scene_rejects_unknown_fields() {
        let err = parse(
            r#"(units: { "yeoman": Scene(path: "a.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, tint: 3) })"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("tint"), "{err}");
    }

    #[test]
    fn unknown_field_and_float_are_parse_errors() {
        assert!(parse(r#"(units: { "yeoman": Primitive(Capsule) }, extra: 1)"#).is_err());
        assert!(
            parse(
                r#"(units: { "yeoman": Scene(path: "a.glb", scale_x100: 1.5, y_offset_tiles_x100: 0, yaw_deg: 0) })"#
            )
            .is_err()
        );
    }
}
