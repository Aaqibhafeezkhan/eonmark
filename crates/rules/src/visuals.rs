//! Schema for `data/visuals.ron`: how each unit kind is drawn.
//!
//! The simulation never reads this file; it is loaded by `Rules::load` so
//! that it is validated with everything else (every unit kind must have a
//! visual, so `sim-cli data-check` fails on a bad or missing entry). It is
//! not part of `rules_hash` (ADR 0003,
//! `docs/adr/0003-visuals-outside-rules-hash.md`; `Rules::visuals` is
//! `#[serde(skip)]`): changing a look needs no `rules_version` bump and no
//! re-record, and replays never record which looks were loaded. The game
//! crate maps a [`Visual`] to a Bevy mesh and material in `present.rs`.
//!
//! M2 shipped primitives only; M4b adds [`Visual::Body`], one primitive
//! with its own height (or length, lying along the unit's facing), so each
//! kind has its own silhouette. [`Visual::Scene`] is parsed and validated
//! so the M7 glTF switch is a data change, but the presenter ignores it
//! until then. Every number is an integer: fields ending in `_x100` are
//! hundredths and `yaw_deg` is whole degrees, following
//! `docs/DATA_FORMAT.md`.

use crate::{Error, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A built-in mesh shape. Its width comes from the unit kind's
/// `radius_tiles_x100`; a [`Visual::Primitive`] also takes its height from
/// the radius, a [`Visual::Body`] carries its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    /// Upright capsule (the M2 look for every unit): as a `Primitive`, its
    /// straight part is 1.4 radii long, so it stands 3.4 radii tall.
    Capsule,
    /// Axis-aligned box two radii wide and deep; as a `Primitive`, two
    /// radii tall.
    Cuboid,
    /// Upright cylinder; as a `Primitive`, two radii tall.
    Cylinder,
    /// Sphere of the kind's radius. Only as a `Primitive`: a `Body` is
    /// never a sphere, since a sphere has no height of its own.
    Sphere,
    /// Upright cone on its base (M4b); as a `Primitive`, two radii tall.
    Cone,
}

/// How a kind is drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Visual {
    /// One shared primitive mesh sized from the kind's radius, tinted with
    /// the owner's team colour.
    Primitive(Primitive),
    /// One shared primitive mesh with its own size (M4b): `shape` is two
    /// radii wide and `height_tiles_x100` tall, or, when `lying`, that long
    /// along the unit's facing and two radii tall. Tinted with the owner's
    /// team colour.
    Body {
        /// The shape; never `Sphere`.
        shape: Primitive,
        /// Height in hundredths of a tile (a tile is a metre), or the
        /// length along the facing when `lying`. In
        /// [`MIN_BODY_HEIGHT_X100`]`..=`[`MAX_BODY_HEIGHT_X100`]; a
        /// `Capsule` is at least two radii (its two half-spheres).
        height_tiles_x100: u32,
        /// Lay the shape on its side with its long axis along the unit's
        /// facing. Only a `Capsule` or a `Cylinder` can lie. Defaults to
        /// `false`.
        #[serde(default)]
        lying: bool,
    },
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

/// Lower bound on a [`Visual::Body`]'s `height_tiles_x100` (20 cm).
pub const MIN_BODY_HEIGHT_X100: u32 = 20;

/// Upper bound on a [`Visual::Body`]'s `height_tiles_x100` (4 m, a unit
/// four tiles tall or long is a data error).
pub const MAX_BODY_HEIGHT_X100: u32 = 400;

impl Visuals {
    /// Validate against the loaded unit kinds: every kind has a visual, every
    /// key names a kind, every `Body` and `Scene` is well formed. Errors name
    /// `path` and the field (`units["yeoman"]` or
    /// `units["yeoman"].scale_x100`).
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
            let Some(kind) = kinds.iter().find(|k| &k.id == id) else {
                return Err(Error::invalid(
                    path,
                    &field(""),
                    "no unit kind with this id in units.ron",
                ));
            };
            match visual {
                Visual::Primitive(_) => {}
                Visual::Body {
                    shape,
                    height_tiles_x100,
                    lying,
                } => {
                    if *shape == Primitive::Sphere {
                        return Err(Error::invalid(
                            path,
                            &field("shape"),
                            "a Body cannot be a Sphere (it has no height of its own); use Primitive(Sphere)",
                        ));
                    }
                    if *lying && !matches!(shape, Primitive::Capsule | Primitive::Cylinder) {
                        return Err(Error::invalid(
                            path,
                            &field("lying"),
                            format!("only a Capsule or a Cylinder can lie, not a {shape:?}"),
                        ));
                    }
                    if !(MIN_BODY_HEIGHT_X100..=MAX_BODY_HEIGHT_X100).contains(height_tiles_x100) {
                        return Err(Error::invalid(
                            path,
                            &field("height_tiles_x100"),
                            format!("must be in {MIN_BODY_HEIGHT_X100}..={MAX_BODY_HEIGHT_X100}"),
                        ));
                    }
                    let diameter = kind.radius_tiles_x100.saturating_mul(2);
                    if *shape == Primitive::Capsule && *height_tiles_x100 < diameter {
                        return Err(Error::invalid(
                            path,
                            &field("height_tiles_x100"),
                            format!(
                                "a Capsule must be at least twice the kind's radius_tiles_x100 ({diameter})"
                            ),
                        ));
                    }
                }
                Visual::Scene {
                    path: scene,
                    scale_x100,
                    yaw_deg,
                    ..
                } => {
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
            hp: 40,
            attack: None,
            cost: BTreeMap::new(),
            ramp: crate::UnitRamp::Flat,
            pop: 1,
            train_time_ds: 120,
            trainer: "town".into(),
            requires_tech: None,
            builder: true,
            vision_tiles: 8,
            immune_to_attrition: false,
            supplies: false,
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

    #[test]
    fn body_parses_and_validates() {
        let ok = [
            (
                r#"(units: { "yeoman": Body(shape: Cone, height_tiles_x100: 150) })"#,
                Visual::Body {
                    shape: Primitive::Cone,
                    height_tiles_x100: 150,
                    lying: false,
                },
            ),
            (
                r#"(units: { "yeoman": Body(shape: Capsule, height_tiles_x100: 180, lying: true) })"#,
                Visual::Body {
                    shape: Primitive::Capsule,
                    height_tiles_x100: 180,
                    lying: true,
                },
            ),
            (
                r#"(units: { "yeoman": Body(shape: Cylinder, height_tiles_x100: 20, lying: true) })"#,
                Visual::Body {
                    shape: Primitive::Cylinder,
                    height_tiles_x100: MIN_BODY_HEIGHT_X100,
                    lying: true,
                },
            ),
            (
                // A Capsule exactly two radii long (35 x 2) is the shortest one.
                r#"(units: { "yeoman": Body(shape: Capsule, height_tiles_x100: 70) })"#,
                Visual::Body {
                    shape: Primitive::Capsule,
                    height_tiles_x100: 70,
                    lying: false,
                },
            ),
            (
                r#"(units: { "yeoman": Body(shape: Cuboid, height_tiles_x100: 400, lying: false) })"#,
                Visual::Body {
                    shape: Primitive::Cuboid,
                    height_tiles_x100: MAX_BODY_HEIGHT_X100,
                    lying: false,
                },
            ),
            (
                r#"(units: { "yeoman": Primitive(Cone) })"#,
                Visual::Primitive(Primitive::Cone),
            ),
        ];
        for (text, want) in ok {
            let v = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            v.validate(Path::new("v.ron"), &kinds())
                .unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(v.get("yeoman"), Some(&want), "{text}");
        }
    }

    #[test]
    fn bad_body_fields_are_rejected_with_field() {
        let bad = [
            (
                r#"(units: { "yeoman": Body(shape: Sphere, height_tiles_x100: 100) })"#,
                "shape",
            ),
            (
                r#"(units: { "yeoman": Body(shape: Cone, height_tiles_x100: 150, lying: true) })"#,
                "lying",
            ),
            (
                r#"(units: { "yeoman": Body(shape: Cuboid, height_tiles_x100: 60, lying: true) })"#,
                "lying",
            ),
            (
                r#"(units: { "yeoman": Body(shape: Cuboid, height_tiles_x100: 19) })"#,
                "height_tiles_x100",
            ),
            (
                r#"(units: { "yeoman": Body(shape: Cylinder, height_tiles_x100: 401) })"#,
                "height_tiles_x100",
            ),
            (
                // Shorter than its two half-spheres (radius 35).
                r#"(units: { "yeoman": Body(shape: Capsule, height_tiles_x100: 69, lying: true) })"#,
                "height_tiles_x100",
            ),
        ];
        for (text, field) in bad {
            let v = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            let err = v
                .validate(Path::new("v.ron"), &kinds())
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("v.ron") && err.contains(&format!(r#"units["yeoman"].{field}"#)),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn unknown_body_shape_is_a_parse_error_with_line_and_column() {
        let err =
            parse("(units: {\n    \"yeoman\": Body(shape: Pyramid, height_tiles_x100: 100),\n})")
                .unwrap_err()
                .to_string();
        // ron 0.12 names the span of the variant: line 2, columns 27-34.
        assert!(err.starts_with("v.ron: 2:27"), "{err}");
        assert!(err.contains("Pyramid") && err.contains("Cone"), "{err}");
    }

    #[test]
    fn body_rejects_unknown_and_missing_fields_and_floats() {
        for text in [
            r#"(units: { "yeoman": Body(shape: Cone, height_tiles_x100: 150, width_tiles_x100: 50) })"#,
            r#"(units: { "yeoman": Body(shape: Cone) })"#,
            r#"(units: { "yeoman": Body(shape: Cone, height_tiles_x100: 1.5) })"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
