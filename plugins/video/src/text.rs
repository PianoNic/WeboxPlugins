//! "Show name": a lower third (a dark translucent bar with an accent stripe and the name in white, bottom left). The
//! text is drawn with the bundled Selawik Semibold (SIL Open Font License 1.1, the font Webox itself ships) once per name
//! and frame height into an alpha mask; each frame only blends the bar and the mask.
use crate::fx::Img;
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

static FONT: &[u8] = include_bytes!("../assets/selawksb.ttf");

/// A name drawn at one size.
pub struct Label { text: String, frame_h: usize, w: usize, h: usize, alpha: Vec<u8> }

impl Label {
    /// The name in white at a height fitting a frame `frame_h` high (cut to fit `max_w`).
    pub fn new(text: &str, frame_h: usize, max_w: usize) -> Option<Label> {
        let font = FontRef::try_from_slice(FONT).ok()?;
        let px = (frame_h as f32 / 20.0).max(10.0);
        let sf = font.as_scaled(PxScale::from(px));
        let (asc, desc) = (sf.ascent(), sf.descent());
        let h = (asc - desc).ceil() as usize + 2;
        let mut x = 0.0f32;
        let mut glyphs = vec![];
        let mut prev = None;
        for c in text.chars().filter(|c| !c.is_control()) {
            let id = sf.glyph_id(c);
            if let Some(p) = prev { x += sf.kern(p, id); }
            let next = x + sf.h_advance(id);
            if next > max_w as f32 { break; }
            glyphs.push(id.with_scale_and_position(px, ab_glyph::point(x, asc)));
            x = next;
            prev = Some(id);
        }
        let w = x.ceil() as usize + 2;
        if glyphs.is_empty() { return None; }
        let mut alpha = vec![0u8; w * h];
        for g in glyphs {
            let Some(o) = font.outline_glyph(g) else { continue };
            let b = o.px_bounds();
            o.draw(|gx, gy, c| {
                let (x, y) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    let a = &mut alpha[y as usize * w + x as usize];
                    *a = (*a).max((c.clamp(0.0, 1.0) * 255.0) as u8);
                }
            });
        }
        Some(Label { text: text.to_string(), frame_h, w, h, alpha })
    }
    /// Whether this label is the one for `text` on a frame `frame_h` high.
    pub fn fits(&self, text: &str, frame_h: usize) -> bool { self.text == text && self.frame_h == frame_h }

    /// Draws the lower third onto `img`.
    pub fn draw(&self, img: &mut Img) {
        let pad = self.h / 2;
        let margin = img.h / 18;
        let stripe = (self.h / 8).max(2);
        let (bw, bh) = (self.w + pad * 2 + stripe, self.h + pad);
        let (x0, y0) = (margin.min(img.w.saturating_sub(bw)), img.h.saturating_sub(margin + bh));
        for y in y0..(y0 + bh).min(img.h) {
            let row = &mut img.px[y * img.stride..y * img.stride + img.w * 4];
            for x in x0..(x0 + bw).min(img.w) {
                let p = &mut row[x * 4..x * 4 + 3];
                let (lx, ly) = (x - x0, y - y0);
                if lx < stripe {
                    // the accent stripe (Webex blue)
                    p.copy_from_slice(&[7, 123, 228]);
                    continue;
                }
                // the bar: 65 % black
                for c in p.iter_mut() { *c = (*c as u32 * 90 / 256) as u8; }
                let (tx, ty) = (lx as isize - (stripe + pad) as isize, ly as isize - (pad / 2) as isize);
                if tx >= 0 && ty >= 0 && (tx as usize) < self.w && (ty as usize) < self.h {
                    let a = self.alpha[ty as usize * self.w + tx as usize] as u32;
                    for c in p.iter_mut() { *c = ((*c as u32 * (255 - a) + 255 * a) / 255) as u8; }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_drawn_bottom_left_in_white() {
        let (w, h) = (320usize, 180usize);
        let mut px = vec![128u8; w * h * 4];
        let l = Label::new("Anna Muster", h, w).expect("label");
        assert!(l.fits("Anna Muster", h) && !l.fits("Anna", h) && !l.fits("Anna Muster", 720));
        assert!(l.alpha.iter().any(|&a| a == 255), "solid glyph pixels");
        l.draw(&mut Img { w, h, stride: w * 4, px: &mut px });
        let lum = |x: usize, y: usize| px[(y * w + x) * 4] as u32;
        // the top half stays untouched, the bar darkens, the text is white
        assert_eq!(lum(w / 2, 10), 128);
        let bottom: Vec<u32> = (h / 2..h).flat_map(|y| (0..w / 2).map(move |x| (x, y))).map(|(x, y)| lum(x, y)).collect();
        assert!(bottom.iter().any(|&v| v < 60) && bottom.iter().any(|&v| v > 240));
        // nothing to draw: no label
        assert!(Label::new("", h, w).is_none());
        // a long name is cut to the frame
        let long = Label::new(&"W".repeat(200), h, w).unwrap();
        assert!(long.w <= w + 2);
    }
}
