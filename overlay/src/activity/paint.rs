//! Software rasteriser for the activity badge.
//!
//! Everything here is platform independent so the animation can be rendered
//! and inspected anywhere. Frames are premultiplied BGRA (`0xAARRGGBB`), the
//! layout `UpdateLayeredWindow` expects.

use std::f32::consts::TAU;

/// What the badge is currently telling the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Claude is working on a turn.
    Working,
    /// Claude is blocked on the user (permission prompt, question, dialog).
    Attention,
    /// The turn ended because of an API error.
    Failure,
    /// The turn finished.
    Done,
}

#[derive(Clone, Copy)]
struct Rgb(f32, f32, f32);

const ORANGE: Rgb = Rgb(0.851, 0.467, 0.341); // #D97757
const ORANGE_LIGHT: Rgb = Rgb(0.957, 0.659, 0.533); // #F4A888
const AMBER: Rgb = Rgb(0.918, 0.616, 0.180); // #EA9D2E
const RED: Rgb = Rgb(0.871, 0.282, 0.251); // #DE4840
const GREEN: Rgb = Rgb(0.169, 0.651, 0.302); // #2BA64D
const WHITE: Rgb = Rgb(1.0, 1.0, 1.0);

/// Render one frame.
///
/// * `t` - seconds since the badge entered `look`
/// * `clock` - free running seconds, keeps continuous motion smooth across
///   state changes
/// * `sessions` - how many sessions share this look (dots under the spark)
/// * `opacity` - 0..1 fade multiplier
pub fn render(look: Look, t: f32, clock: f32, size: u32, sessions: u32, opacity: f32) -> Vec<u32> {
    let mut canvas = Canvas::new(size);
    match look {
        Look::Working => working(&mut canvas, t, clock, sessions),
        Look::Attention => attention(&mut canvas, t, clock, AMBER),
        Look::Failure => attention(&mut canvas, t, clock, RED),
        Look::Done => done(&mut canvas, t, clock),
    }
    canvas.into_bgra(opacity)
}

fn working(c: &mut Canvas, t: f32, clock: f32, sessions: u32) {
    let u = c.unit();
    let (cx, cy) = c.centre();
    let appear = ease_out_back(clamp01(t / 0.35));

    let glow = 0.20 + 0.08 * (clock * 2.6).sin();
    c.glow(cx, cy, 17.0 * u * appear, ORANGE, glow);

    // A pinwheel of rounded rays: the whole spark turns slowly while a wave
    // of length runs around it, the way the terminal spinner breathes.
    let rotation = clock * 0.75;
    let rays = 8;
    for i in 0..rays {
        let fi = i as f32;
        let angle = rotation + fi * TAU / rays as f32;
        let wave = 0.5 + 0.5 * (clock * TAU / 1.3 - fi * TAU / rays as f32).sin();
        let inner = 3.4 * u;
        let outer = (9.0 + 7.0 * wave) * u * appear;
        let radius = (1.75 + 0.55 * wave) * u * appear;
        let (s, k) = angle.sin_cos();
        let colour = mix(ORANGE, ORANGE_LIGHT, wave * 0.55);
        c.capsule(
            cx + k * inner,
            cy + s * inner,
            cx + k * outer,
            cy + s * outer,
            radius,
            colour,
            1.0,
        );
    }
    c.circle(cx, cy, 3.0 * u * appear, ORANGE, 1.0);

    // One dot per extra session that is working at the same time.
    let extra = sessions.saturating_sub(1).min(4);
    if extra > 0 {
        let spacing = 4.2 * u;
        let start = cx - spacing * (extra as f32 - 1.0) / 2.0;
        for i in 0..extra {
            c.circle(start + spacing * i as f32, c.size_f() - 2.6 * u, 1.35 * u, ORANGE, 0.95);
        }
    }
}

fn attention(c: &mut Canvas, t: f32, clock: f32, colour: Rgb) {
    let u = c.unit();
    let (cx, cy) = c.centre();
    let appear = ease_out_back(clamp01(t / 0.4));

    // Radar ping every 1.4 s so it reads as "waiting for you".
    let period = 1.4;
    for offset in [0.0, period / 2.0] {
        let p = ((t + offset) % period) / period;
        let e = ease_out_cubic(p);
        let r = (11.0 + 11.0 * e) * u;
        c.ring(cx, cy, r, (1.6 * (1.0 - p)).max(0.2) * u, colour, 0.65 * (1.0 - p) * appear);
    }

    let bounce = 1.0 + 0.05 * (clock * 4.6).sin();
    let r = 10.8 * u * appear * bounce;
    c.glow(cx, cy, 16.0 * u * appear, colour, 0.25);
    c.circle(cx, cy, r, colour, 1.0);

    // Exclamation mark.
    let s = appear * bounce;
    c.capsule(cx, cy - 5.6 * u * s, cx, cy + 1.1 * u * s, 1.65 * u * s, WHITE, 1.0);
    c.circle(cx, cy + 5.0 * u * s, 1.8 * u * s, WHITE, 1.0);
}

fn done(c: &mut Canvas, t: f32, clock: f32) {
    let u = c.unit();
    let (cx, cy) = c.centre();

    // 1. The spark bursts outward and hands its colour over to green.
    if t < 0.5 {
        let p = clamp01(t / 0.5);
        let e = ease_out_cubic(p);
        let rays = 8;
        for i in 0..rays {
            let angle = 0.35 + i as f32 * TAU / rays as f32;
            let (s, k) = angle.sin_cos();
            let inner = (3.5 + 13.0 * e) * u;
            let outer = (10.0 + 11.0 * e) * u;
            let radius = 1.9 * u * (1.0 - 0.7 * p);
            c.capsule(
                cx + k * inner,
                cy + s * inner,
                cx + k * outer,
                cy + s * outer,
                radius,
                mix(ORANGE, GREEN, p),
                1.0 - p,
            );
        }
    }

    // 2. A green disc pops in with a little overshoot.
    let pop = ease_out_back(clamp01((t - 0.15) / 0.38));
    if pop > 0.0 {
        // 4. Settled: a soft reminder ripple every few seconds.
        let settled = t - 1.5;
        if settled > 0.0 {
            let period = 2.8;
            let p = (settled % period) / period;
            if p < 0.55 {
                let q = p / 0.55;
                let e = ease_out_cubic(q);
                c.ring(cx, cy, (11.0 + 9.0 * e) * u, 1.3 * u * (1.0 - q).max(0.2), GREEN, 0.45 * (1.0 - q));
            }
        }
        let breathe = if settled > 0.0 { 0.18 + 0.06 * (clock * 2.2).sin() } else { 0.28 };
        c.glow(cx, cy, 16.5 * u * pop, GREEN, breathe);
        c.circle(cx, cy, 11.2 * u * pop, GREEN, 1.0);
    }

    // 3. The check mark draws itself, then a ripple goes out once.
    let draw = ease_in_out(clamp01((t - 0.38) / 0.36));
    if draw > 0.0 {
        let p0 = (cx - 5.0 * u, cy + 0.4 * u);
        let p1 = (cx - 1.5 * u, cy + 4.0 * u);
        let p2 = (cx + 5.4 * u, cy - 3.8 * u);
        let l1 = dist(p0, p1);
        let l2 = dist(p1, p2);
        let drawn = draw * (l1 + l2);
        let w = 1.75 * u;
        if drawn <= l1 {
            let q = drawn / l1;
            c.capsule(p0.0, p0.1, lerp(p0.0, p1.0, q), lerp(p0.1, p1.1, q), w, WHITE, 1.0);
        } else {
            c.capsule(p0.0, p0.1, p1.0, p1.1, w, WHITE, 1.0);
            let q = (drawn - l1) / l2;
            c.capsule(p1.0, p1.1, lerp(p1.0, p2.0, q), lerp(p1.1, p2.1, q), w, WHITE, 1.0);
        }
    }
    let ripple = clamp01((t - 0.55) / 0.9);
    if ripple > 0.0 && ripple < 1.0 {
        let e = ease_out_cubic(ripple);
        c.ring(cx, cy, (11.5 + 11.0 * e) * u, 1.6 * u * (1.0 - ripple).max(0.2), GREEN, 0.7 * (1.0 - ripple));
    }
}

struct Canvas {
    size: usize,
    /// Premultiplied linear-ish RGBA accumulation buffer.
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(size: u32) -> Self {
        let size = size.max(1) as usize;
        Self {
            size,
            px: vec![[0.0; 4]; size * size],
        }
    }

    fn size_f(&self) -> f32 {
        self.size as f32
    }

    /// Design unit: the artwork is drawn on a 48 px grid.
    fn unit(&self) -> f32 {
        self.size_f() / 48.0
    }

    fn centre(&self) -> (f32, f32) {
        (self.size_f() / 2.0, self.size_f() / 2.0)
    }

    /// Shade the pixels in a bounding box with a signed distance function.
    fn shade(&mut self, bounds: (f32, f32, f32, f32), mut coverage: impl FnMut(f32, f32) -> f32, colour: Rgb, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let n = self.size as i32;
        let x0 = (bounds.0.floor() as i32).clamp(0, n);
        let y0 = (bounds.1.floor() as i32).clamp(0, n);
        let x1 = (bounds.2.ceil() as i32 + 1).clamp(0, n);
        let y1 = (bounds.3.ceil() as i32 + 1).clamp(0, n);
        for y in y0..y1 {
            for x in x0..x1 {
                let cov = coverage(x as f32 + 0.5, y as f32 + 0.5);
                if cov <= 0.0 {
                    continue;
                }
                let a = (cov * alpha).min(1.0);
                let dst = &mut self.px[y as usize * self.size + x as usize];
                let inv = 1.0 - a;
                dst[0] = colour.0 * a + dst[0] * inv;
                dst[1] = colour.1 * a + dst[1] * inv;
                dst[2] = colour.2 * a + dst[2] * inv;
                dst[3] = a + dst[3] * inv;
            }
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, colour: Rgb, alpha: f32) {
        if r <= 0.0 {
            return;
        }
        self.shade(
            (cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0),
            |x, y| edge(((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r),
            colour,
            alpha,
        );
    }

    fn ring(&mut self, cx: f32, cy: f32, r: f32, half_width: f32, colour: Rgb, alpha: f32) {
        if r <= 0.0 || half_width <= 0.0 {
            return;
        }
        let o = r + half_width + 1.0;
        self.shade(
            (cx - o, cy - o, cx + o, cy + o),
            |x, y| edge((((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() - half_width),
            colour,
            alpha,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn capsule(&mut self, ax: f32, ay: f32, bx: f32, by: f32, r: f32, colour: Rgb, alpha: f32) {
        if r <= 0.0 {
            return;
        }
        let bounds = (
            ax.min(bx) - r - 1.0,
            ay.min(by) - r - 1.0,
            ax.max(bx) + r + 1.0,
            ay.max(by) + r + 1.0,
        );
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        self.shade(
            bounds,
            |x, y| {
                let h = (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0);
                let (px, py) = (x - ax - dx * h, y - ay - dy * h);
                edge((px * px + py * py).sqrt() - r)
            },
            colour,
            alpha,
        );
    }

    /// Soft radial halo that fades to nothing at `r`.
    fn glow(&mut self, cx: f32, cy: f32, r: f32, colour: Rgb, alpha: f32) {
        if r <= 0.0 {
            return;
        }
        self.shade(
            (cx - r, cy - r, cx + r, cy + r),
            |x, y| {
                let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / r;
                let f = (1.0 - d).max(0.0);
                f * f
            },
            colour,
            alpha,
        );
    }

    fn into_bgra(self, opacity: f32) -> Vec<u32> {
        let o = clamp01(opacity);
        self.px
            .into_iter()
            .map(|[r, g, b, a]| {
                let to8 = |v: f32| (clamp01(v * o) * 255.0 + 0.5) as u32;
                (to8(a) << 24) | (to8(r) << 16) | (to8(g) << 8) | to8(b)
            })
            .collect()
    }
}

/// Anti-aliased coverage of a pixel whose centre is `d` pixels outside a shape.
fn edge(d: f32) -> f32 {
    clamp01(0.5 - d)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = clamp01(t);
    Rgb(lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - clamp01(t)).powi(3)
}

fn ease_in_out(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * (3.0 - 2.0 * t)
}

fn ease_out_back(t: f32) -> f32 {
    let t = clamp01(t);
    if t <= 0.0 {
        return 0.0;
    }
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(px: u32) -> u32 {
        px >> 24
    }

    #[test]
    fn frames_have_requested_size_and_are_premultiplied() {
        for look in [Look::Working, Look::Attention, Look::Failure, Look::Done] {
            for t in [0.0, 0.2, 0.6, 1.0, 2.5, 7.3] {
                let frame = render(look, t, t + 3.0, 40, 2, 1.0);
                assert_eq!(frame.len(), 40 * 40);
                for px in &frame {
                    let a = alpha(*px);
                    assert!((px >> 16 & 0xFF) <= a && (px >> 8 & 0xFF) <= a && (px & 0xFF) <= a);
                }
            }
        }
    }

    #[test]
    fn badge_is_visible_once_settled_and_corners_stay_clear() {
        for look in [Look::Working, Look::Attention, Look::Done] {
            let frame = render(look, 3.0, 9.0, 48, 1, 1.0);
            let centre = frame[24 * 48 + 24];
            assert!(alpha(centre) > 200, "{look:?} centre alpha {}", alpha(centre));
            assert_eq!(alpha(frame[0]), 0, "{look:?} corner must be transparent");
        }
    }

    #[test]
    fn opacity_scales_output() {
        let full = render(Look::Done, 3.0, 3.0, 32, 1, 1.0);
        let none = render(Look::Done, 3.0, 3.0, 32, 1, 0.0);
        assert!(full.iter().any(|px| alpha(*px) > 0));
        assert!(none.iter().all(|px| *px == 0));
    }
}
