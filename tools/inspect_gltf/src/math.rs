//! The little linear algebra the tool needs, written out by hand so the
//! crate stays free of `glam` (rule 8 greps for it): column-major 4x4
//! matrices in glTF's layout, quaternions as `[x, y, z, w]`, and the normal
//! matrix. Everything is `f32`, like the glTF data it reads.

/// A point or direction.
pub type Vec3 = [f32; 3];

/// A unit quaternion `[x, y, z, w]` (glTF's order).
pub type Quat = [f32; 4];

/// A 4x4 matrix in glTF's column-major layout: `self.0[column][row]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [[f32; 4]; 4]);

impl Mat4 {
    /// The identity.
    pub const IDENTITY: Mat4 = Mat4([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    /// `T * R * S`, the glTF node transform.
    pub fn from_trs(t: Vec3, r: Quat, s: Vec3) -> Mat4 {
        let [x, y, z, w] = r;
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        Mat4([
            [
                (1.0 - 2.0 * (yy + zz)) * s[0],
                2.0 * (xy + wz) * s[0],
                2.0 * (xz - wy) * s[0],
                0.0,
            ],
            [
                2.0 * (xy - wz) * s[1],
                (1.0 - 2.0 * (xx + zz)) * s[1],
                2.0 * (yz + wx) * s[1],
                0.0,
            ],
            [
                2.0 * (xz + wy) * s[2],
                2.0 * (yz - wx) * s[2],
                (1.0 - 2.0 * (xx + yy)) * s[2],
                0.0,
            ],
            [t[0], t[1], t[2], 1.0],
        ])
    }

    /// A translation.
    pub fn translation(t: Vec3) -> Mat4 {
        Mat4::from_trs(t, [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0])
    }

    /// A uniform scale.
    pub fn scale(s: f32) -> Mat4 {
        Mat4::from_trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [s, s, s])
    }

    /// A rotation of `deg` degrees about +Y, the same sense as a
    /// right-handed rotation (`x' = x cos + z sin`, `z' = -x sin + z cos`,
    /// so 90 turns +Z into +X). Multiples of 90 are exact.
    pub fn yaw_deg(deg: i32) -> Mat4 {
        let (sin, cos) = match deg.rem_euclid(360) {
            0 => (0.0, 1.0),
            90 => (1.0, 0.0),
            180 => (0.0, -1.0),
            270 => (-1.0, 0.0),
            d => (d as f32).to_radians().sin_cos(),
        };
        Mat4([
            [cos, 0.0, -sin, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [sin, 0.0, cos, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    /// `self * other`.
    pub fn mul(&self, other: &Mat4) -> Mat4 {
        let a = &self.0;
        let b = &other.0;
        let mut out = [[0.0f32; 4]; 4];
        for (c, col) in out.iter_mut().enumerate() {
            for (r, cell) in col.iter_mut().enumerate() {
                *cell =
                    a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2] + a[3][r] * b[c][3];
            }
        }
        Mat4(out)
    }

    /// The matrix applied to a point (w = 1).
    pub fn point(&self, p: Vec3) -> Vec3 {
        let m = &self.0;
        [
            m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
            m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
            m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
        ]
    }

    /// The matrix applied to a direction (w = 0).
    pub fn vector(&self, v: Vec3) -> Vec3 {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2],
            m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2],
            m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2],
        ]
    }

    /// The upper 3x3's columns.
    fn columns3(&self) -> [Vec3; 3] {
        let m = &self.0;
        [
            [m[0][0], m[0][1], m[0][2]],
            [m[1][0], m[1][1], m[1][2]],
            [m[2][0], m[2][1], m[2][2]],
        ]
    }

    /// The normal matrix: the inverse transpose of the upper 3x3, as its
    /// three columns, or `None` when the matrix is singular. A normal `n`
    /// maps to `n.x * c0 + n.y * c1 + n.z * c2`, then renormalised.
    pub fn normal_matrix(&self) -> Option<[Vec3; 3]> {
        let [c0, c1, c2] = self.columns3();
        let det = dot(c0, cross(c1, c2));
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        Some([
            scale3(cross(c1, c2), inv),
            scale3(cross(c2, c0), inv),
            scale3(cross(c0, c1), inv),
        ])
    }

    /// The matrix as 16 floats, column by column.
    pub fn to_cols_array(&self) -> [f32; 16] {
        let m = &self.0;
        let mut out = [0.0; 16];
        for c in 0..4 {
            out[c * 4..c * 4 + 4].copy_from_slice(&m[c]);
        }
        out
    }
}

/// Apply a normal matrix from [`Mat4::normal_matrix`] and renormalise.
pub fn apply_normal(n3: &[Vec3; 3], n: Vec3) -> Vec3 {
    normalize(add(
        add(scale3(n3[0], n[0]), scale3(n3[1], n[1])),
        scale3(n3[2], n[2]),
    ))
}

/// `a + b`.
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `a - b`.
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a * s`.
pub fn scale3(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Dot product.
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `a` scaled to length 1, or `a` itself when it is (nearly) zero.
pub fn normalize(a: Vec3) -> Vec3 {
    let len = dot(a, a).sqrt();
    if len > 1e-20 { scale3(a, 1.0 / len) } else { a }
}

/// Area of the triangle `a b c`.
pub fn triangle_area(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let n = cross(sub(b, a), sub(c, a));
    0.5 * dot(n, n).sqrt()
}

/// Linear interpolation of two vectors.
pub fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A quaternion scaled to length 1 (the identity when it is zero).
pub fn quat_normalize(q: Quat) -> Quat {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len > 1e-20 {
        [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Spherical linear interpolation along the shorter arc (glTF's LINEAR for
/// rotations), falling back to a normalised lerp for nearly equal inputs.
pub fn slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let mut cos = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let mut b = b;
    if cos < 0.0 {
        cos = -cos;
        b = [-b[0], -b[1], -b[2], -b[3]];
    }
    let (wa, wb) = if cos > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = cos.acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    quat_normalize([
        a[0] * wa + b[0] * wb,
        a[1] * wa + b[1] * wb,
        a[2] * wa + b[2] * wb,
        a[3] * wa + b[3] * wb,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn trs_rotates_translates_and_scales_like_hand_math() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        // 90 degrees about +Z turns +X into +Y.
        let m = Mat4::from_trs([1.0, 2.0, 3.0], [0.0, 0.0, half, half], [2.0, 2.0, 2.0]);
        assert!(close(m.point([1.0, 0.0, 0.0]), [1.0, 4.0, 3.0]));
        assert!(close(m.vector([0.0, 1.0, 0.0]), [-2.0, 0.0, 0.0]));
        let t = Mat4::translation([0.0, 1.0, 0.0]);
        assert!(close(t.mul(&m).point([0.0; 3]), [1.0, 3.0, 3.0]));
        assert!(close(m.mul(&t).point([0.0; 3]), [-1.0, 2.0, 3.0]));
    }

    #[test]
    fn yaw_quarter_turns_are_exact_and_turn_z_into_x() {
        assert_eq!(Mat4::yaw_deg(90).point([0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]);
        assert_eq!(Mat4::yaw_deg(-90).point([0.0, 0.0, 1.0]), [-1.0, 0.0, 0.0]);
        assert_eq!(Mat4::yaw_deg(180).point([1.0, 5.0, 2.0]), [-1.0, 5.0, -2.0]);
        assert!(close(
            Mat4::yaw_deg(45).point([0.0, 0.0, 1.0]),
            [
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2
            ]
        ));
    }

    #[test]
    fn normal_matrix_keeps_normals_perpendicular_under_non_uniform_scale() {
        // A 45-degree slope face, then squash y by 4: the normal must tilt
        // toward +Y, not away from it.
        let m = Mat4::from_trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0, 0.25, 1.0]);
        let n3 = m.normal_matrix().unwrap();
        let n = apply_normal(&n3, normalize([1.0, 1.0, 0.0]));
        let edge = m.vector([1.0, -1.0, 0.0]);
        assert!(dot(n, edge).abs() < 1e-6, "{n:?} vs {edge:?}");
        // A mirror keeps the outward direction.
        let mirror = Mat4::from_trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [-1.0, 1.0, 1.0]);
        let n = apply_normal(&mirror.normal_matrix().unwrap(), [1.0, 0.0, 0.0]);
        assert!(close(n, [-1.0, 0.0, 0.0]));
        assert!(Mat4::scale(0.0).normal_matrix().is_none());
    }

    #[test]
    fn slerp_halfway_is_half_the_angle() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let q = slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, half, half], 0.5);
        let m = Mat4::from_trs([0.0; 3], q, [1.0; 3]);
        assert!(close(m.point([1.0, 0.0, 0.0]), [half, half, 0.0]));
        // The shorter arc: -q is the same rotation.
        let q2 = slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, -half, -half], 0.5);
        let m2 = Mat4::from_trs([0.0; 3], q2, [1.0; 3]);
        assert!(close(m2.point([1.0, 0.0, 0.0]), [half, half, 0.0]));
    }
}
