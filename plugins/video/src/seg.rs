//! Person segmentation for the background effects: MediaPipe Selfie Segmentation, landscape variant (Google, Apache-2.0; the
//! ONNX conversion from huggingface.co/onnx-community/mediapipe_selfie_segmentation_landscape, `onnx/model.onnx`), run with
//! tract on the CPU at 256 x 144. The model ships inside the plugin (450 KB); nothing is downloaded and no picture leaves the
//! machine. The raw mask flickers a little from frame to frame, so `Smoother` blends each new one into the last
//! (exponential moving average) and turns it into an 8-bit alpha with a soft edge.
use tract_onnx::prelude::*;

/// The model's input size (16:9, the same as a camera frame).
pub const W: usize = 256;
pub const H: usize = 144;
static MODEL: &[u8] = include_bytes!("../assets/selfie_segmentation_landscape.onnx");

pub struct Segmenter { plan: TypedRunnableModel<TypedModel>, input: Vec<f32> }

impl Segmenter {
    /// Loads and optimises the model (done once, when a background effect is first switched on).
    pub fn new() -> TractResult<Segmenter> {
        let mut model = tract_onnx::onnx().model_for_read(&mut std::io::Cursor::new(MODEL))?;
        // The file declares every intermediate shape with an open batch size, which then cannot meet the fixed one below:
        // forget them (tract works them out again) and feed one picture at a time.
        for id in 0..model.nodes().len() {
            if model.input_outlets()?.iter().any(|o| o.node == id) { continue; }
            for slot in 0..model.nodes()[id].outputs.len() { model.set_outlet_fact(OutletId::new(id, slot), InferenceFact::default())?; }
        }
        let plan = model.with_input_fact(0, f32::fact([1, 3, H, W]).into())?.into_optimized()?.into_runnable()?;
        Ok(Segmenter { plan, input: vec![0.0; 3 * H * W] })
    }

    /// The person probability (0..1) per model pixel for an RGBA frame (stretched to 256 x 144), row by row.
    pub fn run(&mut self, px: &[u8], w: usize, h: usize, stride: usize) -> TractResult<Vec<f32>> {
        sample(px, w, h, stride, &mut self.input);
        let input = std::mem::take(&mut self.input);
        let r = self.infer(&input);
        self.input = input;
        r
    }
    /// The mask for a model input made by `sample`.
    pub fn infer(&mut self, input: &[f32]) -> TractResult<Vec<f32>> {
        let t = Tensor::from_shape(&[1, 3, H, W], input)?;
        let out = self.plan.run(tvec!(t.into()))?;
        Ok(out[0].as_slice::<f32>()?.to_vec())
    }
}

/// The model input for an RGBA frame: 256 x 144 samples, planes R, G, B, 0..1.
pub fn sample(px: &[u8], w: usize, h: usize, stride: usize, input: &mut Vec<f32>) {
    let plane = H * W;
    input.resize(3 * plane, 0.0);
    let (xs, ys) = (pick(w, W), pick(h, H));
    for (my, &sy) in ys.iter().enumerate() {
        let row = &px[sy * stride..];
        for (mx, &sx) in xs.iter().enumerate() {
            let i = sx * 4;
            let o = my * W + mx;
            input[o] = row[i] as f32 / 255.0;
            input[plane + o] = row[i + 1] as f32 / 255.0;
            input[2 * plane + o] = row[i + 2] as f32 / 255.0;
        }
    }
}

/// Segmentation on a thread of its own, so the frame filter never waits for the model: each frame hands over a model input
/// (when the thread is free) and takes the newest finished mask. The mask is therefore a frame or two behind, which the
/// temporal smoothing hides. Dropping it ends and joins the thread (a plugin keeps no thread after `stop`).
pub struct Worker {
    tx: Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
    rx: std::sync::mpsc::Receiver<Result<Vec<f32>, String>>,
    thread: Option<std::thread::JoinHandle<()>>,
    busy: bool,
    /// Why the model cannot run (it failed to load).
    pub failed: Option<String>,
    input: Vec<f32>,
    /// Inference time of the last mask, ms.
    pub last_ms: f32,
}

impl Worker {
    pub fn start() -> Worker {
        let (tx, jobs) = std::sync::mpsc::sync_channel::<Vec<f32>>(1);
        let (done, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("video-segmentation".into()).spawn(move || {
            let mut s = match Segmenter::new() { Ok(s) => s, Err(e) => { let _ = done.send(Err(format!("Modell lädt nicht: {e}"))); return; } };
            while let Ok(input) = jobs.recv() {
                let t = std::time::Instant::now();
                let r = s.infer(&input).map_err(|e| e.to_string());
                let _ = done.send(r.map(|mut m| { m.push(t.elapsed().as_secs_f32() * 1000.0); m }));
            }
        }).ok();
        Worker { tx: Some(tx), rx, thread, busy: false, failed: None, input: vec![], last_ms: 0.0 }
    }
    /// The newest finished mask, if one came since the last call; hands the frame over when the thread is free.
    pub fn step(&mut self, px: &[u8], w: usize, h: usize, stride: usize) -> Option<Vec<f32>> {
        let mut newest = None;
        while let Ok(r) = self.rx.try_recv() {
            self.busy = false;
            match r { Ok(mut m) => { self.last_ms = m.pop().unwrap_or(0.0); newest = Some(m) } Err(e) => self.failed = Some(e) }
        }
        if !self.busy && self.failed.is_none() {
            sample(px, w, h, stride, &mut self.input);
            if self.tx.as_ref().is_some_and(|tx| tx.try_send(self.input.clone()).is_ok()) { self.busy = true; }
        }
        newest
    }
    /// Blocks up to `wait` for a mask (the very first one, so the first frames do not show the room).
    pub fn wait(&mut self, wait: std::time::Duration) -> Option<Vec<f32>> {
        match self.rx.recv_timeout(wait) {
            Ok(Ok(mut m)) => { self.busy = false; self.last_ms = m.pop().unwrap_or(0.0); Some(m) }
            Ok(Err(e)) => { self.busy = false; self.failed = Some(e); None }
            Err(_) => None,
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx = None; // ends the thread's loop after the running inference
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }
}

/// Nearest source positions for `n` samples across `len` pixels (the model input needs no more than that).
fn pick(len: usize, n: usize) -> Vec<usize> { (0..n).map(|i| ((i * 2 + 1) * len / (2 * n)).min(len - 1)).collect() }

/// Temporal smoothing of the mask and its conversion to alpha.
pub struct Smoother {
    /// How much of a new mask goes in (0..1): lower = steadier but slower to follow a movement.
    pub rate: f32,
    avg: Vec<f32>,
    /// The alpha (0 = background, 255 = person), `W` x `H`.
    pub alpha: Vec<u8>,
}

impl Smoother {
    pub fn new(rate: f32) -> Smoother { Smoother { rate, avg: vec![], alpha: vec![] } }
    /// Blends `mask` in and updates `alpha`.
    pub fn update(&mut self, mask: &[f32]) -> &[u8] {
        if self.avg.len() != mask.len() { self.avg = mask.to_vec(); } else {
            for (a, m) in self.avg.iter_mut().zip(mask) { *a += (m - *a) * self.rate; }
        }
        // a soft but narrow edge: below 0.3 background, above 0.7 person, a smooth step between
        self.alpha = self.avg.iter().map(|&p| { let t = ((p - 0.3) / 0.4).clamp(0.0, 1.0); ((t * t * (3.0 - 2.0 * t)) * 255.0 + 0.5) as u8 }).collect();
        &self.alpha
    }
    pub fn reset(&mut self) { self.avg.clear(); self.alpha.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothing_follows_steady_masks_and_damps_flicker() {
        let mut s = Smoother::new(0.5);
        let person = vec![1.0f32; 4];
        let none = vec![0.0f32; 4];
        assert_eq!(s.update(&person), &[255; 4]);
        // one frame of a wrong mask moves it only half way
        let a = s.update(&none)[0];
        assert!(a > 100 && a < 200, "{a}");
        // a steady new mask is reached after a few frames
        for _ in 0..8 { s.update(&none); }
        assert_eq!(s.alpha, vec![0; 4]);
        // thresholds: 0.2 is background, 0.8 person, 0.5 in between
        s.reset();
        let a = s.update(&[0.2, 0.5, 0.8, 0.31]).to_vec();
        assert!(a[0] == 0 && (127..=128).contains(&a[1]) && a[2] == 255 && a[3] == 0, "{a:?}");
        // a mask of another size starts over
        assert_eq!(s.update(&[1.0; 6]).len(), 6);
    }

    #[test]
    fn picks_spread_over_the_frame() {
        assert_eq!(pick(8, 4), vec![1, 3, 5, 7]);
        assert_eq!(pick(1280, 256)[0], 2);
        assert_eq!(*pick(1280, 256).last().unwrap(), 1277);
    }

    /// The model runs and finds no person in a flat grey picture.
    #[test]
    fn model_runs() {
        let mut s = Segmenter::new().expect("model loads");
        let px = vec![128u8; 320 * 180 * 4];
        let m = s.run(&px, 320, 180, 320 * 4).unwrap();
        assert_eq!(m.len(), W * H);
        assert!(m.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(m.iter().sum::<f32>() / (m.len() as f32) < 0.3, "no person in grey");
    }

    /// The worker thread takes a frame when free, delivers masks later, and ends when dropped.
    #[test]
    fn worker_delivers_masks_and_ends() {
        let mut w = Worker::start();
        let px = vec![128u8; 320 * 180 * 4];
        assert!(w.step(&px, 320, 180, 320 * 4).is_none(), "nothing finished yet");
        let m = w.wait(std::time::Duration::from_secs(30)).expect("a mask");
        assert_eq!(m.len(), W * H);
        assert!(w.failed.is_none() && w.last_ms > 0.0);
        // free again: the next frame goes in, its mask comes out on a later step
        let mut got = false;
        for _ in 0..3000 { if w.step(&px, 320, 180, 320 * 4).is_some() { got = true; break; } std::thread::sleep(std::time::Duration::from_millis(5)); }
        assert!(got);
        drop(w); // joins the thread
    }
}
