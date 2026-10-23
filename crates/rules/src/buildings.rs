//! Schema for `data/rules/buildings.ron`: the building kinds, their
//! footprints, costs, build times, hit points, income and gather slots.
//!
//! The list index is the sim's `BuildingKindId`. Exactly one kind is the
//! Town (`town: true`); it is the border source every player starts with
//! and founds more of. From M4a a kind may carry an `attack` (the Town and
//! the Watchtower shoot). Costs, base yields and gather rates are integers;
//! rates are per 30 s ([`crate::RATE_PERIOD_DS`]). Cross-references to
//! `resources.ron` and `units.ron` are checked here, the reverse ones
//! (`resources[i].gathered_from`, `units[i].trainer`) in `Rules::load`.
//! M5a adds `stays_on_annexation`: a kind that stays with the defender
//! when the Town it belongs to is annexed (the Watchtower), never the Town.

use crate::Error;
use crate::attack::{Attack, Prefer};
use crate::map::Terrain;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One building kind. Index in [`Buildings::buildings`] is the
/// `BuildingKindId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingKind {
    /// Stable identifier other data files use (`town`, `croft`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// `true` for the one Town kind: the border source a player starts with
    /// and founds more of, bounded by the town limit.
    #[serde(default)]
    pub town: bool,
    /// Footprint width in tiles (x).
    pub width_tiles: u32,
    /// Footprint height in tiles (y).
    pub height_tiles: u32,
    /// Base cost per resource id, before the building ramp
    /// (`building_ramp_step_permille` in `rules.ron`). Zero entries are
    /// omitted.
    pub cost: BTreeMap<String, u32>,
    /// Construction time with one builder at work, in deciseconds.
    pub build_time_ds: u32,
    /// Hit points of the complete building.
    pub hp: u32,
    /// Vision radius in tiles (fog of war, M7).
    pub vision_tiles: u32,
    /// Income per resource id per 30 s with no worker, while complete.
    #[serde(default)]
    pub base_yield: BTreeMap<String, u32>,
    /// Worker slots that gather a resource here, if any.
    #[serde(default)]
    pub gather: Option<GatherSpec>,
    /// At most this many of the kind per Town (attributed by nearest own
    /// Town), sites included. `None` means no per-Town limit.
    #[serde(default)]
    pub max_per_town: Option<u32>,
    /// Border radius this kind projects once complete (the Watchtower).
    /// The Town's radius comes from `rules.ron` instead.
    #[serde(default)]
    pub border_radius_tiles: Option<u32>,
    /// How the complete building shoots at enemy units; `None` (the
    /// default) for a kind that does not shoot. `prefer` must be `Units`.
    #[serde(default)]
    pub attack: Option<Attack>,
    /// Whether buildings and sites of this kind stay with their owner when
    /// the Town they belong to is annexed (M5a, the Watchtower); the rest
    /// transfer to the attacker with the Town. `false` when absent; never
    /// `true` on the Town kind.
    #[serde(default)]
    pub stays_on_annexation: bool,
}

impl BuildingKind {
    /// Whether the complete building is a border source.
    pub fn projects_border(&self) -> bool {
        self.town || self.border_radius_tiles.is_some()
    }
}

/// The worker slots of a gathering building.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatherSpec {
    /// Resource id the workers produce.
    pub resource: String,
    /// Unit kind id that works the slots (`yeoman`, `scribe`).
    pub worker: String,
    /// Income per worked slot per 30 s.
    pub rate_per_worker: u32,
    /// How many slots the building has.
    pub slots: Slots,
}

/// How many worker slots a building has; fixed when it is placed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Slots {
    /// Always this many.
    Fixed(u32),
    /// One per tile of `terrain` sharing an edge or a corner with the
    /// footprint, at most `max`. A placement with none is rejected.
    Adjacent {
        /// Resource terrain counted (`Forest` or `Mountain`).
        terrain: Terrain,
        /// Upper bound on the slot count.
        max: u32,
    },
}

/// Contents of `data/rules/buildings.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Buildings {
    /// Building kinds in `BuildingKindId` order.
    pub buildings: Vec<BuildingKind>,
}

/// Largest footprint side in tiles.
pub const MAX_FOOTPRINT_TILES: u32 = 8;
/// Largest `build_time_ds` (one hour).
pub const MAX_BUILD_TIME_DS: u32 = 36_000;
/// Largest `hp`, so hit points fit an `i32` with room for damage math.
pub const MAX_HP: u32 = 1_000_000;
/// Largest vision or border radius in tiles.
pub const MAX_RADIUS_TILES: u32 = 64;
/// Largest cost, yield or rate entry.
pub const MAX_AMOUNT: u32 = 100_000;
/// Largest slot count, so a slot index fits a `u8`.
pub const MAX_SLOTS: u32 = 255;

impl Buildings {
    /// Validate ids, ranges and references to `resource_ids` (from
    /// `resources.ron`) and `unit_ids` (from `units.ron`). Every error names
    /// `path` and the field (`buildings[i].cost["x"]`).
    pub fn validate(
        &self,
        path: &Path,
        resource_ids: &[&str],
        unit_ids: &[&str],
    ) -> Result<(), Error> {
        if self.buildings.is_empty() {
            return Err(Error::invalid(
                path,
                "buildings",
                "must list at least one building kind",
            ));
        }
        if self.buildings.len() > usize::from(u16::MAX) {
            return Err(Error::invalid(
                path,
                "buildings",
                "more kinds than BuildingKindId (u16) can index",
            ));
        }
        let towns = self.buildings.iter().filter(|b| b.town).count();
        if towns != 1 {
            return Err(Error::invalid(
                path,
                "buildings",
                format!("exactly one kind must have town: true (found {towns})"),
            ));
        }
        for (i, b) in self.buildings.iter().enumerate() {
            validate_kind(path, i, b, &self.buildings[..i], resource_ids, unit_ids)?;
        }
        Ok(())
    }

    /// Look up a kind by id string, with its index (`BuildingKindId.0`).
    pub fn get(&self, id: &str) -> Option<(u16, &BuildingKind)> {
        self.buildings
            .iter()
            .position(|b| b.id == id)
            .map(|i| (u16::try_from(i).expect("validated len"), &self.buildings[i]))
    }
}

fn validate_kind(
    path: &Path,
    i: usize,
    b: &BuildingKind,
    earlier: &[BuildingKind],
    resource_ids: &[&str],
    unit_ids: &[&str],
) -> Result<(), Error> {
    let field = |name: &str| format!("buildings[{i}].{name}");
    let in_range = |name: &str, v: u32, lo: u32, hi: u32| -> Result<(), Error> {
        if (lo..=hi).contains(&v) {
            Ok(())
        } else {
            Err(Error::invalid(
                path,
                &field(name),
                format!("{v} must be in {lo}..={hi}"),
            ))
        }
    };
    if b.id.is_empty() || !b.id.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        return Err(Error::invalid(
            path,
            &field("id"),
            format!(
                "{:?} must be non-empty lowercase ascii with underscores",
                b.id
            ),
        ));
    }
    if earlier.iter().any(|o| o.id == b.id) {
        return Err(Error::invalid(
            path,
            &field("id"),
            format!("duplicate id {:?}", b.id),
        ));
    }
    if b.name.is_empty() {
        return Err(Error::invalid(path, &field("name"), "must not be empty"));
    }
    in_range("width_tiles", b.width_tiles, 1, MAX_FOOTPRINT_TILES)?;
    in_range("height_tiles", b.height_tiles, 1, MAX_FOOTPRINT_TILES)?;
    in_range("build_time_ds", b.build_time_ds, 1, MAX_BUILD_TIME_DS)?;
    in_range("hp", b.hp, 1, MAX_HP)?;
    in_range("vision_tiles", b.vision_tiles, 1, MAX_RADIUS_TILES)?;
    amounts(path, &field("cost"), &b.cost, resource_ids)?;
    amounts(path, &field("base_yield"), &b.base_yield, resource_ids)?;
    if let Some(g) = &b.gather {
        if !resource_ids.contains(&g.resource.as_str()) {
            return Err(Error::invalid(
                path,
                &field("gather.resource"),
                format!("{:?} is not a resource id in resources.ron", g.resource),
            ));
        }
        if !unit_ids.contains(&g.worker.as_str()) {
            return Err(Error::invalid(
                path,
                &field("gather.worker"),
                format!("{:?} is not a unit id in units.ron", g.worker),
            ));
        }
        in_range("gather.rate_per_worker", g.rate_per_worker, 1, MAX_AMOUNT)?;
        match g.slots {
            Slots::Fixed(n) => in_range("gather.slots", n, 1, MAX_SLOTS)?,
            Slots::Adjacent { terrain, max } => {
                if !matches!(terrain, Terrain::Forest | Terrain::Mountain) {
                    return Err(Error::invalid(
                        path,
                        &field("gather.slots.terrain"),
                        format!("{terrain:?} holds no resource; use Forest or Mountain"),
                    ));
                }
                in_range("gather.slots.max", max, 1, MAX_SLOTS)?;
            }
        }
    }
    if let Some(n) = b.max_per_town {
        if b.town {
            return Err(Error::invalid(
                path,
                &field("max_per_town"),
                "the Town kind is bounded by town_limit in rules.ron instead",
            ));
        }
        in_range("max_per_town", n, 1, 255)?;
    }
    if let Some(r) = b.border_radius_tiles {
        if b.town {
            return Err(Error::invalid(
                path,
                &field("border_radius_tiles"),
                "the Town radius comes from town_radius_tiles in rules.ron",
            ));
        }
        in_range("border_radius_tiles", r, 1, MAX_RADIUS_TILES)?;
    }
    if b.town && b.stays_on_annexation {
        return Err(Error::invalid(
            path,
            &field("stays_on_annexation"),
            "an annexed Town is the building that changes hands; only other kinds can stay",
        ));
    }
    if let Some(a) = &b.attack {
        crate::attack::validate(path, &field("attack"), a)?;
        if a.prefer != Prefer::Units {
            return Err(Error::invalid(
                path,
                &field("attack.prefer"),
                "a building shoots enemy units only; use Units or omit it",
            ));
        }
    }
    if b.projects_border() && (b.width_tiles.is_multiple_of(2) || b.height_tiles.is_multiple_of(2))
    {
        return Err(Error::invalid(
            path,
            &field("width_tiles"),
            "a border source needs odd width and height so its centre is a tile",
        ));
    }
    Ok(())
}

/// Check a resource-id-to-amount map: every key is a resource id and every
/// value is in `1..=MAX_AMOUNT`. Errors name `field["id"]`.
pub(crate) fn amounts(
    path: &Path,
    field: &str,
    map: &BTreeMap<String, u32>,
    resource_ids: &[&str],
) -> Result<(), Error> {
    for (id, &v) in map {
        let key = format!("{field}[{id:?}]");
        if !resource_ids.contains(&id.as_str()) {
            return Err(Error::invalid(
                path,
                &key,
                "not a resource id in resources.ron",
            ));
        }
        if !(1..=MAX_AMOUNT).contains(&v) {
            return Err(Error::invalid(
                path,
                &key,
                format!("{v} must be in 1..={MAX_AMOUNT} (omit zero entries)"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RES: [&str; 4] = ["grain", "lumber", "ore", "lore"];
    const UNITS: [&str; 2] = ["yeoman", "scribe"];

    fn kind(id: &str) -> BuildingKind {
        BuildingKind {
            id: id.into(),
            name: id.into(),
            town: false,
            width_tiles: 2,
            height_tiles: 2,
            cost: BTreeMap::from([("lumber".into(), 30)]),
            build_time_ds: 150,
            hp: 300,
            vision_tiles: 6,
            base_yield: BTreeMap::new(),
            gather: None,
            max_per_town: None,
            border_radius_tiles: None,
            attack: None,
            stays_on_annexation: false,
        }
    }

    fn sample() -> Buildings {
        let mut town = kind("town");
        town.town = true;
        town.width_tiles = 3;
        town.height_tiles = 3;
        let mut croft = kind("croft");
        croft.gather = Some(GatherSpec {
            resource: "grain".into(),
            worker: "yeoman".into(),
            rate_per_worker: 10,
            slots: Slots::Fixed(1),
        });
        croft.max_per_town = Some(5);
        let mut yard = kind("lumber_yard");
        yard.gather = Some(GatherSpec {
            resource: "lumber".into(),
            worker: "yeoman".into(),
            rate_per_worker: 10,
            slots: Slots::Adjacent {
                terrain: Terrain::Forest,
                max: 6,
            },
        });
        let mut tower = kind("watchtower");
        tower.width_tiles = 1;
        tower.height_tiles = 1;
        tower.border_radius_tiles = Some(2);
        tower.attack = Some(Attack {
            damage: 15,
            range_tiles_x100: 700,
            cooldown_ds: 15,
            prefer: Prefer::Units,
        });
        Buildings {
            buildings: vec![town, croft, yard, tower],
        }
    }

    fn err(b: &Buildings) -> String {
        b.validate(Path::new("b.ron"), &RES, &UNITS)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn sample_is_valid() {
        let b = sample();
        b.validate(Path::new("b.ron"), &RES, &UNITS).unwrap();
        assert_eq!(b.get("croft").map(|(i, _)| i), Some(1));
        assert!(b.get("granary").is_none());
        assert!(b.buildings[0].projects_border());
        assert!(b.buildings[3].projects_border());
        assert!(!b.buildings[1].projects_border());
    }

    #[test]
    fn exactly_one_town_kind() {
        let mut b = sample();
        b.buildings[1].town = true;
        b.buildings[1].max_per_town = None;
        b.buildings[1].width_tiles = 3;
        b.buildings[1].height_tiles = 3;
        assert!(err(&b).contains("exactly one kind must have town: true"));
        let mut b = sample();
        b.buildings[0].town = false;
        assert!(err(&b).contains("found 0"));
    }

    #[test]
    fn unknown_resource_in_cost_names_the_key() {
        let mut b = sample();
        b.buildings[1].cost.insert("gold".into(), 5);
        let e = err(&b);
        assert!(e.contains(r#"buildings[1].cost["gold"]"#), "{e}");
    }

    #[test]
    fn zero_amounts_and_times_are_rejected() {
        let mut b = sample();
        b.buildings[1].cost.insert("ore".into(), 0);
        assert!(err(&b).contains(r#"buildings[1].cost["ore"]"#));
        let mut b = sample();
        b.buildings[2].build_time_ds = 0;
        assert!(err(&b).contains("buildings[2].build_time_ds"));
        let mut b = sample();
        b.buildings[0].hp = MAX_HP + 1;
        assert!(err(&b).contains("buildings[0].hp"));
    }

    #[test]
    fn even_footprint_border_source_is_rejected() {
        let mut b = sample();
        b.buildings[3].width_tiles = 2;
        assert!(err(&b).contains("odd width and height"));
        let mut b = sample();
        b.buildings[0].height_tiles = 4;
        assert!(err(&b).contains("buildings[0].width_tiles"));
    }

    #[test]
    fn gather_references_and_slots_are_checked() {
        let mut b = sample();
        b.buildings[1].gather.as_mut().unwrap().worker = "skirmisher".into();
        assert!(err(&b).contains("buildings[1].gather.worker"));
        let mut b = sample();
        b.buildings[1].gather.as_mut().unwrap().slots = Slots::Fixed(0);
        assert!(err(&b).contains("buildings[1].gather.slots"));
        let mut b = sample();
        b.buildings[2].gather.as_mut().unwrap().slots = Slots::Adjacent {
            terrain: Terrain::Grass,
            max: 6,
        };
        assert!(err(&b).contains("buildings[2].gather.slots.terrain"));
    }

    #[test]
    fn shooter_attacks_are_checked() {
        let mut b = sample();
        b.buildings[3].attack.as_mut().unwrap().cooldown_ds = 0;
        assert!(err(&b).contains("buildings[3].attack.cooldown_ds"));
        let mut b = sample();
        b.buildings[3].attack.as_mut().unwrap().prefer = Prefer::Buildings;
        assert!(err(&b).contains("buildings[3].attack.prefer"));
    }

    #[test]
    fn town_takes_no_per_town_limit_or_border_radius() {
        let mut b = sample();
        b.buildings[0].max_per_town = Some(1);
        assert!(err(&b).contains("buildings[0].max_per_town"));
        let mut b = sample();
        b.buildings[0].border_radius_tiles = Some(3);
        assert!(err(&b).contains("buildings[0].border_radius_tiles"));
    }

    #[test]
    fn only_a_non_town_kind_stays_on_annexation() {
        let mut b = sample();
        b.buildings[3].stays_on_annexation = true;
        b.validate(Path::new("b.ron"), &RES, &UNITS).unwrap();
        b.buildings[0].stays_on_annexation = true;
        assert!(err(&b).contains("buildings[0].stays_on_annexation"));
    }

    #[test]
    fn slots_parse_in_both_shapes_and_reject_unknown_fields() {
        let s: Slots = ron::from_str("Adjacent(terrain: Mountain, max: 6)").unwrap();
        assert_eq!(
            s,
            Slots::Adjacent {
                terrain: Terrain::Mountain,
                max: 6
            }
        );
        let s: Slots = ron::from_str("Fixed(3)").unwrap();
        assert_eq!(s, Slots::Fixed(3));
        let e = ron::from_str::<Slots>("Adjacent(terrain: Forest, max: 6, min: 1)").unwrap_err();
        assert!(e.to_string().contains("min"), "{e}");
    }
}
