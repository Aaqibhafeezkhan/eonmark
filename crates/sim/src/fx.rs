//! Fixed-point numerics for the simulation. No floats anywhere in `sim`.
//!
//! [`Fx`] is a 32.32 signed fixed-point scalar; one unit is one tile. All
//! arithmetic operators are checked and panic on overflow in every build
//! profile, so a divergence can never hide behind silent wrapping.

use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};
use serde::{Deserialize, Serialize};

/// The underlying fixed-point type.
pub type Raw = fixed::types::I32F32;

/// 32.32 fixed-point scalar used for positions, speeds, ranges and radii.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Fx(pub Raw);

impl Fx {
    /// Zero.
    pub const ZERO: Fx = Fx(Raw::ZERO);
    /// One tile.
    pub const ONE: Fx = Fx(Raw::ONE);
    /// Half a tile.
    pub const HALF: Fx = Fx(Raw::from_bits(1 << 31));
    /// Largest representable value.
    pub const MAX: Fx = Fx(Raw::MAX);

    /// Construct from an integer number of tiles/units.
    pub const fn from_int(v: i32) -> Fx {
        Fx(Raw::from_bits((v as i64) << 32))
    }

    /// Construct from the raw 64-bit representation.
    pub const fn from_bits(bits: i64) -> Fx {
        Fx(Raw::from_bits(bits))
    }

    /// `num / den` as a fixed-point value, rounded toward negative infinity.
    /// Panics when `den == 0`.
    pub fn from_ratio(num: i32, den: i32) -> Fx {
        assert!(den != 0, "Fx::from_ratio by zero");
        Fx(Raw::from_num(num) / Raw::from_num(den))
    }

    /// Raw 64-bit representation (for hashing and exact arithmetic).
    pub const fn to_bits(self) -> i64 {
        self.0.to_bits()
    }

    /// Integer part, rounded toward negative infinity.
    pub fn floor_to_int(self) -> i32 {
        self.0.floor().to_num::<i32>()
    }

    /// Nearest integer, ties away from zero.
    pub fn round_to_int(self) -> i32 {
        self.0.round().to_num::<i32>()
    }

    /// Absolute value. Panics on `Fx::MIN`.
    pub fn abs(self) -> Fx {
        Fx(self.0.checked_abs().expect("Fx abs overflow"))
    }

    /// Checked addition.
    pub fn checked_add(self, o: Fx) -> Option<Fx> {
        self.0.checked_add(o.0).map(Fx)
    }

    /// Checked subtraction.
    pub fn checked_sub(self, o: Fx) -> Option<Fx> {
        self.0.checked_sub(o.0).map(Fx)
    }

    /// Checked multiplication.
    pub fn checked_mul(self, o: Fx) -> Option<Fx> {
        self.0.checked_mul(o.0).map(Fx)
    }

    /// Checked division; `None` on division by zero or overflow.
    pub fn checked_div(self, o: Fx) -> Option<Fx> {
        self.0.checked_div(o.0).map(Fx)
    }

    /// Multiply by an integer.
    pub fn mul_int(self, k: i32) -> Fx {
        Fx(self
            .0
            .checked_mul_int(i64::from(k))
            .expect("Fx mul_int overflow"))
    }

    /// Divide by an integer, rounding toward negative infinity.
    pub fn div_int(self, k: i32) -> Fx {
        assert!(k != 0, "Fx div_int by zero");
        Fx(self
            .0
            .checked_div_int(i64::from(k))
            .expect("Fx div_int overflow"))
    }

    /// Smaller of two values.
    pub fn min(self, o: Fx) -> Fx {
        if self <= o { self } else { o }
    }

    /// Larger of two values.
    pub fn max(self, o: Fx) -> Fx {
        if self >= o { self } else { o }
    }

    /// Clamp into `[lo, hi]`.
    pub fn clamp(self, lo: Fx, hi: Fx) -> Fx {
        self.max(lo).min(hi)
    }
}

impl Add for Fx {
    type Output = Fx;
    fn add(self, o: Fx) -> Fx {
        self.checked_add(o).expect("Fx add overflow")
    }
}

impl AddAssign for Fx {
    fn add_assign(&mut self, o: Fx) {
        *self = *self + o;
    }
}

impl Sub for Fx {
    type Output = Fx;
    fn sub(self, o: Fx) -> Fx {
        self.checked_sub(o).expect("Fx sub overflow")
    }
}

impl SubAssign for Fx {
    fn sub_assign(&mut self, o: Fx) {
        *self = *self - o;
    }
}

impl Mul for Fx {
    type Output = Fx;
    fn mul(self, o: Fx) -> Fx {
        self.checked_mul(o).expect("Fx mul overflow")
    }
}

impl Div for Fx {
    type Output = Fx;
    fn div(self, o: Fx) -> Fx {
        self.checked_div(o).expect("Fx div by zero or overflow")
    }
}

impl Neg for Fx {
    type Output = Fx;
    fn neg(self) -> Fx {
        Fx(self.0.checked_neg().expect("Fx neg overflow"))
    }
}

impl From<i32> for Fx {
    fn from(v: i32) -> Fx {
        Fx::from_int(v)
    }
}

/// A 2-D fixed-point vector in tile units.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct FxVec2 {
    /// East-west component.
    pub x: Fx,
    /// North-south component.
    pub y: Fx,
}

impl FxVec2 {
    /// Origin.
    pub const ZERO: FxVec2 = FxVec2 {
        x: Fx::ZERO,
        y: Fx::ZERO,
    };

    /// Construct from components.
    pub const fn new(x: Fx, y: Fx) -> FxVec2 {
        FxVec2 { x, y }
    }

    /// Construct from integer tile coordinates.
    pub const fn from_ints(x: i32, y: i32) -> FxVec2 {
        FxVec2 {
            x: Fx::from_int(x),
            y: Fx::from_int(y),
        }
    }

    /// Scale both components.
    pub fn scale(self, k: Fx) -> FxVec2 {
        FxVec2 {
            x: self.x * k,
            y: self.y * k,
        }
    }

    /// Dot product in `Fx`. Panics on overflow; for distances use
    /// [`FxVec2::dist_sq_i64`], which cannot overflow on maps up to 256 tiles.
    pub fn dot(self, o: FxVec2) -> Fx {
        self.x * o.x + self.y * o.y
    }

    /// Squared distance to `o`, returned as an `i64` in `I32F32` scale
    /// (`result == (d * d).to_bits()` for an exact fixed-point `d`).
    ///
    /// Computed as `((dx.to_bits() as i128)^2 + (dy.to_bits() as i128)^2) >> 32`
    /// so the intermediate never overflows; the result fits in `i64` for any
    /// two points within a 256 x 256 map (max `2^49`).
    pub fn dist_sq_i64(self, o: FxVec2) -> i64 {
        let dx = i128::from(self.x.to_bits()) - i128::from(o.x.to_bits());
        let dy = i128::from(self.y.to_bits()) - i128::from(o.y.to_bits());
        let sum = dx * dx + dy * dy;
        i64::try_from(sum >> 32).expect("dist_sq_i64 exceeds i64")
    }

    /// Squared length as an `i64` in `I32F32` scale.
    pub fn length_sq_i64(self) -> i64 {
        self.dist_sq_i64(FxVec2::ZERO)
    }
}

impl Add for FxVec2 {
    type Output = FxVec2;
    fn add(self, o: FxVec2) -> FxVec2 {
        FxVec2 {
            x: self.x + o.x,
            y: self.y + o.y,
        }
    }
}

impl AddAssign for FxVec2 {
    fn add_assign(&mut self, o: FxVec2) {
        *self = *self + o;
    }
}

impl Sub for FxVec2 {
    type Output = FxVec2;
    fn sub(self, o: FxVec2) -> FxVec2 {
        FxVec2 {
            x: self.x - o.x,
            y: self.y - o.y,
        }
    }
}

impl SubAssign for FxVec2 {
    fn sub_assign(&mut self, o: FxVec2) {
        *self = *self - o;
    }
}

impl Neg for FxVec2 {
    type Output = FxVec2;
    fn neg(self) -> FxVec2 {
        FxVec2 {
            x: -self.x,
            y: -self.y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `v` tiles squared, in I32F32 scale.
    fn sq_tiles(v: i64) -> i64 {
        (v * v) << 32
    }

    #[test]
    fn dist_sq_i64_map_corners() {
        // 128 map: opposite corners at (0,0) and (128,128).
        let a = FxVec2::from_ints(0, 0);
        let b = FxVec2::from_ints(128, 128);
        assert_eq!(a.dist_sq_i64(b), 2 * sq_tiles(128));
        assert_eq!(b.dist_sq_i64(a), 2 * sq_tiles(128));
        // 128 map: along one edge.
        assert_eq!(a.dist_sq_i64(FxVec2::from_ints(128, 0)), sq_tiles(128));
        // 256 map: opposite corners.
        let c = FxVec2::from_ints(256, 256);
        assert_eq!(a.dist_sq_i64(c), 2 * sq_tiles(256));
        assert_eq!(a.dist_sq_i64(c), 131_072i64 << 32);
        // Tile-centre to opposite tile-centre on a 256 map (255.5 each axis).
        let h = Fx::HALF;
        let p = FxVec2::new(h, h);
        let q = FxVec2::new(Fx::from_int(255) + h, Fx::from_int(255) + h);
        assert_eq!(p.dist_sq_i64(q), 2 * sq_tiles(255));
    }

    #[test]
    fn dist_sq_i64_matches_fixed_multiply_for_small_values() {
        let p = FxVec2::new(Fx::from_ratio(3, 4), Fx::from_ratio(-5, 8));
        let q = FxVec2::new(Fx::from_int(2), Fx::HALF);
        let d = q - p;
        let expected = (d.x * d.x + d.y * d.y).to_bits();
        assert_eq!(p.dist_sq_i64(q), expected);
        assert_eq!(Fx::HALF.checked_mul(Fx::HALF).unwrap().to_bits(), 1 << 30);
        assert_eq!(FxVec2::new(Fx::HALF, Fx::ZERO).length_sq_i64(), 1 << 30);
    }

    #[test]
    fn arithmetic_helpers() {
        let a = Fx::from_int(3);
        let b = Fx::from_ratio(1, 2);
        assert_eq!(a + b, Fx::from_ratio(7, 2));
        assert_eq!(a - b, Fx::from_ratio(5, 2));
        assert_eq!(a * b, Fx::from_ratio(3, 2));
        assert_eq!(a / b, Fx::from_int(6));
        assert_eq!(a.mul_int(4), Fx::from_int(12));
        assert_eq!(a.div_int(2), Fx::from_ratio(3, 2));
        assert_eq!((-a).abs(), a);
        assert_eq!(Fx::from_ratio(7, 2).floor_to_int(), 3);
        assert_eq!(Fx::from_ratio(-7, 2).floor_to_int(), -4);
        assert_eq!(Fx::from_ratio(7, 2).round_to_int(), 4);
        assert_eq!(a.clamp(Fx::ZERO, Fx::ONE), Fx::ONE);
        assert_eq!(
            FxVec2::from_ints(1, 2).dot(FxVec2::from_ints(3, 4)),
            Fx::from_int(11)
        );
        assert_eq!(Fx::from_int(-1).to_bits(), -(1i64 << 32));
        assert_eq!(Fx::from_bits(1 << 32), Fx::ONE);
    }

    #[test]
    #[should_panic(expected = "Fx add overflow")]
    fn overflow_panics_in_every_profile() {
        let _ = Fx::MAX + Fx::ONE;
    }
}
