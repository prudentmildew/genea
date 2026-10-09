# Patched crates (PROTOTYPE, #18)

Verbatim copies of the crates.io sources, wired in through `[patch.crates-io]`, with env-gated experiments. Unset, every hook behaves exactly like upstream.

- `i-slint-common` 1.18.1, `sharedfontique.rs`: `SPIKE_NO_SYSTEM_FONTS=1` builds the font collection without scanning system fonts. Pair it with `SLINT_DEFAULT_FONT=/System/Library/Fonts/Menlo.ttc`. **Adopted**: Genea carries this as a patch until Slint offers an option.
- `i-slint-renderer-skia` 1.18.1, `wgpu_30_surface.rs` and `lib.rs`:
  - `SPIKE_SKIA_LOG=1` logs Skia's resource cache and the footprint at each render step;
  - `SPIKE_SLEEP_AT=<step>` pauses there, so `footprint` can be taken;
  - `SPIKE_SKIA_CACHE_MB` caps Skia's cache, `SPIKE_SKIA_PURGE` frees it after every frame, and `SPIKE_FRAME_LATENCY` sets wgpu's frame latency. None of them helped.
- `wgpu-hal` 30.0.1, `metal/surface.rs`: `SPIKE_NO_OCCLUSION_GATE=1` acquires drawables before AppKit reports the window visible. It didn't help: the window then appears later.
