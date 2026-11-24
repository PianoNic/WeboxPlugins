//! The picture effects, on RGBA frames with a row stride (what `Host::camera_filter` hands over). Each one is a pass or two
//! over the frame with integer arithmetic; anything that blurs works on a smaller copy (`Small`) and samples it back
//! bilinearly, so the cost stays flat however strong the effect is.

/// A frame being edited: RGBA, rows `stride` bytes apart.
pub struct Img<'a> { pub w: usize, pub h: usize, pub stride: usize, pub px: &'a mut [u8] }

impl Img<'_> {
    fn row(&mut self, y: usize) -> &mut [u8] { let s = y * self.stride; &mut self.px[s..s + self.w * 4] }
}

/// Threads a per-pixel pass may use: a few, so a pass over a 720p frame takes a few ms on a laptop, without filling every core.
const THREADS: usize = 4;

/// Runs `f(band, first row)` on horizontal bands of `img`, on up to `THREADS` scoped threads (all joined before it returns).
pub fn par_rows(img: &mut Img, f: impl Fn(&mut Img, usize) + Sync) {
    let n = std::thread::available_parallelism().map_or(1, |n| n.get()).min(THREADS);
    let (w, h, stride) = (img.w, img.h, img.stride);
    if n < 2 || h < 64 { return f(img, 0); }
    let rows = h.div_ceil(n);
    std::thread::scope(|sc| {
        let mut parts = img.px.chunks_mut(stride * rows).enumerate();
        let first = parts.next();
        for (i, px) in parts {
            let f = &f;
            sc.spawn(move || f(&mut Img { w, h: rows.min(h - i * rows), stride, px }, i * rows));
        }
        // the first band on this thread
        if let Some((_, px)) = first { f(&mut Img { w, h: rows.min(h), stride, px }, 0); }
    });
}

/// Brightness and contrast (-100..100, 0 = unchanged) and a gain (1 = none) as one tone curve per channel.
pub fn tone_lut(brightness: f32, contrast: f32, gain: f32) -> [u8; 256] {
    let b = brightness.clamp(-100.0, 100.0) / 100.0 * 0.4;
    let c = 1.0 + contrast.clamp(-100.0, 100.0) / 100.0 * if contrast > 0.0 { 1.0 } else { 0.6 };
    let g = gain.max(0.1);
    let mut t = [0u8; 256];
    for (i, o) in t.iter_mut().enumerate() {
        let mut v = i as f32 / 255.0;
        // soft-knee gain: lifts the shadows and mid-tones, the highlights do not clip
        v = v * g / (1.0 + (g - 1.0) * v);
        v = (v - 0.5) * c + 0.5 + b;
        *o = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    t
}

/// Whether the curve changes anything.
pub fn is_identity(lut: &[u8; 256]) -> bool { lut.iter().enumerate().all(|(i, &v)| v as usize == i) }

pub fn apply_lut(img: &mut Img, lut: &[u8; 256]) {
    par_rows(img, |b, _| for y in 0..b.h {
        for p in b.row(y).chunks_exact_mut(4) { p[0] = lut[p[0] as usize]; p[1] = lut[p[1] as usize]; p[2] = lut[p[2] as usize]; }
    });
}

/// Average luma 0..1 from a sparse grid.
pub fn mean_luma(img: &Img) -> f32 {
    let (mut acc, mut n) = (0u64, 0u64);
    for gy in 0..24 { for gx in 0..32 {
        let (x, y) = (gx * img.w / 32, gy * img.h / 24);
        let i = y * img.stride + x * 4;
        acc += luma(img.px[i], img.px[i + 1], img.px[i + 2]) as u64;
        n += 1;
    } }
    acc as f32 / n as f32 / 255.0
}

#[inline]
pub fn luma(r: u8, g: u8, b: u8) -> u32 { (r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8 }

/// The low-light gain for a picture of average luma `mean`: moves it toward 0.42, never darkens, at most 3x.
pub fn low_light_gain(mean: f32) -> f32 {
    let m = mean.clamp(0.02, 0.98);
    // the soft knee maps m to m*g/(1+(g-1)m); solved for 0.42
    (0.42 * (1.0 - m) / (m * 0.58)).clamp(1.0, 3.0)
}

/// Flips the picture left to right.
pub fn mirror(img: &mut Img) {
    par_rows(img, |b, _| {
        let mut tmp = vec![0u8; b.w * 4];
        for y in 0..b.h {
            let r = b.row(y);
            for (o, p) in tmp.chunks_exact_mut(4).zip(r.chunks_exact(4).rev()) { o.copy_from_slice(p); }
            r.copy_from_slice(&tmp);
        }
    });
}

/// A smaller RGB copy (`k` x `k` pixel boxes averaged), for blurs.
pub struct Small { pub w: usize, pub h: usize, pub px: Vec<u8> }

impl Small {
    /// `img` shrunk by `k` (at least 1 x 1).
    pub fn of(img: &Img, k: usize) -> Small {
        let k = k.min(img.w).min(img.h).max(1);
        let (w, h) = (img.w / k, img.h / k);
        let mut px = vec![0u8; w * h * 3];
        let n = (k * k) as u32;
        let mut acc = vec![0u32; w * 3];
        for (sy, out) in px.chunks_exact_mut(w * 3).enumerate() {
            acc.iter_mut().for_each(|a| *a = 0);
            // whole k x k boxes only (a ragged right / bottom edge of fewer than k pixels is left out)
            for y in sy * k..sy * k + k {
                let row = &img.px[y * img.stride..y * img.stride + w * k * 4];
                for (a, blk) in acc.chunks_exact_mut(3).zip(row.chunks_exact(k * 4)) {
                    for p in blk.chunks_exact(4) { a[0] += p[0] as u32; a[1] += p[1] as u32; a[2] += p[2] as u32; }
                }
            }
            for (o, a) in out.iter_mut().zip(&acc) { *o = (a / n) as u8; }
        }
        Small { w, h, px }
    }
    /// Box blur of radius `r`, `passes` times (three passes come close to a Gaussian).
    pub fn blur(&mut self, r: usize, passes: usize) {
        if r == 0 { return; }
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0u8; self.px.len()];
        for _ in 0..passes {
            box_rows(&self.px, &mut tmp, w, r);
            box_cols(&tmp, &mut self.px, w * 3, h, r);
        }
    }
    /// Bilinear sampling positions for a frame `fw` x `fh` scaled onto this one: (index, next index, weight 0..256) per column / row.
    pub fn grid(&self, fw: usize, fh: usize) -> (Vec<(usize, usize, u32)>, Vec<(usize, usize, u32)>) { (axis(fw, self.w), axis(fh, self.h)) }
}

/// Running-sum box blur of radius `r` along each row of an RGB image `w` wide (edges repeat the outermost pixel).
fn box_rows(src: &[u8], dst: &mut [u8], w: usize, r: usize) {
    let d = (2 * r + 1) as u32;
    for (s, o) in src.chunks_exact(w * 3).zip(dst.chunks_exact_mut(w * 3)) {
        let at = |i: usize, c: usize| s[i.min(w - 1) * 3 + c] as u32;
        let mut sum = [0u32; 3];
        for c in 0..3 { sum[c] = (0..=r).map(|i| at(i, c)).sum::<u32>() + at(0, c) * r as u32; }
        for i in 0..w {
            for c in 0..3 { o[i * 3 + c] = (sum[c] / d) as u8; sum[c] = sum[c] + at(i + r + 1, c) - at(i.saturating_sub(r), c); }
        }
    }
}
/// The same down the columns, one whole row (`n` values) at a time, so memory is read in order.
fn box_cols(src: &[u8], dst: &mut [u8], n: usize, h: usize, r: usize) {
    let d = (2 * r + 1) as u32;
    let row = |y: usize| &src[y.min(h - 1) * n..y.min(h - 1) * n + n];
    let mut sum = vec![0u32; n];
    for y in 0..=r { for (s, &v) in sum.iter_mut().zip(row(y)) { *s += v as u32; } }
    for (s, &v) in sum.iter_mut().zip(row(0)) { *s += v as u32 * r as u32; }
    for (y, o) in dst.chunks_exact_mut(n).enumerate() {
        for (o, s) in o.iter_mut().zip(&sum) { *o = (s / d) as u8; }
        let (add, sub) = (row(y + r + 1), row(y.saturating_sub(r)));
        for ((s, &a), &b) in sum.iter_mut().zip(add).zip(sub) { *s = *s + a as u32 - b as u32; }
    }
}

/// Bilinear positions mapping `big` samples onto `small` ones.
pub fn axis(big: usize, small: usize) -> Vec<(usize, usize, u32)> {
    let s = small as f32 / big as f32;
    (0..big).map(|x| {
        let f = ((x as f32 + 0.5) * s - 0.5).clamp(0.0, (small - 1) as f32);
        let i = f as usize;
        (i, (i + 1).min(small - 1), ((f - i as f32) * 256.0) as u32)
    }).collect()
}


/// The soft-focus face filter: smooths skin and flat areas, keeps edges (eyes, hair, outlines). `amount` 0..1; `weight`
/// (person mask, 0..255 per pixel of a `mw` x `mh` mask) limits it to the person when there is one.
pub fn soften(img: &mut Img, amount: f32, mask: Option<(&[u8], usize, usize)>) {
    let a = (amount.clamp(0.0, 1.0) * 256.0) as u32;
    if a == 0 || img.w < 8 || img.h < 8 { return; }
    // quarter size, a radius of ~0.5 % of the height: the scale of skin texture, not of features
    let mut s = Small::of(img, 4);
    s.blur((img.h / 360).max(1), 2);
    let (xs, ys) = s.grid(img.w, img.h);
    let (mx, my) = mask.map_or((vec![], vec![]), |(_, mw, mh)| (axis(img.w, mw), axis(img.h, mh)));
    par_rows(img, |band, y0| {
        let (mut line, mut mline) = (vec![0u32; s.w * 3], vec![0u32; mask.map_or(0, |m| m.1)]);
        let (mut brow, mut mrow) = (vec![0u8; band.w * 3], vec![255u8; band.w]);
        for y in 0..band.h {
            vrow(&s.px, s.w * 3, ys[y0 + y], &mut line);
            hline::<3>(&line, &xs, &mut brow);
            if let Some((m, mw, _)) = mask { vrow(m, mw, my[y0 + y], &mut mline); hline::<1>(&mline, &mx, &mut mrow); }
            for ((p, b), &m) in band.row(y).chunks_exact_mut(4).zip(brow.chunks_exact(3)).zip(&mrow) {
                let k = a * m as u32 / 255;
                // edge-preserving: the more the pixel differs from its surroundings, the less it is smoothed
                let diff = (luma(p[0], p[1], p[2]) as i32 - luma(b[0], b[1], b[2]) as i32).unsigned_abs();
                let k = k * 32u32.saturating_sub(diff) >> 5;
                for c in 0..3 { p[c] = ((p[c] as u32 * (256 - k) + b[c] as u32 * k) >> 8) as u8; }
            }
        }
    });
}

/// One output row's worth of a small image, already interpolated vertically (values x 256): `n` = samples per small row.
#[inline]
fn vrow(px: &[u8], n: usize, (y0, y1, fy): (usize, usize, u32), out: &mut [u32]) {
    let (a, b) = (&px[y0 * n..(y0 + 1) * n], &px[y1 * n..(y1 + 1) * n]);
    for ((o, &a), &b) in out.iter_mut().zip(a).zip(b) { *o = a as u32 * (256 - fy) + b as u32 * fy; }
}
/// The horizontal half of the bilinear scaling: a `vrow` line of `CH` channels to one full-width output row.
#[inline]
fn hline<const CH: usize>(line: &[u32], xs: &[(usize, usize, u32)], out: &mut [u8]) {
    for (o, &(x0, x1, fx)) in out.chunks_exact_mut(CH).zip(xs) {
        let (a, b) = (&line[x0 * CH..x0 * CH + CH], &line[x1 * CH..x1 * CH + CH]);
        for c in 0..CH { o[c] = ((a[c] * (256 - fx) + b[c] * fx) >> 16) as u8; }
    }
}

/// What goes behind the person.
pub enum Behind<'a> {
    /// The frame itself, blurred: shrunk by 8, blurred, sampled back.
    Blur(Small),
    /// A picture of the frame's size (RGBA, tightly packed).
    Picture(&'a [u8]),
}

/// Lays the person (mask 0..255, `mw` x `mh`, 255 = person) over `behind`.
pub fn composite(img: &mut Img, mask: &[u8], mw: usize, mh: usize, behind: &Behind) {
    let (mx, my) = (axis(img.w, mw), axis(img.h, mh));
    let grid = if let Behind::Blur(s) = behind { Some(s.grid(img.w, img.h)) } else { None };
    let w = img.w;
    par_rows(img, |band, y0| {
        let mut mline = vec![0u32; mw];
        let mut line = vec![0u32; if let Behind::Blur(s) = behind { s.w * 3 } else { 0 }];
        let (mut mrow, mut brow) = (vec![0u8; w], vec![0u8; w * 4]);
        for y in 0..band.h {
            let fy = y0 + y;
            vrow(mask, mw, my[fy], &mut mline);
            hline::<1>(&mline, &mx, &mut mrow);
            // the background row as RGBA, either scaled up from the blurred copy or straight from the picture
            let bg: &[u8] = match (behind, &grid) {
                (Behind::Blur(s), Some((xs, ys))) => {
                    vrow(&s.px, s.w * 3, ys[fy], &mut line);
                    for (o, &(x0, x1, fx)) in brow.chunks_exact_mut(4).zip(xs.iter()) {
                        for c in 0..3 { o[c] = ((line[x0 * 3 + c] * (256 - fx) + line[x1 * 3 + c] * fx) >> 16) as u8; }
                    }
                    &brow
                }
                (Behind::Picture(b), _) => &b[fy * w * 4..(fy + 1) * w * 4],
                _ => return,
            };
            for ((p, b), &a) in band.row(y).chunks_exact_mut(4).zip(bg.chunks_exact(4)).zip(&mrow) {
                if a == 255 { continue; }
                let (a, na) = (a as u32, 255 - a as u32);
                for c in 0..3 { p[c] = ((p[c] as u32 * a + b[c] as u32 * na + 127) / 255) as u8; }
            }
        }
    });
}

/// The blurred background of `img`: shrunk by 8, then blurred with a radius of `strength` (1..=8) small pixels.
pub fn blurred(img: &Img, strength: usize) -> Small {
    let mut s = Small::of(img, 8);
    s.blur(strength.clamp(1, 8), 2);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> (Vec<u8>, usize) {
        let stride = w * 4 + 8; // padding, as a host may hand over
        let mut v = vec![0u8; stride * h];
        for y in 0..h { for x in 0..w { let c = f(x, y); v[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]); } }
        (v, stride)
    }
    fn at(v: &[u8], stride: usize, x: usize, y: usize) -> [u8; 3] { let i = y * stride + x * 4; [v[i], v[i + 1], v[i + 2]] }

    #[test]
    fn tone_curve() {
        assert!(is_identity(&tone_lut(0.0, 0.0, 1.0)));
        let bright = tone_lut(50.0, 0.0, 1.0);
        assert!(bright[100] > 100 && bright[0] > 0);
        let contrast = tone_lut(0.0, 60.0, 1.0);
        assert!(contrast[60] < 60 && contrast[200] > 200 && contrast[128].abs_diff(128) <= 2);
        let flat = tone_lut(0.0, -100.0, 1.0);
        assert!(flat[0] > 50 && flat[255] < 205);
        // gain lifts the shadows a lot, the highlights barely, never past white
        let g = tone_lut(0.0, 0.0, 2.5);
        assert!(g[30] > 60 && g[250] >= 250 && g[255] == 255);
        // monotonic
        assert!(g.windows(2).all(|w| w[0] <= w[1]) && contrast.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn low_light_gain_only_brightens_dark_pictures() {
        assert!(low_light_gain(0.1) > 2.0);
        assert!((low_light_gain(0.42) - 1.0).abs() < 1e-4);
        assert_eq!(low_light_gain(0.8), 1.0);
        assert_eq!(low_light_gain(0.0), 3.0);
        let (mut v, stride) = frame(32, 18, |_, _| [20, 20, 20]);
        let mut img = Img { w: 32, h: 18, stride, px: &mut v };
        let m = mean_luma(&img);
        assert!((m - 20.0 / 255.0).abs() < 0.01);
        apply_lut(&mut img, &tone_lut(0.0, 0.0, low_light_gain(m)));
        // at most 3x: a very dark room gets clearly brighter without turning into noise
        assert!(mean_luma(&img) > 0.18, "{}", mean_luma(&img));
    }

    #[test]
    fn mirror_flips_and_keeps_padding() {
        let (mut v, stride) = frame(5, 2, |x, y| [x as u8, y as u8, 0]);
        v[5 * 4] = 99; // padding byte after the first row
        mirror(&mut Img { w: 5, h: 2, stride, px: &mut v });
        assert_eq!(at(&v, stride, 0, 0), [4, 0, 0]);
        assert_eq!(at(&v, stride, 4, 1), [0, 1, 0]);
        assert_eq!(at(&v, stride, 2, 0), [2, 0, 0]);
        assert_eq!(v[5 * 4], 99);
    }

    /// Frames high enough to be split into bands: every row is done, once, with its own row number.
    #[test]
    fn bands_cover_every_row_once() {
        let (mut v, stride) = frame(8, 131, |x, y| [x as u8, y as u8, 0]);
        par_rows(&mut Img { w: 8, h: 131, stride, px: &mut v }, |b, y0| for y in 0..b.h { for p in b.row(y).chunks_exact_mut(4) { assert_eq!(p[1] as usize, y0 + y); p[2] += 1; } });
        assert!((0..131).all(|y| (0..8).all(|x| at(&v, stride, x, y)[2] == 1)));
        mirror(&mut Img { w: 8, h: 131, stride, px: &mut v });
        assert_eq!(at(&v, stride, 0, 130), [7, 130, 1]);
    }

    #[test]
    fn blur_and_shrink() {
        let (v, stride) = frame(16, 8, |x, _| if x < 8 { [0, 0, 0] } else { [240, 240, 240] });
        let mut px = v.clone();
        let img = Img { w: 16, h: 8, stride, px: &mut px };
        let mut s = Small::of(&img, 2);
        assert_eq!((s.w, s.h), (8, 4));
        assert_eq!(&s.px[..3], &[0, 0, 0]);
        assert_eq!(&s.px[7 * 3..7 * 3 + 3], &[240, 240, 240]);
        s.blur(1, 1);
        // the hard edge got soft, the far ends stay
        assert!(s.px[3 * 3] > 0 && s.px[4 * 3] < 240);
        assert_eq!(s.px[0], 0);
    }

    #[test]
    fn soften_smooths_noise_and_keeps_edges() {
        // left half: grey with fine noise (skin), right half: white; a hard edge between them
        let (mut v, stride) = frame(64, 32, |x, y| if x < 32 { let n = if (x + y) % 2 == 0 { 110 } else { 130 }; [n, n, n] } else { [250, 250, 250] });
        soften(&mut Img { w: 64, h: 32, stride, px: &mut v }, 1.0, None);
        let (a, b) = (at(&v, stride, 10, 10)[0], at(&v, stride, 11, 10)[0]);
        assert!(a.abs_diff(b) < 10, "noise smoothed: {a} {b}");
        assert!(at(&v, stride, 33, 10)[0] > 235 && at(&v, stride, 30, 10)[0] < 140, "edge kept");
        // with a mask of zeros (no person): nothing happens
        let (mut v2, _) = frame(64, 32, |x, y| if (x + y) % 2 == 0 { [110; 3] } else { [130; 3] });
        let before = v2.clone();
        soften(&mut Img { w: 64, h: 32, stride, px: &mut v2 }, 1.0, Some((&[0u8; 16], 4, 4)));
        assert_eq!(v2, before);
    }

    #[test]
    fn composite_puts_the_person_over_the_background() {
        // mask: left half person, right half background
        let mask: Vec<u8> = (0..8 * 4).map(|i| if i % 8 < 4 { 255 } else { 0 }).collect();
        let (mut v, stride) = frame(16, 8, |_, _| [200, 10, 10]);
        let pic: Vec<u8> = [10u8, 200, 10, 255].repeat(16 * 8);
        composite(&mut Img { w: 16, h: 8, stride, px: &mut v }, &mask, 8, 4, &Behind::Picture(&pic));
        assert_eq!(at(&v, stride, 1, 3), [200, 10, 10]);
        assert_eq!(at(&v, stride, 14, 3), [10, 200, 10]);
        // in between: a blend
        let mid = at(&v, stride, 8, 3);
        assert!(mid[0] > 10 && mid[0] < 200, "{mid:?}");
        // blur behind: the background side becomes the blurred frame (here the same flat colour)
        let (mut v, stride) = frame(64, 32, |x, _| if x < 32 { [255, 0, 0] } else { [0, 0, 255] });
        let img = Img { w: 64, h: 32, stride, px: &mut v };
        let bg = blurred(&img, 2);
        let mut px = v.clone();
        composite(&mut Img { w: 64, h: 32, stride, px: &mut px }, &[0u8; 4], 2, 2, &Behind::Blur(bg));
        let edge = at(&px, stride, 31, 16);
        assert!(edge[0] < 250 && edge[2] > 5, "blurred across the edge: {edge:?}");
        assert!(at(&px, stride, 0, 16)[0] > 200, "far from the edge it stays red");
    }
}
