//! Tests for PSB (Large Document Format, version 2) support.
//!
//! Real PSB files are large and scarce, so these tests construct minimal synthetic
//! PSB byte buffers in memory. PSB differs from PSD only in the width of a handful of
//! length fields, so a hand-built buffer that exercises those widened fields is enough
//! to prove the version-aware parsing works end to end (including the `.rgba()` path
//! that the Anchorpoint thumbnailer relies on).

use psd::{ColorMode, Psd, PsdDepth, PsdVersion};

/// Append a big-endian u16.
fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Append a big-endian u32.
fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Append a big-endian u64.
fn push_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Build the 26-byte PSB file header.
///
/// `width`/`height` in pixels, RGB, 8-bit, `channel_count` channels.
fn psb_header(width: u32, height: u32, channel_count: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(26);
    h.extend_from_slice(b"8BPS"); // signature
    push_u16(&mut h, 2); // version 2 == PSB
    h.extend_from_slice(&[0u8; 6]); // reserved
    push_u16(&mut h, channel_count); // channels
    push_u32(&mut h, height); // height (PSB: 4 bytes, max 300,000)
    push_u32(&mut h, width); // width  (PSB: 4 bytes, max 300,000)
    push_u16(&mut h, 8); // depth (bits per channel)
    push_u16(&mut h, 3); // color mode 3 == RGB
    assert_eq!(h.len(), 26);
    h
}

/// Assemble a complete minimal PSB from an already-built image-data section.
///
/// Color Mode Data and Image Resources lengths stay 4 bytes in a PSB; the Layer and
/// Mask Information section length is 8 bytes. We emit all three sections empty.
fn assemble_psb(width: u32, height: u32, channel_count: u16, image_data: Vec<u8>) -> Vec<u8> {
    let mut buf = psb_header(width, height, channel_count);

    // Color Mode Data section: 4-byte length = 0.
    push_u32(&mut buf, 0);

    // Image Resources section: 4-byte length = 0.
    push_u32(&mut buf, 0);

    // Layer and Mask Information section: 8-byte length = 0 (PSB widens this field).
    push_u64(&mut buf, 0);

    // Image Data section (compression + planar channel data).
    buf.extend_from_slice(&image_data);

    buf
}

/// Build a raw (uncompressed) image-data section for an RGB(A) image whose channels
/// are given in planar order. Each `channels[i]` is the full plane for that channel.
fn raw_image_data(channels: &[Vec<u8>]) -> Vec<u8> {
    let mut data = Vec::new();
    push_u16(&mut data, 0); // compression 0 == raw
    for plane in channels {
        data.extend_from_slice(plane);
    }
    data
}

/// PackBits-encode a single scanline as a literal run (header byte `len-1`, then bytes).
fn packbits_literal(scanline: &[u8]) -> Vec<u8> {
    assert!(!scanline.is_empty() && scanline.len() <= 128);
    let mut out = Vec::with_capacity(scanline.len() + 1);
    out.push((scanline.len() - 1) as u8);
    out.extend_from_slice(scanline);
    out
}

/// Build an RLE-compressed image-data section. `planes[c][row]` is the raw bytes for
/// channel `c`, row `row`. In a PSB the per-scanline byte counts are stored as 4-byte
/// big-endian values (2-byte in a PSD) — this is the field this test exercises.
fn rle_image_data_psb(planes: &[Vec<Vec<u8>>]) -> Vec<u8> {
    let mut data = Vec::new();
    push_u16(&mut data, 1); // compression 1 == RLE

    // Encode every scanline up front so we know its compressed length.
    let mut encoded: Vec<Vec<Vec<u8>>> = Vec::new();
    for channel in planes {
        let mut ch = Vec::new();
        for row in channel {
            ch.push(packbits_literal(row));
        }
        encoded.push(ch);
    }

    // Byte-count table: for each channel, one 4-byte count per scanline (PSB width).
    for channel in &encoded {
        for row in channel {
            push_u32(&mut data, row.len() as u32);
        }
    }

    // The compressed scanline data itself.
    for channel in &encoded {
        for row in channel {
            data.extend_from_slice(row);
        }
    }

    data
}

#[test]
fn psb_header_is_recognized_as_version_two() {
    // 1x1 RGB, single channel, raw, value 200.
    let image_data = raw_image_data(&[vec![200]]);
    let bytes = assemble_psb(1, 1, 1, image_data);

    let psd = Psd::from_bytes(&bytes).expect("synthetic PSB should parse");
    assert_eq!(psd.version(), PsdVersion::Two);
    assert_eq!(psd.width(), 1);
    assert_eq!(psd.height(), 1);
    assert_eq!(psd.depth(), PsdDepth::Eight);
    assert_eq!(psd.color_mode(), ColorMode::Rgb);
    assert_eq!(psd.layers().len(), 0);
}

#[test]
fn psb_raw_rgb_composite_decodes() {
    // 2x2 RGB, raw. Planar order: all R, then all G, then all B.
    // Pixels (row-major): (255,0,0) (0,255,0) / (0,0,255) (255,255,0)
    let width = 2;
    let height = 2;
    let red = vec![255, 0, 0, 255];
    let green = vec![0, 255, 0, 255];
    let blue = vec![0, 0, 255, 0];

    let image_data = raw_image_data(&[red, green, blue]);
    let bytes = assemble_psb(width, height, 3, image_data);

    let psd = Psd::from_bytes(&bytes).expect("synthetic PSB should parse");
    assert_eq!(psd.version(), PsdVersion::Two);

    let rgba = psd.rgba();
    // No alpha channel -> fully opaque.
    let expected: Vec<u8> = vec![
        255, 0, 0, 255, // (0,0)
        0, 255, 0, 255, // (1,0)
        0, 0, 255, 255, // (0,1)
        255, 255, 0, 255, // (1,1)
    ];
    assert_eq!(rgba, expected);
}

#[test]
fn psb_raw_rgba_composite_decodes_alpha() {
    // 2x1 RGBA, raw. Pixel0 opaque red, Pixel1 half-transparent green.
    let width = 2;
    let height = 1;
    let red = vec![255, 0];
    let green = vec![0, 255];
    let blue = vec![0, 0];
    let alpha = vec![255, 128];

    let image_data = raw_image_data(&[red, green, blue, alpha]);
    let bytes = assemble_psb(width, height, 4, image_data);

    let psd = Psd::from_bytes(&bytes).expect("synthetic PSB should parse");
    let rgba = psd.rgba();
    let expected: Vec<u8> = vec![
        255, 0, 0, 255, // opaque red
        0, 255, 0, 128, // half-transparent green
    ];
    assert_eq!(rgba, expected);
}

#[test]
fn psb_rle_composite_decodes_with_four_byte_scanline_counts() {
    // 3x2 RGB, RLE. This exercises the PSB-widened 4-byte scanline byte counts.
    // Row 0: red, green, blue ; Row 1: white, black, grey
    let width = 3;
    let height = 2;

    // Per-channel, per-row raw bytes.
    let red = vec![vec![255, 0, 0], vec![255, 0, 128]];
    let green = vec![vec![0, 255, 0], vec![255, 0, 128]];
    let blue = vec![vec![0, 0, 255], vec![255, 0, 128]];

    let image_data = rle_image_data_psb(&[red, green, blue]);
    let bytes = assemble_psb(width, height, 3, image_data);

    let psd = Psd::from_bytes(&bytes).expect("synthetic RLE PSB should parse");
    assert_eq!(psd.version(), PsdVersion::Two);

    let rgba = psd.rgba();
    let expected: Vec<u8> = vec![
        // Row 0
        255, 0, 0, 255, // red
        0, 255, 0, 255, // green
        0, 0, 255, 255, // blue
        // Row 1
        255, 255, 255, 255, // white
        0, 0, 0, 255, // black
        128, 128, 128, 255, // grey
    ];
    assert_eq!(rgba, expected);
}

#[test]
fn psb_large_dimensions_are_accepted() {
    // PSD caps width/height at 30,000; PSB allows up to 300,000. We can't allocate a
    // real 40k-wide composite cheaply, so just assert the header parses (the image-data
    // section is intentionally too small, which would fail later, so we only parse the
    // header via a width that is valid for PSB but invalid for PSD and confirm the
    // dimension is accepted by building a 1-row image).
    let width = 40_000; // > 30,000, valid only for PSB
    let height = 1;
    let red = vec![0u8; width as usize];
    let green = vec![0u8; width as usize];
    let blue = vec![0u8; width as usize];
    let image_data = raw_image_data(&[red, green, blue]);
    let bytes = assemble_psb(width, height, 3, image_data);

    let psd = Psd::from_bytes(&bytes).expect("PSB should allow width > 30,000");
    assert_eq!(psd.width(), 40_000);
    assert_eq!(psd.height(), 1);
}

/// Build a minimal Layer and Mask Information section for a PSB containing a single
/// fully-opaque RGB layer that covers the whole image. This exercises the PSB-widened
/// 8-byte "Layer info length" and 8-byte per-channel data length fields, plus reading
/// the layer's channel image data.
fn psb_single_layer_section(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let px = (width * height) as usize;

    // Raw channel data for one channel = compression(u16=0) + one byte per pixel.
    let make_channel = |value: u8| -> Vec<u8> {
        let mut c = Vec::with_capacity(2 + px);
        push_u16(&mut c, 0); // raw
        c.extend(std::iter::repeat(value).take(px));
        c
    };
    let red_ch = make_channel(r);
    let green_ch = make_channel(g);
    let blue_ch = make_channel(b);

    // --- Layer record ---
    let mut record = Vec::new();
    // Rectangle: top, left, bottom, right (bottom/right are exclusive-ish per spec; the
    // crate subtracts 1 internally, so pass height/width directly).
    push_u32(&mut record, 0); // top
    push_u32(&mut record, 0); // left
    push_u32(&mut record, height); // bottom
    push_u32(&mut record, width); // right
    push_u16(&mut record, 3); // channel count

    // Channel info: id (i16) + data length. PSB widens the length to 8 bytes.
    // Length includes the 2 compression bytes.
    for (id, ch) in [(0i16, &red_ch), (1, &green_ch), (2, &blue_ch)] {
        record.extend_from_slice(&id.to_be_bytes());
        push_u64(&mut record, ch.len() as u64);
    }

    record.extend_from_slice(b"8BIM"); // blend mode signature
    record.extend_from_slice(b"norm"); // blend mode key
    record.push(255); // opacity
    record.push(0); // clipping (0 = base)
    record.push(1 << 1); // flags: visible bit set
    record.push(0); // filler

    // Extra data field: layer mask (u32 len=0) + blending ranges (u32 len=0) + name.
    let mut extra = Vec::new();
    push_u32(&mut extra, 0); // layer mask data length
    push_u32(&mut extra, 0); // layer blending ranges length
                             // Pascal string name "L", padded to a multiple of 4 bytes (incl. the length byte).
    extra.push(1); // name length
    extra.push(b'L'); // name
    extra.push(0); // pad
    extra.push(0); // pad
    push_u32(&mut record, extra.len() as u32);
    record.extend_from_slice(&extra);

    // --- Channel image data (follows all layer records) ---
    let mut channel_data = Vec::new();
    channel_data.extend_from_slice(&red_ch);
    channel_data.extend_from_slice(&green_ch);
    channel_data.extend_from_slice(&blue_ch);

    // --- Layer info block ---
    let mut layer_info_body = Vec::new();
    push_u16(&mut layer_info_body, 1); // layer count = 1
    layer_info_body.extend_from_slice(&record);
    layer_info_body.extend_from_slice(&channel_data);

    // Layer info length is 8 bytes in a PSB.
    let mut layer_info = Vec::new();
    push_u64(&mut layer_info, layer_info_body.len() as u64);
    layer_info.extend_from_slice(&layer_info_body);

    // Global layer mask info: 4-byte length = 0 (stays 4 bytes in PSB).
    push_u32(&mut layer_info, 0);

    // Section wrapper: 8-byte total length in a PSB.
    let mut section = Vec::new();
    push_u64(&mut section, layer_info.len() as u64);
    section.extend_from_slice(&layer_info);
    section
}

#[test]
fn psb_with_one_layer_parses_and_reads_channels() {
    // 2x2 PSB with a single solid orange layer covering the whole canvas.
    let width = 2;
    let height = 2;
    let (r, g, b) = (200u8, 120u8, 40u8);

    let mut buf = psb_header(width, height, 3);
    push_u32(&mut buf, 0); // color mode data
    push_u32(&mut buf, 0); // image resources
    buf.extend_from_slice(&psb_single_layer_section(width, height, r, g, b));

    // A merged composite is still required for `.rgba()`. Provide a matching raw one.
    let plane = |v: u8| vec![v; (width * height) as usize];
    let image_data = raw_image_data(&[plane(r), plane(g), plane(b)]);
    buf.extend_from_slice(&image_data);

    let psd = Psd::from_bytes(&buf).expect("synthetic PSB with a layer should parse");
    assert_eq!(psd.version(), PsdVersion::Two);
    assert_eq!(
        psd.layers().len(),
        1,
        "should have parsed exactly one layer"
    );

    let layer = &psd.layers()[0];
    assert_eq!(layer.name(), "L");

    // The layer's own RGBA should be the solid color we wrote.
    let layer_rgba = layer.rgba();
    assert_eq!(&layer_rgba[0..4], &[r, g, b, 255]);
    assert_eq!(layer_rgba.len(), (width * height * 4) as usize);

    // The merged composite path used by the thumbnailer.
    let rgba = psd.rgba();
    assert_eq!(&rgba[0..4], &[r, g, b, 255]);

    // Flattening should also yield the solid color.
    let flat = psd.flatten_layers_rgba(&|_| true).unwrap();
    assert_eq!(&flat[0..4], &[r, g, b, 255]);
}

/// A standard PSD (version 1) buffer built the same way must still parse, proving the
/// version-aware reads did not regress the PSD path.
#[test]
fn psd_version_one_still_parses_via_synthetic_buffer() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"8BPS");
    push_u16(&mut buf, 1); // version 1 == PSD
    buf.extend_from_slice(&[0u8; 6]);
    push_u16(&mut buf, 3); // channels
    push_u32(&mut buf, 1); // height
    push_u32(&mut buf, 1); // width
    push_u16(&mut buf, 8); // depth
    push_u16(&mut buf, 3); // RGB

    push_u32(&mut buf, 0); // color mode data len (4 bytes)
    push_u32(&mut buf, 0); // image resources len (4 bytes)
    push_u32(&mut buf, 0); // layer & mask len (4 bytes in PSD)

    // Image data: raw RGB, one pixel.
    push_u16(&mut buf, 0); // compression raw
    buf.push(10); // R
    buf.push(20); // G
    buf.push(30); // B

    let psd = Psd::from_bytes(&buf).expect("synthetic PSD should parse");
    assert_eq!(psd.version(), PsdVersion::One);
    assert_eq!(psd.rgba(), vec![10, 20, 30, 255]);
}
