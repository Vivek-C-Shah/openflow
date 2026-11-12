//! Platform-independent drawing for the overlay pill and tray icon.
//! Shapes are signed-distance fields with analytic anti-aliasing, rendered
//! into a small premultiplied-alpha float canvas. No GPU, no UI framework.

#[derive(Clone)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    px: Vec<[f32; 4]>, // premultiplied RGBA
}

pub type Rgba = [f32; 4]; // straight (non-premultiplied) RGBA, 0..1

pub fn rgba(hex: u32, a: f32) -> Rgba {
    [((hex >> 16) & 0xff) as f32 / 255.0, ((hex >> 8) & 0xff) as f32 / 255.0, (hex & 0xff) as f32 / 255.0, a]
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, px: vec![[0.0; 4]; w * h] }
    }

    /// Composite a shape given by its signed distance (in pixels) over the canvas.
    /// `bounds` = (x0, y0, x1, y1) in pixels limits the work.
    pub fn fill(&mut self, bounds: (f32, f32, f32, f32), color: Rgba, sdf: impl Fn(f32, f32) -> f32) {
        if color[3] <= 0.0 {
            return;
        }
        let x0 = (bounds.0 - 1.0).floor().max(0.0) as usize;
        let y0 = (bounds.1 - 1.0).floor().max(0.0) as usize;
        let x1 = ((bounds.2 + 1.0).ceil() as usize).min(self.w);
        let y1 = ((bounds.3 + 1.0).ceil() as usize).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let a = color[3] * cov;
                let p = &mut self.px[y * self.w + x];
                for c in 0..3 {
                    p[c] = color[c] * a + p[c] * (1.0 - a);
                }
                p[3] = a + p[3] * (1.0 - a);
            }
        }
    }

    pub fn round_rect(&mut self, cx: f32, cy: f32, w: f32, h: f32, r: f32, color: Rgba) {
        let (hw, hh) = (w / 2.0, h / 2.0);
        let r = r.min(hw).min(hh);
        self.fill((cx - hw, cy - hh, cx + hw, cy + hh), color, |x, y| sd_round_rect(x - cx, y - cy, hw, hh, r));
    }

    /// Outline of a rounded rect, `t` pixels thick, inside the shape's edge.
    pub fn round_rect_stroke(&mut self, cx: f32, cy: f32, w: f32, h: f32, r: f32, t: f32, color: Rgba) {
        let (hw, hh) = (w / 2.0, h / 2.0);
        let r = r.min(hw).min(hh);
        self.fill((cx - hw, cy - hh, cx + hw, cy + hh), color, |x, y| {
            let d = sd_round_rect(x - cx, y - cy, hw, hh, r);
            (d + t / 2.0).abs() - t / 2.0
        });
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgba) {
        self.fill((cx - r, cy - r, cx + r, cy + r), color, |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r);
    }

    /// Line segment with round caps.
    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: Rgba) {
        let r = width / 2.0;
        let bounds = (a.0.min(b.0) - r, a.1.min(b.1) - r, a.0.max(b.0) + r, a.1.max(b.1) + r);
        self.fill(bounds, color, |x, y| sd_segment(x, y, a, b) - r);
    }

    /// BGRA, premultiplied (what Windows `UpdateLayeredWindow` and icons want).
    pub fn to_bgra_premultiplied(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            out.extend([to8(p[2]), to8(p[1]), to8(p[0]), to8(p[3])]);
        }
        out
    }

    /// RGBA, straight alpha (PNG, macOS/Linux icons).
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            let a = p[3];
            let un = |c: f32| if a > 0.0 { to8(c / a) } else { 0 };
            out.extend([un(p[0]), un(p[1]), un(p[2]), to8(a)]);
        }
        out
    }
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub fn sd_round_rect(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn sd_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (pax, pay) = (px - a.0, py - a.1);
    let (bax, bay) = (b.0 - a.0, b.1 - a.1);
    let len2 = bax * bax + bay * bay;
    let h = if len2 > 0.0 { ((pax * bax + pay * bay) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((pax - bax * h).powi(2) + (pay - bay * h).powi(2)).sqrt()
}

// ───────────────────────────── overlay ─────────────────────────────

pub const BARS: usize = 9;
/// Logical (unscaled) size of the overlay window. The pill sits bottom-center.
pub const CANVAS_W: f32 = 220.0;
pub const CANVAS_H: f32 = 60.0;
const BOTTOM_PAD: f32 = 4.0;

/// Animated visual state; every field eases toward its target.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub width: f32,
    pub height: f32,
    pub opacity: f32,
    /// 0 = idle bar, 1 = fully expanded content.
    pub expand: f32,
    /// 0..1: reveal of hands-free cancel/stop buttons.
    pub controls: f32,
    /// 0..1: "thinking" shimmer instead of live bars.
    pub processing: f32,
    /// 0..1: red error tint.
    pub error: f32,
    pub bars: [f32; BARS],
    /// Seconds since start, drives the shimmer.
    pub time: f32,
}

impl Look {
    pub fn idle(visible: bool) -> Look {
        Look {
            width: 44.0,
            height: 8.0,
            opacity: if visible { 1.0 } else { 0.0 },
            expand: 0.0,
            controls: 0.0,
            processing: 0.0,
            error: 0.0,
            bars: [0.0; BARS],
            time: 0.0,
        }
    }
}

/// Geometry shared by drawing and click hit-testing (logical units).
pub struct Layout {
    pub cx: f32,
    pub cy: f32,
    pub cancel: (f32, f32),
    pub stop: (f32, f32),
    pub button_r: f32,
}

pub fn layout(l: &Look) -> Layout {
    let cx = CANVAS_W / 2.0;
    let cy = CANVAS_H - BOTTOM_PAD - l.height / 2.0;
    let inset = 17.0;
    Layout {
        cx,
        cy,
        cancel: (cx - l.width / 2.0 + inset, cy),
        stop: (cx + l.width / 2.0 - inset, cy),
        button_r: 11.0,
    }
}

const BG: u32 = 0x0e0e12;
const RED: u32 = 0xef4444;

pub fn draw_overlay(l: &Look, scale: f32) -> Canvas {
    let mut c = Canvas::new((CANVAS_W * scale).ceil() as usize, (CANVAS_H * scale).ceil() as usize);
    if l.opacity <= 0.003 {
        return c;
    }
    let s = scale;
    let o = l.opacity;
    let g = layout(l);
    let (cx, cy, w, h) = (g.cx * s, g.cy * s, l.width * s, l.height * s);
    let r = h / 2.0;

    // soft shadow
    for (grow, a) in [(6.0, 0.05), (3.5, 0.08), (1.5, 0.12)] {
        c.round_rect(cx, cy + 1.5 * s, w + grow * s, h + grow * s, r + grow * s / 2.0, rgba(0x000000, a * o * (0.4 + 0.6 * l.expand)));
    }
    // body + hairline border
    c.round_rect(cx, cy, w, h, r, rgba(BG, (0.78 + 0.16 * l.expand) * o));
    let border = mix(rgba(0xffffff, 0.16 + 0.04 * l.expand), rgba(RED, 0.9), l.error);
    c.round_rect_stroke(cx, cy, w, h, r, 1.0 * s.max(1.0), [border[0], border[1], border[2], border[3] * o]);

    let content = ((l.expand - 0.35) / 0.65).clamp(0.0, 1.0) * o;
    if content <= 0.0 {
        return c;
    }

    // voice bars / thinking shimmer
    let bar_w = 3.0 * s;
    let gap = 3.4 * s;
    let span = BARS as f32 * bar_w + (BARS - 1) as f32 * gap;
    let shift = (l.controls * 0.0) * s; // bars stay centered between the buttons
    let x0 = cx - span / 2.0 + bar_w / 2.0 + shift;
    let bar_color = mix(rgba(0xffffff, 0.95), rgba(0xfca5a5, 0.95), l.error);
    for i in 0..BARS {
        let x = x0 + i as f32 * (bar_w + gap);
        let live = 3.0 + l.bars[i].clamp(0.0, 1.0) * 17.0;
        let wave = (l.time * 7.0 - i as f32 * 0.75).sin().max(0.0);
        let think = 3.0 + wave * 3.0;
        let bh = (live * (1.0 - l.processing) + think * l.processing) * s;
        let alpha = (1.0 - l.processing) + l.processing * (0.35 + 0.65 * wave);
        c.round_rect(x, cy, bar_w, bh.max(bar_w), bar_w / 2.0, [bar_color[0], bar_color[1], bar_color[2], bar_color[3] * alpha * content]);
    }

    // hands-free buttons: cancel (x) left, stop (red, square) right
    let k = (l.controls * content).clamp(0.0, 1.0);
    if k > 0.0 {
        let br = g.button_r * s * (0.7 + 0.3 * k);
        let (ax, ay) = (g.cancel.0 * s, g.cancel.1 * s);
        c.circle(ax, ay, br, rgba(0xffffff, 0.12 * k));
        let d = 3.6 * s;
        c.line((ax - d, ay - d), (ax + d, ay + d), 1.6 * s, rgba(0xffffff, 0.85 * k));
        c.line((ax - d, ay + d), (ax + d, ay - d), 1.6 * s, rgba(0xffffff, 0.85 * k));
        let (sx, sy) = (g.stop.0 * s, g.stop.1 * s);
        c.circle(sx, sy, br, rgba(RED, 0.95 * k));
        c.round_rect(sx, sy, 7.5 * s, 7.5 * s, 1.8 * s, rgba(0xffffff, k));
    }
    c
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// Tray / app icon: a dark pill with three voice bars (red while recording).
pub fn draw_icon(size: usize, recording: bool) -> Canvas {
    let mut c = Canvas::new(size, size);
    let s = size as f32 / 32.0;
    let (cx, cy) = (16.0 * s, 16.0 * s);
    let fill = if recording { rgba(RED, 1.0) } else { rgba(0x18181b, 1.0) };
    c.round_rect(cx, cy, 30.0 * s, 20.0 * s, 10.0 * s, fill);
    c.round_rect_stroke(cx, cy, 30.0 * s, 20.0 * s, 10.0 * s, 1.6 * s, rgba(0xffffff, 0.9));
    for (i, h) in [7.0, 12.0, 7.0].iter().enumerate() {
        let x = cx + (i as f32 - 1.0) * 6.0 * s;
        c.round_rect(x, cy, 3.2 * s, h * s, 1.6 * s, rgba(0xffffff, 1.0));
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_overlay_is_fully_transparent() {
        let c = draw_overlay(&Look::idle(false), 1.0);
        assert!(c.to_rgba().chunks(4).all(|p| p[3] == 0));
    }

    #[test]
    fn idle_bar_draws_something_small() {
        let c = draw_overlay(&Look::idle(true), 2.0);
        let opaque = c.to_rgba().chunks(4).filter(|p| p[3] > 128).count();
        assert!(opaque > 300 && opaque < 4000, "{opaque}");
    }

    #[test]
    fn icon_is_opaque_in_center() {
        let c = draw_icon(32, false);
        let p = &c.to_rgba()[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 4];
        assert_eq!(p[3], 255);
    }
}
