//! Texel footprints of faces and rect sets over a texture: what a face
//! samples, which rects a set of faces covers, and whether a face lies
//! inside, outside or across a set of team rects.

use std::fmt;

/// A texel rect `(x, y, w, h)`, as `visuals.ron`'s `team_swatches` write
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rect {
    /// Left texel.
    pub x: u32,
    /// Top texel.
    pub y: u32,
    /// Width in texels, at least 1.
    pub w: u32,
    /// Height in texels, at least 1.
    pub h: u32,
}

impl Rect {
    /// Whether the two rects share a texel.
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    /// The smallest rect holding both.
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect {
            x,
            y,
            w: (self.x + self.w).max(o.x + o.w) - x,
            h: (self.y + self.h).max(o.y + o.h) - y,
        }
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {}, {})", self.x, self.y, self.w, self.h)
    }
}

/// Rects in RON list form: `[(x, y, w, h), ...]`.
pub fn ron_list(rects: &[Rect]) -> String {
    let items: Vec<String> = rects.iter().map(Rect::to_string).collect();
    format!("[{}]", items.join(", "))
}

/// The texels a face with these texture coordinates samples: the texel
/// bounding box of its UV triangle, at least one texel. A triangle lying
/// wholly in one repeat of the texture is wrapped into it; one spanning a
/// repeat is clamped.
pub fn face_rect(uvs: [[f32; 2]; 3], width: u32, height: u32) -> Rect {
    let axis = |i: usize, size: u32| -> (u32, u32) {
        let lo = uvs.iter().map(|uv| uv[i]).fold(f32::INFINITY, f32::min);
        let hi = uvs.iter().map(|uv| uv[i]).fold(f32::NEG_INFINITY, f32::max);
        let shift = lo.floor();
        let (lo, hi) = (lo - shift, hi - shift);
        let s = size as f32;
        let a = ((lo * s).floor().max(0.0) as u32).min(size - 1);
        let b = ((hi * s).ceil().max(0.0) as u32).min(size);
        (a, b.max(a + 1))
    };
    let (x0, x1) = axis(0, width);
    let (y0, y1) = axis(1, height);
    Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

/// Where a face's footprint lies against a set of rects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    /// No texel in the rects.
    Outside,
    /// Every texel in the rects.
    Inside,
    /// Some texels in, some out: a repaint would split the face.
    Straddles,
}

/// A set of texels of a `width` x `height` texture.
#[derive(Clone, Debug)]
pub struct Mask {
    width: u32,
    height: u32,
    bits: Vec<bool>,
}

impl Mask {
    /// An empty set.
    pub fn new(width: u32, height: u32) -> Mask {
        Mask {
            width,
            height,
            bits: vec![false; width as usize * height as usize],
        }
    }

    /// The set covered by `rects` (clipped to the texture).
    pub fn from_rects(width: u32, height: u32, rects: &[Rect]) -> Mask {
        let mut m = Mask::new(width, height);
        for r in rects {
            m.add(r);
        }
        m
    }

    /// Add a rect's texels.
    pub fn add(&mut self, r: &Rect) {
        for y in r.y..(r.y + r.h).min(self.height) {
            for x in r.x..(r.x + r.w).min(self.width) {
                self.bits[y as usize * self.width as usize + x as usize] = true;
            }
        }
    }

    fn get(&self, x: u32, y: u32) -> bool {
        self.bits[y as usize * self.width as usize + x as usize]
    }

    /// How many texels are set.
    pub fn count(&self) -> usize {
        self.bits.iter().filter(|&&b| b).count()
    }

    /// Where `r` lies against the set.
    pub fn cover(&self, r: &Rect) -> Cover {
        let (mut inside, mut outside) = (false, false);
        for y in r.y..(r.y + r.h).min(self.height) {
            for x in r.x..(r.x + r.w).min(self.width) {
                if self.get(x, y) {
                    inside = true;
                } else {
                    outside = true;
                }
            }
        }
        match (inside, outside) {
            (true, false) => Cover::Inside,
            (true, true) => Cover::Straddles,
            _ => Cover::Outside,
        }
    }

    /// The set as rects: the bounding box of each 4-connected component,
    /// with overlapping boxes merged until none overlap, sorted by row then
    /// column. The rects can hold texels outside the set (an L-shaped
    /// component becomes its box), so callers re-test faces against them.
    pub fn rects(&self) -> Vec<Rect> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut seen = vec![false; w * h];
        let mut out: Vec<Rect> = Vec::new();
        for start in 0..w * h {
            if !self.bits[start] || seen[start] {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                let mut push = |j: usize| {
                    if self.bits[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                };
                if x > 0 {
                    push(i - 1);
                }
                if x + 1 < w {
                    push(i + 1);
                }
                if y > 0 {
                    push(i - w);
                }
                if y + 1 < h {
                    push(i + w);
                }
            }
            out.push(Rect {
                x: x0 as u32,
                y: y0 as u32,
                w: (x1 - x0 + 1) as u32,
                h: (y1 - y0 + 1) as u32,
            });
        }
        merge_overlapping(out)
    }
}

/// Merge overlapping rects until none overlap; sorted by row then column.
pub fn merge_overlapping(mut rects: Vec<Rect>) -> Vec<Rect> {
    loop {
        let mut merged = false;
        'outer: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if rects[i].intersects(&rects[j]) {
                    let u = rects[i].union(&rects[j]);
                    rects[i] = u;
                    rects.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            break;
        }
    }
    rects.sort_by_key(|r| (r.y, r.x, r.h, r.w));
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_rects_cover_the_uv_box_and_at_least_one_texel() {
        let r = face_rect([[0.25, 0.5], [0.5, 0.5], [0.25, 0.75]], 8, 8);
        assert_eq!(
            r,
            Rect {
                x: 2,
                y: 4,
                w: 2,
                h: 2
            }
        );
        let point = face_rect([[0.3, 0.3]; 3], 10, 10);
        assert_eq!(
            point,
            Rect {
                x: 3,
                y: 3,
                w: 1,
                h: 1
            }
        );
        let edge = face_rect([[1.0, 1.0]; 3], 4, 4);
        assert_eq!(
            edge,
            Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1
            },
            "1.0 wraps to 0"
        );
        let repeat = face_rect([[1.25, 2.5], [1.5, 2.5], [1.25, 2.75]], 8, 8);
        assert_eq!(repeat, r);
    }

    #[test]
    fn mask_rects_are_component_boxes_and_cover_classifies_faces() {
        let mut m = Mask::new(16, 16);
        m.add(&Rect {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        });
        m.add(&Rect {
            x: 3,
            y: 1,
            w: 1,
            h: 1,
        });
        m.add(&Rect {
            x: 10,
            y: 0,
            w: 2,
            h: 4,
        });
        let rects = m.rects();
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 10,
                    y: 0,
                    w: 2,
                    h: 4
                },
                Rect {
                    x: 1,
                    y: 1,
                    w: 3,
                    h: 2
                },
            ]
        );
        assert_eq!(ron_list(&rects), "[(10, 0, 2, 4), (1, 1, 3, 2)]");
        let team = Mask::from_rects(16, 16, &rects);
        assert_eq!(
            team.cover(&Rect {
                x: 1,
                y: 1,
                w: 1,
                h: 1
            }),
            Cover::Inside
        );
        assert_eq!(
            team.cover(&Rect {
                x: 5,
                y: 5,
                w: 2,
                h: 2
            }),
            Cover::Outside
        );
        assert_eq!(
            team.cover(&Rect {
                x: 0,
                y: 0,
                w: 2,
                h: 2
            }),
            Cover::Straddles
        );
        assert_eq!(team.count(), 8 + 6);
    }

    #[test]
    fn overlapping_boxes_merge() {
        let merged = merge_overlapping(vec![
            Rect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            Rect {
                x: 3,
                y: 3,
                w: 4,
                h: 4,
            },
            Rect {
                x: 20,
                y: 0,
                w: 1,
                h: 1,
            },
        ]);
        assert_eq!(
            merged,
            vec![
                Rect {
                    x: 0,
                    y: 0,
                    w: 7,
                    h: 7
                },
                Rect {
                    x: 20,
                    y: 0,
                    w: 1,
                    h: 1
                },
            ]
        );
    }
}
