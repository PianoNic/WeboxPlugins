//! By hand: the cost of each effect on a 1280 x 720 frame, and a before / after picture of each.
//!
//! `cargo test --release -p collabhost_video bench -- --ignored --nocapture`
//!
//! `VIDEO_BENCH_PHOTO=<png>` uses that picture (scaled to 1280 x 720) instead of the synthetic one, `VIDEO_BENCH_BG=<png>` a
//! background picture, `VIDEO_BENCH_OUT=<folder>` writes `before.png` and one PNG per effect there.
use crate::fx::{self, Behind, Img};
use crate::{seg, text};
use std::time::Instant;

const W: usize = 1280;
const H: usize = 720;

fn load(path: &str) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    // cover-scale with nearest samples: good enough for a test picture
    let s = (W as f32 / w as f32).max(H as f32 / h as f32);
    let (ox, oy) = ((w as f32 - W as f32 / s) / 2.0, (h as f32 - H as f32 / s) / 2.0);
    let mut out = vec![255u8; W * H * 4];
    for y in 0..H { for x in 0..W {
        let (sx, sy) = (((x as f32 + 0.5) / s + ox) as usize, ((y as f32 + 0.5) / s + oy) as usize);
        let p = img.get_pixel(sx.min(w - 1) as u32, sy.min(h - 1) as u32).0;
        out[(y * W + x) * 4..(y * W + x) * 4 + 4].copy_from_slice(&p);
    } }
    Some(out)
}

/// A camera-like test picture: a lit "head and shoulders" shape on a busy background, with fine noise.
fn synthetic() -> Vec<u8> {
    let mut v = vec![255u8; W * H * 4];
    for y in 0..H { for x in 0..W {
        let (fx, fy) = (x as f32 / W as f32, y as f32 / H as f32);
        let head = ((fx - 0.5) / 0.11).powi(2) + ((fy - 0.38) / 0.2).powi(2) < 1.0;
        let body = fy > 0.6 && ((fx - 0.5) / 0.3).powi(2) + ((fy - 1.05) / 0.45).powi(2) < 1.0;
        let n = ((x * 7919 + y * 104_729) % 23) as u8;
        let c = if head { [196 + n / 2, 150 + n / 2, 120 + n / 2] } else if body { [40 + n, 70 + n, 140 + n] }
            else if (x / 80 + y / 80) % 2 == 0 { [90 + n, 110 + n, 90 + n] } else { [160 + n, 150 + n, 130 + n] };
        v[(y * W + x) * 4..(y * W + x) * 4 + 3].copy_from_slice(&c);
    } }
    v
}

fn save(dir: &Option<String>, name: &str, px: &[u8]) {
    if let Some(d) = dir { let _ = image::save_buffer(format!("{d}/{name}.png"), px, W as u32, H as u32, image::ExtendedColorType::Rgba8); }
}

/// Median time of `n` runs of `f` on a fresh copy of `src`, in ms; leaves the last result in `out`.
fn time(src: &[u8], out: &mut Vec<u8>, n: usize, mut f: impl FnMut(&mut Img)) -> f64 {
    let mut t = vec![];
    for _ in 0..n {
        out.clear();
        out.extend_from_slice(src);
        let s = Instant::now();
        f(&mut Img { w: W, h: H, stride: W * 4, px: out });
        t.push(s.elapsed().as_secs_f64() * 1000.0);
    }
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    t[n / 2]
}

#[test]
#[ignore]
fn bench() {
    let dir = std::env::var("VIDEO_BENCH_OUT").ok();
    let src = std::env::var("VIDEO_BENCH_PHOTO").ok().and_then(|p| load(&p)).unwrap_or_else(synthetic);
    let bg = std::env::var("VIDEO_BENCH_BG").ok().and_then(|p| load(&p))
        .unwrap_or_else(|| (0..W * H).flat_map(|i| { let (x, y) = (i % W, i / W); [(x * 255 / W) as u8, 120, (y * 255 / H) as u8, 255] }).collect());
    save(&dir, "before", &src);
    let mut out = vec![];
    let n = 30;
    let mut rows: Vec<(&str, f64)> = vec![];

    rows.push(("Brightness/contrast (+30/+20)", time(&src, &mut out, n, |i| fx::apply_lut(i, &fx::tone_lut(30.0, 20.0, 1.0)))));
    save(&dir, "brightness_contrast", &out);
    rows.push(("Low-light boost", time(&src, &mut out, n, |i| { let g = fx::low_light_gain(fx::mean_luma(i) * 0.4); fx::apply_lut(i, &fx::tone_lut(0.0, 0.0, g)) })));
    save(&dir, "low_light", &out);
    rows.push(("Mirror", time(&src, &mut out, n, fx::mirror)));
    save(&dir, "mirror", &out);
    rows.push(("Soft focus for the face (100)", time(&src, &mut out, n, |i| fx::soften(i, 1.0, None))));
    save(&dir, "soften", &out);
    rows.push(("  of which: quarter size", time(&src, &mut out, n, |i| { std::hint::black_box(fx::Small::of(i, 4)); })));
    rows.push(("  of which: quarter size + blur", time(&src, &mut out, n, |i| { let mut s = fx::Small::of(i, 4); s.blur(2, 2); std::hint::black_box(s); })));
    let label = text::Label::new("Anna Smith", H, W * 2 / 3).unwrap();
    rows.push(("Show name", time(&src, &mut out, n, |i| label.draw(i))));
    save(&dir, "name", &out);

    let t = Instant::now();
    let mut s = seg::Segmenter::new().unwrap();
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let mut mask = vec![];
    rows.push(("Segmentation (256x144, tract; own thread)", time(&src, &mut out, n, |i| { mask = s.run(i.px, i.w, i.h, i.stride).unwrap(); })));
    let mut input = vec![];
    rows.push(("Segmentation: input for the thread", time(&src, &mut out, n, |i| seg::sample(i.px, i.w, i.h, i.stride, &mut input))));
    let mut sm = seg::Smoother::new(0.6);
    for _ in 0..5 { sm.update(&mask); }
    let alpha = sm.alpha.clone();
    rows.push(("Mask smoothing", time(&src, &mut out, n, |_| { sm.update(&mask); })));
    rows.push(("Background blur (without segm.)", time(&src, &mut out, n, |i| { let b = fx::blurred(i, 5); fx::composite(i, &alpha, seg::W, seg::H, &Behind::Blur(b)) })));
    save(&dir, "background_blur", &out);
    rows.push(("Picture background (without segm.)", time(&src, &mut out, n, |i| fx::composite(i, &alpha, seg::W, seg::H, &Behind::Picture(&bg)))));
    save(&dir, "background_picture", &out);
    // all at once, as the filter runs them: the inference runs on its own thread, the filter only samples and takes the mask
    rows.push(("Everything in the filter (picture background)", time(&src, &mut out, n, |i| {
        fx::apply_lut(i, &fx::tone_lut(10.0, 10.0, 1.2));
        seg::sample(i.px, i.w, i.h, i.stride, &mut input);
        let m = mask.clone();
        sm.update(&m);
        fx::composite(i, &sm.alpha, seg::W, seg::H, &Behind::Picture(&bg));
        fx::soften(i, 0.5, Some((&sm.alpha, seg::W, seg::H)));
        fx::mirror(i);
        label.draw(i);
    })));
    save(&dir, "all", &out);

    println!("1280x720, median of {n} runs, release build:");
    println!("  Model load (once): {load_ms:.1} ms");
    for (name, ms) in rows { println!("  {name:<42} {ms:6.2} ms"); }
}
