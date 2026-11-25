# Video

More camera options for Webox. Everything is off by default, and everything runs on this computer: no picture goes
anywhere except, as usual, into the call. The effects show in the preview under Settings > Video.

| Setting | What it does |
|---|---|
| Video source: Camera / Video file / Picture, File | A video file (looping, at its own frame rate) or a picture instead of the camera. The camera stays off. Webox decodes the file with the system's own video framework (Media Foundation, GStreamer, AVFoundation), so what plays inline in Webox works here too. |
| Background: Off / Blur / Picture / Video, Background file, Blur strength | Finds you in the picture and blurs everything behind you, or replaces it with a picture or a looping video. |
| Mirror | The others see you mirrored (your own preview in Webox is mirrored anyway). |
| Brightness, Contrast | -100..100, 0 = unchanged. |
| Soft focus for the face | Smooths skin and flat areas, keeps edges sharp; only on you when a background effect is on. |
| Show name | Your name as a lower third, bottom left. |
| Low-light boost | Brightens a dark picture automatically (at most 3x, eased in, never darkens). |

## How it works

It needs Webox with plugin-api 1.6.1 (contract 1.6): the host methods `camera.filter`, `camera.source` and `media.*`
(Webox `docs/plugins.md`, "Camera pictures"). On an older Webox it says so in a toast and does nothing.

- **Video source** is `camera.source`: Webox itself opens the file in place of the camera.
- **Every other effect** runs in a frame filter (`Host::camera_filter`) that Webox calls once per camera picture with the
  full-size RGBA frame, on a thread of its own, under a 20 ms budget. A frame the plugin does not finish in time is skipped
  (Webox shows the previous one), so the camera never stutters because of the plugin. The filter is only registered
  while an effect is on: with everything off the plugin costs nothing per frame.
- **Order per frame:** brightness / contrast / low-light gain as one tone curve, background, soft focus, mirror, name (last,
  so it stays readable).
- **Background:** person segmentation (on a thread of its own, see below) with
  [MediaPipe Selfie Segmentation](https://developers.google.com/mediapipe/solutions/vision/image_segmenter) (landscape
  variant, 256 x 144), run on the CPU with [tract](https://github.com/sonos/tract) (pure Rust). The mask is smoothed over
  time (moving average) and given a soft edge, then scaled up. The blur works on a picture shrunk by 8; picture and video
  backgrounds are decoded by Webox (`media.open` / `media.read`) at the frame's size.

## Cost

Per 1280 x 720 frame, release build (median of 30 runs; `cargo test --release -p collabhost_video bench -- --ignored
--nocapture`). The per-pixel passes split the frame into up to 4 bands on scoped threads.

| Effect | ms |
|---|---|
| Brightness / Contrast | 1.0 |
| Low-light boost | 0.9 |
| Mirror | 0.7 |
| Soft focus for the face (100) | 5.9 |
| Show name | 0.05 |
| Background blur (without the model) | 3.3 |
| Picture background (without the model) | 1.1 |
| Segmentation input + mask smoothing (in the filter) | 0.2 |
| Everything on with a picture background (in the filter) | 6.9 |
| Segmentation model, on its own thread | 24 (load once: 80) |

The model takes longer than a frame's share of the budget, so it runs on a thread of its own: the filter hands over a
256 x 144 sample and takes the newest mask, which is then a frame or two behind (the smoothing hides it). Before the
first mask the whole picture counts as background, so the room never shows while the model loads.

## Third-party files

| File | Origin | Licence |
|---|---|---|
| `assets/selfie_segmentation_landscape.onnx` | MediaPipe Selfie Segmentation (landscape), (c) Google LLC; ONNX conversion `onnx/model.onnx` from [onnx-community/mediapipe_selfie_segmentation_landscape](https://huggingface.co/onnx-community/mediapipe_selfie_segmentation_landscape) (SHA-256 `7a0adcfdb1715d3b0ff0f61486d8a181c33f4a208343a5abfccbc6540e872d24`), unmodified, embedded in the library | [Apache License 2.0](assets/LICENSE-APACHE-2.0.txt) |
| `assets/selawksb.ttf` | Selawik Semibold, (c) Microsoft Corporation (the font Webox ships) | [SIL Open Font License 1.1](assets/Selawik-OFL.txt) |
