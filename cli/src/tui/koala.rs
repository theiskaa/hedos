//! The koala in the header, drawn the way theiskaa.com/art draws it: the
//! banner's braille decoded into dots, the dots chained into one path around
//! the outline, and the path redrawn every frame.
//!
//! At launch a pen draws the path in. Then the koala rocks from its feet,
//! the top swinging wider than the base, breathing a little; every six
//! seconds a shine runs the length of the outline and the koala blinks
//! twice. Braille holds a 2×4 grid of dots per cell, so the koala moves by
//! half a cell at a time.

use std::sync::LazyLock;

use ratatui::style::Color;

use super::motion::{Motion, drift};
use super::palette::{BRIGHT, GHOST, INK_COLOR, SOFT_COLOR, WHITE, mix};
use crate::support::banner::KOALA;

/// The koala's box in cells: its 18 columns and a column either side to sway
/// into.
pub(crate) const WIDTH: usize = 20;
pub(crate) const HEIGHT: usize = KOALA.len();
/// Dot columns either side of the drawing, the room the sway takes.
const PAD_DOTS: usize = 2;
/// How long the pen takes to draw the koala in.
const INTRO_S: f32 = 1.0;
/// How many dots behind the pen are still cooling from white.
const PEN_TRAIL: f32 = 10.0;
/// The shine and the blinks come round this often.
const CYCLE_S: f32 = 6.0;
/// How long the shine takes to run the path.
const SHINE_S: f32 = 1.8;
/// When the eyes are shut, in seconds into the cycle.
const BLINKS: [(f32, f32); 2] = [(2.3, 2.45), (2.65, 2.8)];
/// The braille bit for each dot of a cell, by `(column, row)`.
const BITS: [(usize, usize); 8] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (1, 0),
    (1, 1),
    (1, 2),
    (0, 3),
    (1, 3),
];
/// The upper dot row of each eye; shut, only the lower row shows, a dash.
const EYES: [(usize, usize); 4] = [(15, 9), (16, 9), (24, 9), (25, 9)];
/// The rows of dots the drawing spans, for the sway's pivot at the feet.
const DOT_ROWS: usize = HEIGHT * 4;

/// One dot of the drawing, in dot coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dot {
    x: usize,
    y: usize,
}

/// Every lit dot of the banner's koala, chained nearest first from the
/// top-left dot, so a movement can travel along the outline.
static PATH: LazyLock<Vec<Dot>> = LazyLock::new(|| chain(dots()));

fn dots() -> Vec<Dot> {
    let mut dots = Vec::new();
    for (row, line) in KOALA.iter().enumerate() {
        for (column, glyph) in line.chars().enumerate() {
            let bits = u32::from(glyph).saturating_sub(0x2800);
            for (bit, (dx, dy)) in BITS.iter().enumerate() {
                if bits >> bit & 1 == 1 {
                    dots.push(Dot {
                        x: column * 2 + dx,
                        y: row * 4 + dy,
                    });
                }
            }
        }
    }
    dots
}

fn chain(mut left: Vec<Dot>) -> Vec<Dot> {
    let mut path = Vec::with_capacity(left.len());
    let mut head = left
        .iter()
        .enumerate()
        .min_by_key(|(_, dot)| (dot.y, dot.x))
        .map_or(0, |(index, _)| index);
    while !left.is_empty() {
        let current = left.swap_remove(head);
        path.push(current);
        head = left
            .iter()
            .enumerate()
            .min_by_key(|(_, dot)| {
                let dx = dot.x as i64 - current.x as i64;
                let dy = dot.y as i64 - current.y as i64;
                dx * dx + dy * dy
            })
            .map_or(0, |(index, _)| index);
    }
    path
}

/// A cell of the koala: its braille glyph and its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Cell {
    pub glyph: char,
    pub fg: Color,
}

/// What a cell gathers from the dots that land in it.
#[derive(Debug, Clone, Copy, Default)]
struct Gathered {
    bits: u32,
    glow: f32,
    pen: f32,
}

/// The koala at the clock's time, `HEIGHT` rows of `WIDTH` cells, `None`
/// where no dot lands. Settled, it is the whole drawing at rest.
pub(crate) fn frame(motion: &Motion) -> Vec<Vec<Option<Cell>>> {
    let mut gathered = vec![vec![Gathered::default(); WIDTH]; HEIGHT];
    let t = motion.age(0);
    let n = PATH.len();
    let count = n as f32;
    let cycle = t.filter(|t| *t > INTRO_S).map(|t| (t - INTRO_S) % CYCLE_S);
    let shut = cycle.is_some_and(|cycle| BLINKS.iter().any(|&(a, b)| cycle > a && cycle < b));
    let sway = t.map_or(0.0, |t| drift(t, 1.3, 2.1) * 0.65);
    let pen = t.filter(|t| *t < INTRO_S).map(|t| t / INTRO_S * count);
    let swell = t.map(|t| (1.0 - (t * 0.021).rem_euclid(1.0)) * count);
    for (index, dot) in PATH.iter().enumerate() {
        let i = index as f32;
        if pen.is_some_and(|pen| i > pen) {
            break;
        }
        if shut && EYES.contains(&(dot.x, dot.y)) {
            continue;
        }
        let lift = 1.0 - dot.y as f32 / (DOT_ROWS - 1) as f32;
        let shift = (sway * lift * 2.4).round() as isize;
        let x = (dot.x + PAD_DOTS) as isize + shift;
        let Ok(x) = usize::try_from(x) else { continue };
        let (column, row) = (x / 2, dot.y / 4);
        if column >= WIDTH || row >= HEIGHT {
            continue;
        }
        let mut glow = 0.0;
        if let Some(cycle) = cycle {
            let u = (i + 15.0) / (count + 30.0);
            let reached = SHINE_S * (1.0 - 2.0 * u).acos() / std::f32::consts::PI;
            if cycle >= reached {
                glow = (-(cycle - reached) * 2.2).exp();
            }
        }
        if let Some(swell) = swell {
            let distance = (i - swell).abs();
            let far = distance.min(count - distance);
            glow += (-(far * far) / 1600.0).exp() * 0.25;
        }
        let pen_glow = pen.map_or(0.0, |pen| {
            let behind = pen - i;
            if behind < PEN_TRAIL {
                1.0 - behind / PEN_TRAIL
            } else {
                0.0
            }
        });
        let bit = BITS
            .iter()
            .position(|&cell| cell == (x % 2, dot.y % 4))
            .unwrap_or(0);
        let cell = &mut gathered[row][column];
        cell.bits |= 1 << bit;
        cell.glow = cell.glow.max(glow);
        cell.pen = cell.pen.max(pen_glow);
    }
    let breathe = t.map_or(1.0, |t| 0.9 + 0.1 * drift(t, 0.61, 1.07));
    let rest = mix(GHOST, SOFT_COLOR, 0.55 * breathe);
    gathered
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|cell| {
                    let glyph = char::from_u32(0x2800 + cell.bits)?;
                    if cell.bits == 0 {
                        return None;
                    }
                    let mut fg = mix(rest, INK_COLOR, cell.glow.min(1.0));
                    if cell.glow > 0.55 {
                        fg = mix(fg, BRIGHT, (cell.glow - 0.55) / 0.45);
                    }
                    if cell.pen > 0.0 {
                        fg = mix(fg, WHITE, cell.pen);
                    }
                    Some(Cell { glyph, fg })
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_at(seconds: f32) -> Motion {
        let mut motion = Motion::from_env_value_for_tests(None);
        motion.set((seconds * 1000.0) as u64);
        motion
    }

    fn text(frame: &[Vec<Option<Cell>>]) -> Vec<String> {
        frame
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.map_or('\u{2800}', |cell| cell.glyph))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_path_visits_every_lit_dot_once() {
        let mut path = PATH.clone();
        let mut dots = dots();
        assert_eq!(path.len(), dots.len());
        path.sort_by_key(|dot| (dot.y, dot.x));
        dots.sort_by_key(|dot| (dot.y, dot.x));
        assert_eq!(path, dots);
    }

    #[test]
    fn settled_it_is_the_banners_koala_with_room_either_side() {
        let rows = text(&frame(&Motion::settled()));
        for (row, banner) in rows.iter().zip(KOALA) {
            assert_eq!(row, &format!("\u{2800}{banner}\u{2800}"));
        }
    }

    #[test]
    fn the_pen_draws_it_in() {
        let start = frame(&live_at(0.0));
        assert!(start.iter().flatten().filter(|cell| cell.is_some()).count() <= 1);
        let dots_in = |frame: Vec<Vec<Option<Cell>>>| -> u32 {
            frame
                .into_iter()
                .flatten()
                .flatten()
                .map(|cell| (u32::from(cell.glyph) - 0x2800).count_ones())
                .sum()
        };
        let halfway = dots_in(frame(&live_at(INTRO_S / 2.0)));
        assert!(halfway > 0 && halfway < PATH.len() as u32);
        assert_eq!(dots_in(frame(&live_at(INTRO_S + 0.01))), PATH.len() as u32);
    }

    #[test]
    fn a_blink_takes_only_the_eyes() {
        let open_at = INTRO_S + 2.0;
        let shut_at = INTRO_S + 2.35;
        let dots_in = |frame: &[Vec<Option<Cell>>]| -> u32 {
            frame
                .iter()
                .flatten()
                .flatten()
                .map(|cell| (u32::from(cell.glyph) - 0x2800).count_ones())
                .sum()
        };
        let open = frame(&live_at(open_at));
        let shut = frame(&live_at(shut_at));
        assert_eq!(dots_in(&open) - dots_in(&shut), EYES.len() as u32);
    }

    #[test]
    fn the_feet_never_move() {
        let settled = text(&frame(&Motion::settled()));
        for seconds in [2.0, 3.3, 7.9, 12.4] {
            let rows = text(&frame(&live_at(seconds)));
            assert_eq!(rows[HEIGHT - 1], settled[HEIGHT - 1], "at {seconds}s");
        }
    }

    #[test]
    fn every_glyph_is_braille() {
        for seconds in [0.4, 1.0, 4.0, 9.0] {
            for cell in frame(&live_at(seconds)).into_iter().flatten().flatten() {
                assert!(('\u{2801}'..='\u{28ff}').contains(&cell.glyph));
            }
        }
    }
}
