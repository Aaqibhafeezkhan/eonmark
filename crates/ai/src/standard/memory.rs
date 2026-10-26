//! What the Standard bot remembers between plans: [`BotMemory`], encoded
//! with postcard into the player's hashed `AiMemory`.
//!
//! Only facts that cannot be re-derived from the view live here (the wave
//! roster and timers, the defend latch, the military latch, blocked-step
//! cooldowns, the chosen site). Everything else, the build-order window
//! included, is recomputed every plan, so the memory stays small (it is
//! hashed and snapshotted every tick) and a restore reproduces the bot's
//! next decision exactly.

use serde::{Deserialize, Serialize};
use sim::{BuildingId, Tile, UnitId};

/// The Standard bot's memory. The default (nothing remembered yet) encodes
/// to no bytes at all, so a bot that has nothing to remember leaves the
/// player's `AiMemory` empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotMemory {
    /// Set once the build order's `Military` step is crossed; never
    /// cleared, so harassment that reopens earlier steps does not stop
    /// production.
    pub military: bool,
    /// Steps (build-order index) blocked for a reason other than cost, and
    /// the tick from which each is retried.
    pub blocked: Vec<(u16, u32)>,
    /// The site chosen for a `Build` step (index, footprint north-west
    /// tile), kept until a check rejects it.
    pub site: Option<(u16, Tile)>,
    /// The active attack wave.
    pub wave: Option<Wave>,
    /// No wave launches before this tick.
    pub next_wave_at: u32,
    /// Waves launched so far.
    pub waves_launched: u16,
    /// The defend latch.
    pub defend: Option<Defend>,
    /// Defences released so far.
    pub defends: u16,
}

/// An attack wave's roster and progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wave {
    /// The armed units sent, in id order.
    pub units: Vec<UnitId>,
    /// The Supply Wain sent with them.
    pub wain: Option<UnitId>,
    /// The enemy Town attacked.
    pub target: BuildingId,
    /// Where the wave gathers before the assault.
    pub stage: Tile,
    /// Where the wave is.
    pub phase: WavePhase,
    /// The tick it launched, or retargeted after a capture: its
    /// `wave_max_ds` runs from here.
    pub launched_at: u32,
    /// The tick the current phase began (`stage_max_ds` runs from the
    /// start of a `Stage`).
    pub phase_at: u32,
}

/// The phases of a wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WavePhase {
    /// Walking to, or waiting at, the stage point.
    Stage,
    /// Attacking the target.
    Assault,
    /// Holding the ring of the target at 0 HP to annex it.
    Hold,
}

/// The defend latch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defend {
    /// The own Town defended.
    pub town: BuildingId,
    /// The last tick a threat was seen.
    pub last_threat: u32,
    /// The tick the defence started.
    pub since: u32,
}

/// The counts `sim-cli bench --bots` reads from a player's memory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BotStats {
    /// Waves launched.
    pub waves_launched: u16,
    /// Defences released.
    pub defends: u16,
    /// Whether a defence is running now.
    pub defending: bool,
}

impl BotMemory {
    /// Decode a player's memory bytes: empty bytes are the default memory.
    ///
    /// # Panics
    ///
    /// When non-empty bytes do not decode: only this bot writes them, so
    /// that is a bug (or another bot's memory), never bad input.
    pub fn decode(bytes: &[u8]) -> BotMemory {
        if bytes.is_empty() {
            return BotMemory::default();
        }
        postcard::from_bytes(bytes).expect("the Standard bot's memory decodes")
    }

    /// Encode for the player's `AiMemory`: no bytes for the default
    /// memory, postcard otherwise.
    pub fn encode(&self) -> Vec<u8> {
        if *self == BotMemory::default() {
            return Vec::new();
        }
        postcard::to_allocvec(self).expect("the Standard bot's memory encodes")
    }
}

/// The bench counts from a Standard bot's memory bytes.
pub fn stats(bytes: &[u8]) -> BotStats {
    let m = BotMemory::decode(bytes);
    BotStats {
        waves_launched: m.waves_launched,
        defends: m.defends,
        defending: m.defend.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_memory_is_no_bytes_and_every_field_round_trips() {
        assert!(BotMemory::default().encode().is_empty());
        assert_eq!(BotMemory::decode(&[]), BotMemory::default());
        let m = BotMemory {
            military: true,
            blocked: vec![(3, 600)],
            site: Some((9, Tile::new(19, 82))),
            wave: Some(Wave {
                units: vec![UnitId(40), UnitId(41)],
                wain: Some(UnitId(42)),
                target: BuildingId(2),
                stage: Tile::new(80, 64),
                phase: WavePhase::Assault,
                launched_at: 13_000,
                phase_at: 13_600,
            }),
            next_wave_at: 16_600,
            waves_launched: 1,
            defend: Some(Defend {
                town: BuildingId(1),
                last_threat: 12_990,
                since: 12_900,
            }),
            defends: 2,
        };
        let bytes = m.encode();
        assert!(!bytes.is_empty());
        assert_eq!(BotMemory::decode(&bytes), m);
        assert_eq!(
            stats(&bytes),
            BotStats {
                waves_launched: 1,
                defends: 2,
                defending: true
            }
        );
        assert_eq!(stats(&[]), BotStats::default());
    }

    /// A full 40-unit roster with a cooldown per step stays far under the
    /// 4096-byte guide.
    #[test]
    fn a_full_wave_stays_under_the_memory_guide() {
        let m = BotMemory {
            blocked: (0..64).map(|i| (i, 54_000)).collect(),
            wave: Some(Wave {
                units: (100_000..100_040).map(UnitId).collect(),
                wain: Some(UnitId(100_040)),
                target: BuildingId(1_000_000),
                stage: Tile::new(127, 127),
                phase: WavePhase::Hold,
                launched_at: 54_000,
                phase_at: 54_000,
            }),
            ..BotMemory::default()
        };
        assert!(m.encode().len() < sim::AI_MEMORY_GUIDE_BYTES / 4);
    }
}
