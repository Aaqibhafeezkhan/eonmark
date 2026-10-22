//! `combat_step`: step time while 200 vs 200 units fight
//! (`sim::scenarios::combat_bench` at 400 units, the battle `sim-cli bench
//! --units 400 --ticks 1200 --combat` times). Target (ROADMAP M4a): mean
//! step < 5 ms with 200 vs 200 fighting, in release on the dev Mac. The
//! `sim-cli` command is the acceptance command and reports the mean, p95
//! and fighting-tick mean per step (it may use `std::time::Instant`, banned
//! in this crate); this criterion bench is the profiling companion
//! (`IMPLEMENTER_NOTES` rule 17: profile before changing algorithms).
//!
//! Setup: the battle runs to its first fighting tick, the first whose step
//! lands a `Hit` while both sides keep at least 50 units (the `sim-cli`
//! definition), and is snapshotted before that step. Each iteration
//! restores the snapshot into a fresh sim (not timed) and times
//! `first_20_fighting`: the 20 steps from that tick, with a throughput of
//! 20 elements, so criterion's `thrpt` line is steps per second and the
//! mean per step is the reported time divided by 20.

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use sim::scenarios::{self, COMBAT_BENCH_SEED, Stream};
use sim::{MatchSetup, Rules, Sim, SimEvent};
use std::hint::black_box;
use std::path::Path;

/// Units in the battle: 200 a side.
const UNITS: u16 = 400;
/// A fighting tick keeps at least this many units on each side.
const KEEP: usize = 50;
/// Steps timed per iteration.
const STEPS: u32 = 20;
/// The lines must have met by this tick.
const MAX_TICKS: u32 = 1200;

fn rules() -> Rules {
    Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).expect("data/ loads")
}

/// A fresh sim for the battle (0 ticks run).
fn fresh(rules: &Rules, setup: &MatchSetup, stream: &Stream) -> Sim {
    scenarios::run(setup.clone(), rules.clone(), stream, 0)
}

/// The battle's first fighting tick, its snapshot taken before that tick's
/// step, and the hash after [`STEPS`] steps from there.
fn first_fighting(rules: &Rules, setup: &MatchSetup, stream: &Stream) -> (u32, Vec<u8>, u64) {
    let mut sim = fresh(rules, setup, stream);
    for t in 0..MAX_TICKS {
        let snap = sim.snapshot();
        sim.step(scenarios::commands_at(stream, t));
        let hit = sim
            .drain_events()
            .iter()
            .any(|e| matches!(e, SimEvent::Hit { .. }));
        let mut alive = [0usize; 2];
        for u in sim.view().units() {
            alive[usize::from(u.owner.0)] += 1;
        }
        if hit && alive.iter().all(|&n| n >= KEEP) {
            for s in t + 1..t + STEPS {
                sim.step(scenarios::commands_at(stream, s));
            }
            return (t, snap, sim.hash());
        }
    }
    panic!("the battle has no fighting tick before tick {MAX_TICKS}");
}

fn bench_combat(c: &mut Criterion) {
    let rules = rules();
    let (setup, stream) = scenarios::combat_bench(&rules, UNITS, COMBAT_BENCH_SEED);
    let (first, snap, hash_after) = first_fighting(&rules, &setup, &stream);
    let restored = || {
        let mut sim = fresh(&rules, &setup, &stream);
        sim.restore(&snap)
            .expect("a snapshot of the same setup restores");
        sim
    };
    let steps = |sim: &mut Sim| {
        for t in first..first + STEPS {
            sim.step(scenarios::commands_at(&stream, t));
        }
    };
    let mut check = restored();
    steps(&mut check);
    assert_eq!(check.hash(), hash_after, "restore then step is exact");

    let mut group = c.benchmark_group("combat_step");
    group.sample_size(20);
    group.throughput(Throughput::Elements(u64::from(STEPS)));
    group.bench_function("first_20_fighting", |b| {
        b.iter_batched(
            restored,
            |mut sim| {
                steps(&mut sim);
                black_box(sim)
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_combat);
criterion_main!(benches);
