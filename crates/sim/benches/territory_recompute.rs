//! `territory_recompute`: cost of editing the territory field.
//! Target (docs/design/territory.md, M3a acceptance): mean incremental
//! recompute < 2 ms for 16 sources and 100 edits, in release on the dev Mac.
//! `sim-cli bench --territory --sources 16 --edits 100` is the acceptance
//! command; this criterion bench is the profiling companion
//! (`IMPLEMENTER_NOTES` rule 17: profile before changing algorithms).
//!
//! Setup: the default map's field with 16 Towns, owners alternating between
//! the two players, centres on a fixed 4 x 4 grid across the map, radii
//! alternating between the two Town tiers from `rules.ron`. Benches:
//!
//! - `change_source_tier`: one Town toggling between tier 1 and tier 2 (two
//!   `change_source` calls per iteration, so the field returns to its start).
//! - `full_recompute`: clear every layer and re-add all 16 sources.

use criterion::{Criterion, criterion_group, criterion_main};
use rules::Rules;
use sim::{BuildingId, PlayerId, Source, SourceOwner, TerritoryField, Tile};
use std::hint::black_box;
use std::path::Path;

const SOURCES: u16 = 16;

fn setup() -> (TerritoryField, Source, Source) {
    let rules =
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).expect("data/ loads");
    let def = rules.map(&rules.default_map).expect("default map");
    let r1 = u16::try_from(rules.town_radius_tiles).expect("radius fits u16");
    let r2 = u16::try_from(rules.town_radius_grown_tiles).expect("radius fits u16");
    let mut field = TerritoryField::new(def.width, def.height, 2);
    let step_x = def.width / 4;
    let step_y = def.height / 4;
    for i in 0..SOURCES {
        let src = Source {
            owner: SourceOwner::Player(PlayerId(u8::try_from(i % 2).expect("0 or 1"))),
            centre: Tile::new(step_x / 2 + (i % 4) * step_x, step_y / 2 + (i / 4) * step_y),
            radius: if i % 2 == 0 { r1 } else { r2 },
        };
        field.add_source(BuildingId(u32::from(i) + 1), src);
    }
    let first = field.sources()[&BuildingId(1)];
    let grown = Source {
        radius: r2,
        ..first
    };
    assert_eq!(field, field.recomputed(), "setup is consistent");
    (field, first, grown)
}

fn bench(c: &mut Criterion) {
    let (field, first, grown) = setup();

    let mut f = field.clone();
    c.bench_function("territory_change_source_tier", |b| {
        b.iter(|| {
            f.change_source(BuildingId(1), black_box(grown));
            f.change_source(BuildingId(1), black_box(first));
        });
    });
    assert_eq!(f, field, "the toggle returns the field to its start");

    let mut g = field.clone();
    c.bench_function("territory_full_recompute", |b| {
        b.iter(|| {
            g.full_recompute();
            black_box(&g);
        });
    });
    assert_eq!(g, field);
}

criterion_group!(benches, bench);
criterion_main!(benches);
