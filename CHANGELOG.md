# psd Changelog

Types of changes:

- `[added]` for new features.
- `[changed]` for changes in existing functionality.
- `[deprecated]` for once-stable features removed in upcoming releases.
- `[removed]` for deprecated features removed in this release.
- `[fixed]` for any bug fixes.
- `[security]` to invite users to upgrade in case of vulnerabilities.

## Anchorpoint fork

_Vendored fork of `chinedufn/psd` v0.3.5. Deviations from upstream:_

- [added] Support for Photoshop PSB (Large Document Format, version 2) files: version-aware
  section/length/scanline widths, sharing the PSD parser and `.rgba()` path.
- [added] 32-bit/channel float composites render: samples are clamped to [0,1] and sRGB-encoded
  (Photoshop stores 32-bit as linear light), alongside the existing 8/16-bit paths. Validated
  against the OIIO `psd_rgb_8`/`psd_rgb_32` pair. HDR values above 1.0 clip to white.
- [changed] Parser hardened so malformed or unsupported input never panics — the thumbnailer
  must degrade to a fallback rather than unwind:
  - [fixed] 16-bit raw composites now down-sample every channel (previously only red was
    converted, overflowing the RGBA buffer and panicking on green/blue).
  - [fixed] A flattened file with a zero-length layer-info block no longer reads a phantom
    layer record past the section end.
  - [changed] `PsdCursor` reads clamp to the buffer and zero-pad fixed-width integers at EOF,
    so truncated files yield short reads instead of out-of-bounds panics.
  - [changed] ZIP-compressed layer channels degrade to an empty channel (the composite still
    renders); a ZIP-compressed composite returns a clean `Err` instead of `unimplemented!`.
  - [changed] Image-resource parsing is bounded and no longer asserts on truncated input.
- [known] Large documents (16/32-bit) store their layers in 'Lr16'/'Lr32' tagged blocks with
  the main layer-info length set to 0; we don't parse those, so `layers()` is empty and the
  metadata layer count is reported as 0 for such files. The merged composite (what the
  thumbnailer renders) is unaffected. Parsing them is entangled with the upstream crate's
  inconsistent layer-visibility handling and its lack of 16/32-bit per-layer rendering, so it
  is deferred to a dedicated effort.

## Not Yet Published

_Here we list notable things that have been merged into the master branch but have not been released yet._

- [added] Public information on the position of each layer (E.g. `layer_top`, `layer_bottom`. `layer_left`, `layer_right`).

## 0.1.8 - April 23, 2020

- [fixed] Parsing of slices resource section [PR][17]

## 0.1.7 - April 11, 2020

- [added] Support for PSD groups [PR][13]
  - @tdakkota

[13]: https://github.com/chinedufn/psd/pull/13
[17]: https://github.com/chinedufn/psd/pull/17
