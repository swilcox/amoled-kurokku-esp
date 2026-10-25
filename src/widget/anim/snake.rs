//! `snake`: the computer plays Snake on a 24×24 grid. It takes the shortest
//! path to the food when it can still reach its own tail afterwards, and
//! otherwise wanders where there's most room. When it's boxed in, it blinks
//! and starts over.

use std::collections::VecDeque;

use super::super::{Step, Widget};
use crate::clock::LocalTime;
use crate::color::rgb565;
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::rng::Rng;

const N: i32 = 24;
const CELL: i32 = 20;
const STEP_MS: u32 = 80;
const GROW: u32 = 2;
/// Long enough: blink and start again.
const MAX_LEN: usize = 220;
const BLINK_MS: u32 = 220;
const BLINKS: u32 = 6;

const BODY: [u16; 2] = [rgb565(70, 190, 70), rgb565(40, 150, 50)];
const HEAD: u16 = rgb565(130, 235, 100);
const EYE: u16 = rgb565(10, 30, 10);
const FOOD: u16 = rgb565(235, 50, 45);
const LEAF: u16 = rgb565(90, 200, 70);

type Cell = (i32, i32);
const DIRS: [Cell; 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

pub struct Snake {
    rng: Rng,
    /// Head first, with each segment's color.
    body: VecDeque<(Cell, u16)>,
    dir: Cell,
    food: Cell,
    grow: u32,
    /// Segments laid so far, for the stripe pattern.
    laid: u32,
    next_step: u32,
    /// Blinking out after a game ends: when it started.
    dying: Option<u32>,
    /// Whether the body shows at this point of the blinking.
    blink_on: bool,
    first: bool,
}

fn cell_rect((x, y): Cell) -> Rect {
    Rect::from_size(x * CELL, y * CELL, CELL, CELL)
}

fn inside((x, y): Cell) -> bool {
    (0..N).contains(&x) && (0..N).contains(&y)
}

fn add(a: Cell, b: Cell) -> Cell {
    (a.0 + b.0, a.1 + b.1)
}

/// Breadth-first search over free cells from `from`: (distance grid,
/// cells reached). `blocked` marks occupied cells.
fn bfs(from: Cell, blocked: &[bool]) -> (Vec<u16>, usize) {
    let mut dist = vec![u16::MAX; (N * N) as usize];
    let mut q = VecDeque::from([from]);
    dist[(from.1 * N + from.0) as usize] = 0;
    let mut reached = 0;
    while let Some(c) = q.pop_front() {
        reached += 1;
        let d = dist[(c.1 * N + c.0) as usize];
        for dir in DIRS {
            let n = add(c, dir);
            if inside(n) {
                let i = (n.1 * N + n.0) as usize;
                if !blocked[i] && dist[i] == u16::MAX {
                    dist[i] = d + 1;
                    q.push_back(n);
                }
            }
        }
    }
    (dist, reached)
}

/// Occupied cells of `body`, leaving out its last `free_tail` segments
/// (they'll have moved on by the time the head gets there).
fn blocked(body: &VecDeque<Cell>, free_tail: usize) -> Vec<bool> {
    let mut b = vec![false; (N * N) as usize];
    for &(x, y) in body.iter().take(body.len().saturating_sub(free_tail)) {
        b[(y * N + x) as usize] = true;
    }
    b
}

impl Snake {
    pub fn new(rng: Rng) -> Self {
        let mut s = Self {
            rng,
            body: VecDeque::new(),
            dir: (1, 0),
            food: (0, 0),
            grow: 0,
            laid: 0,
            next_step: 0,
            dying: None,
            blink_on: true,
            first: true,
        };
        s.restart();
        s
    }

    fn restart(&mut self) {
        self.body.clear();
        self.laid = 0;
        for x in (N / 2 - 3..=N / 2).rev() {
            let c = self.stripe();
            self.body.push_back(((x, N / 2), c));
        }
        self.dir = (1, 0);
        self.grow = 0;
        self.place_food();
    }

    fn stripe(&mut self) -> u16 {
        self.laid += 1;
        BODY[(self.laid % 4 == 0) as usize]
    }

    fn cells(&self) -> VecDeque<Cell> {
        self.body.iter().map(|&(c, _)| c).collect()
    }

    fn place_food(&mut self) {
        let free: Vec<Cell> =
            (0..N * N).map(|i| (i % N, i / N)).filter(|c| !self.body.iter().any(|(b, _)| b == c)).collect();
        if !free.is_empty() {
            self.food = free[self.rng.below(free.len() as u32) as usize];
        }
    }

    /// The direction to move next, or None when there's nowhere to go.
    fn choose(&self) -> Option<Cell> {
        let body = self.cells();
        let head = body[0];
        // The tail moves out of the way unless the snake is growing.
        let free_tail = if self.grow > 0 { 0 } else { 1 };
        let (dist, _) = bfs(head, &blocked(&body, free_tail));
        let at = |c: Cell| dist[(c.1 * N + c.0) as usize];
        if at(self.food) != u16::MAX && self.safe_after_eating(&dist) {
            // Walk back from the food to find the first step.
            let mut c = self.food;
            while at(c) > 1 {
                c = DIRS.iter().map(|&d| add(c, d)).find(|&n| inside(n) && at(n) == at(c) - 1).unwrap();
            }
            return Some((c.0 - head.0, c.1 - head.1));
        }
        // No safe meal: go where there's most room, keeping the tail in
        // reach when possible.
        let mut best: Option<(bool, usize, Cell)> = None;
        for d in DIRS {
            let n = add(head, d);
            if !inside(n) || blocked(&body, free_tail)[(n.1 * N + n.0) as usize] {
                continue;
            }
            let mut moved = body.clone();
            moved.push_front(n);
            if free_tail > 0 {
                moved.pop_back();
            }
            let tail = *moved.back().unwrap();
            let b = blocked(&moved, 1);
            let (dist, room) = bfs(n, &b);
            let tail_ok = dist[(tail.1 * N + tail.0) as usize] != u16::MAX;
            if best.is_none_or(|(t, r, _)| (tail_ok, room) > (t, r)) {
                best = Some((tail_ok, room, d));
            }
        }
        best.map(|(_, _, d)| d)
    }

    /// Would the snake, having followed the shortest path to the food,
    /// still be able to reach its tail?
    fn safe_after_eating(&self, dist: &[u16]) -> bool {
        let at = |c: Cell| dist[(c.1 * N + c.0) as usize];
        let mut path = vec![self.food];
        let mut c = self.food;
        while at(c) > 1 {
            c = DIRS.iter().map(|&d| add(c, d)).find(|&n| inside(n) && at(n) == at(c) - 1).unwrap();
            path.push(c);
        }
        let mut body = self.cells();
        let mut grow = self.grow;
        for &step in path.iter().rev() {
            body.push_front(step);
            if grow > 0 {
                grow -= 1;
            } else {
                body.pop_back();
            }
        }
        // Eating adds GROW segments, which then trail behind the tail.
        let tail = *body.back().unwrap();
        let (d, _) = bfs(body[0], &blocked(&body, 1));
        d[(tail.1 * N + tail.0) as usize] != u16::MAX
    }

    /// Move one cell; returns the cells that changed.
    fn advance(&mut self) -> Vec<Rect> {
        let Some(dir) = self.choose() else {
            self.dying = Some(self.next_step);
            return vec![];
        };
        self.dir = dir;
        let head = add(self.body[0].0, dir);
        let mut dirty = vec![cell_rect(self.body[0].0), cell_rect(head)];
        let c = self.stripe();
        self.body.push_front((head, c));
        if self.grow > 0 {
            self.grow -= 1;
        } else if let Some((tail, _)) = self.body.pop_back() {
            dirty.push(cell_rect(tail));
            dirty.extend(self.body.back().map(|&(c, _)| cell_rect(c)));
        }
        if head == self.food {
            self.grow += GROW;
            self.place_food();
            dirty.push(cell_rect(self.food));
        }
        if self.body.len() >= MAX_LEN {
            self.dying = Some(self.next_step);
        }
        dirty
    }

}

impl Widget for Snake {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        if std::mem::replace(&mut self.first, false) {
            self.next_step = now_ms;
            return Step::Redraw(vec![SCREEN]);
        }
        if let Some(t) = self.dying {
            let k = now_ms.wrapping_sub(t) / BLINK_MS;
            if k >= BLINKS {
                self.dying = None;
                self.blink_on = true;
                self.restart();
                self.next_step = now_ms.wrapping_add(STEP_MS * 4);
                return Step::Redraw(vec![SCREEN]);
            }
            // Blink: repaint the body each time it shows or hides.
            let on = k % 2 == 1;
            if std::mem::replace(&mut self.blink_on, on) != on {
                return Step::Redraw(self.body.iter().map(|&(c, _)| cell_rect(c)).collect());
            }
            return Step::Redraw(vec![]);
        }
        let mut dirty = Vec::new();
        // Catch up if a frame was late, but don't race.
        let mut n = 0;
        while now_ms.wrapping_sub(self.next_step) < u32::MAX / 2 && n < 3 && self.dying.is_none() {
            dirty.extend(self.advance());
            self.next_step = self.next_step.wrapping_add(STEP_MS);
            n += 1;
        }
        Step::Redraw(dirty)
    }

    fn invalidate(&mut self) {
        self.first = true;
    }

    fn frame_ms(&self) -> u32 {
        20
    }
}

impl Scene for Snake {
    fn draw(&self, c: &mut Canvas) {
        c.fill(c.area(), 0);
        // Food: an apple with a leaf.
        let f = cell_rect(self.food);
        let (fx, fy) = ((f.x0 + f.x1 + 1) * 128, (f.y0 + f.y1 + 1) * 128 + 256);
        let r = 7 * 256;
        c.shade(f, FOOD, |x, y| {
            let (dx, dy) = (x * 256 + 128 - fx, y * 256 + 128 - fy);
            ((r * r - dx * dx - dy * dy) / (2 * r) + 128).clamp(0, 255) as u8
        });
        c.fill(Rect::from_size(f.x0 + 10, f.y0 + 2, 4, 3), LEAF);

        if !self.blink_on {
            return;
        }
        let inset = |r: Rect| Rect::new(r.x0 + 2, r.y0 + 2, r.x1 - 2, r.y1 - 2);
        // Bridges between neighbors first, so the body reads as one piece.
        for (a, b) in self.body.iter().zip(self.body.iter().skip(1)) {
            let (ra, rb) = (inset(cell_rect(a.0)), inset(cell_rect(b.0)));
            let (ca, cb) = ((ra.x0 + ra.x1) / 2, (rb.x0 + rb.x1) / 2);
            let (ma, mb) = ((ra.y0 + ra.y1) / 2, (rb.y0 + rb.y1) / 2);
            let bridge = if a.0 .1 == b.0 .1 {
                Rect::new(ca.min(cb), ra.y0, ca.max(cb), ra.y1)
            } else {
                Rect::new(ra.x0, ma.min(mb), ra.x1, ma.max(mb))
            };
            c.fill(bridge, b.1);
        }
        for (i, &(cell, color)) in self.body.iter().enumerate() {
            let r = inset(cell_rect(cell));
            c.fill_round_rect(r, 6, if i == 0 { HEAD } else { color }, if i == 0 { HEAD } else { color }, true, true);
        }
        // Eyes, toward the front of the head.
        let h = cell_rect(self.body[0].0);
        let (cx, cy) = (h.x0 + CELL / 2, h.y0 + CELL / 2);
        let (dx, dy) = self.dir;
        for side in [-1, 1] {
            let (ex, ey) = (cx + dx * 4 + dy * side * 5, cy + dy * 4 - dx * side * 5);
            c.fill(Rect::new(ex - 2, ey - 2, ex + 1, ey + 1), EYE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::check_incremental;
    use super::*;

    #[test]
    fn plays_eats_and_restarts() {
        let mut s = Snake::new(Rng::new(5));
        let t = LocalTime::default();
        let (mut longest, mut restarts) = (0, 0);
        for ms in (0..400_000).step_by(20) {
            let before = s.body.len();
            s.tick(ms, &t);
            longest = longest.max(s.body.len());
            if s.body.len() < before {
                restarts += 1;
            }
        }
        assert!(longest > 40, "snake should grow, got {longest}");
        assert!(restarts >= 1, "a game should end within the run");
    }

    #[test]
    fn incremental_redraws_match() {
        let t = LocalTime::default();
        check_incremental(Box::new(Snake::new(Rng::new(9))), (0..600u32).map(|i| (i * 20, t)));
    }
}
