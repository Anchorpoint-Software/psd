//! Robustness tests: the thumbnailer must never panic on real-world or malformed
//! PSD/PSB input. Every file should either render from its composite or fail with a
//! clean `Result::Err` — never unwind. These cases reproduce panics observed against
//! real files (16-bit composites, flattened files with an empty layer-info block,
//! ZIP-compressed layers) plus a truncation fuzz over a valid buffer.

use psd::Psd;

fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}
fn push_i16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_be_bytes());
}
fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}
fn push_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// 26-byte PSD (version 1) file header.
fn psd_header(width: u32, height: u32, channels: u16, depth: u16, color_mode: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(26);
    h.extend_from_slice(b"8BPS");
    push_u16(&mut h, 1); // version 1 == PSD
    h.extend_from_slice(&[0u8; 6]); // reserved
    push_u16(&mut h, channels);
    push_u32(&mut h, height);
    push_u32(&mut h, width);
    push_u16(&mut h, depth);
    push_u16(&mut h, color_mode);
    assert_eq!(h.len(), 26);
    h
}

/// Raw (uncompressed) image-data section from already-laid-out planar bytes.
fn raw_image_data(planes: &[&[u8]]) -> Vec<u8> {
    let mut d = Vec::new();
    push_u16(&mut d, 0); // compression 0 == raw
    for p in planes {
        d.extend_from_slice(p);
    }
    d
}

/// RLE-compressed image-data section for `channels` whole-canvas channels of `height`
/// scanlines. The packed payload is filler — `Psd::from_bytes` slices the channel data
/// out of the section but does not decode the packed bytes, so only the scanline
/// byte-count table and the resulting section length matter for exercising the composite
/// RLE slicing under truncation.
fn rle_image_data(height: u32, channels: u16) -> Vec<u8> {
    let scanline_len: u16 = 3; // arbitrary per-scanline packed length
    let mut d = Vec::new();
    push_u16(&mut d, 1); // compression 1 == RLE
    for _ in 0..(channels as u32 * height) {
        push_u16(&mut d, scanline_len); // per-scanline byte count (PSD: 2 bytes each)
    }
    let packed = channels as usize * height as usize * scanline_len as usize;
    d.extend(std::iter::repeat(0u8).take(packed));
    d
}

/// Assemble a complete PSD from a layer-and-mask section (including its own length
/// marker) and an image-data section.
fn assemble(header: Vec<u8>, layer_and_mask: &[u8], image_data: &[u8]) -> Vec<u8> {
    let mut buf = header;
    push_u32(&mut buf, 0); // color mode data length
    push_u32(&mut buf, 0); // image resources length
    buf.extend_from_slice(layer_and_mask);
    buf.extend_from_slice(image_data);
    buf
}

/// A PSD layer-and-mask section with exactly one whole-canvas RGB layer, whose three
/// channels each use the given `channel_compression` (0=raw, 1=rle, 2/3=zip). The
/// channel payloads are zero bytes — for this fixture only "does it parse without
/// panicking" matters; the composite is what `.rgba()` renders.
fn one_layer_section(width: u32, height: u32, channel_compression: u16) -> Vec<u8> {
    let body = layer_info_body(width, height, channel_compression);

    let mut layer_info = Vec::new();
    push_u32(&mut layer_info, body.len() as u32); // layer info length (PSD: 4 bytes)
    layer_info.extend_from_slice(&body);
    push_u32(&mut layer_info, 0); // global layer mask info length

    let mut section = Vec::new();
    push_u32(&mut section, layer_info.len() as u32); // section length (PSD: 4 bytes)
    section.extend_from_slice(&layer_info);
    section
}

/// A layer-info body for one whole-canvas RGB layer: layer count + record + channel data.
/// Shared by the main layer-info block and the 'Lr16'/'Lr32'/'Layr' tagged blocks that
/// large (16/32-bit) documents use to store their layers.
fn layer_info_body(width: u32, height: u32, channel_compression: u16) -> Vec<u8> {
    let px = (width * height) as usize;
    let payload = vec![0u8; px];
    let channel_len_field = (2 + payload.len()) as u32; // 2 compression bytes + payload

    let mut record = Vec::new();
    push_u32(&mut record, 0); // top
    push_u32(&mut record, 0); // left
    push_u32(&mut record, height); // bottom
    push_u32(&mut record, width); // right
    push_u16(&mut record, 3); // channel count
    for id in 0i16..3 {
        push_i16(&mut record, id);
        push_u32(&mut record, channel_len_field);
    }
    record.extend_from_slice(b"8BIM");
    record.extend_from_slice(b"norm");
    record.push(255); // opacity
    record.push(0); // clipping
    record.push(1 << 1); // flags: visible
    record.push(0); // filler

    let mut extra = Vec::new();
    push_u32(&mut extra, 0); // layer mask length
    push_u32(&mut extra, 0); // blending ranges length
    extra.push(1); // name length
    extra.push(b'L');
    extra.push(0); // pad to multiple of 4 (1 len byte + 1 char + 2 pad)
    extra.push(0);
    push_u32(&mut record, extra.len() as u32);
    record.extend_from_slice(&extra);

    // Channel image data block: per channel, compression marker + payload.
    let mut channel_data = Vec::new();
    for _ in 0..3 {
        push_u16(&mut channel_data, channel_compression);
        channel_data.extend_from_slice(&payload);
    }

    let mut body = Vec::new();
    push_i16(&mut body, 1); // layer count
    body.extend_from_slice(&record);
    body.extend_from_slice(&channel_data);
    body
}

#[test]
fn sixteen_bit_raw_rgb_composite_renders_all_channels() {
    // 2x1, 16-bit RGB, raw. Each 16-bit sample maps down to its high byte.
    let header = psd_header(2, 1, 3, 16, 3);
    let red = [0x12, 0x34, 0x56, 0x78];
    let green = [0xAB, 0x00, 0xCD, 0x00];
    let blue = [0x00, 0xFF, 0xFF, 0x00];
    let image_data = raw_image_data(&[&red, &green, &blue]);
    let no_layers = {
        let mut s = Vec::new();
        push_u32(&mut s, 0);
        s
    };
    let bytes = assemble(header, &no_layers, &image_data);

    let psd = Psd::from_bytes(&bytes).expect("16-bit RGB PSD should parse");
    let rgba = psd.rgba();
    assert_eq!(
        rgba,
        vec![0x12, 0xAB, 0x00, 255, 0x56, 0xCD, 0xFF, 255],
        "all three 16-bit channels must be down-sampled, not just red"
    );
}

#[test]
fn thirty_two_bit_float_raw_composite_srgb_encodes_all_channels() {
    // 1x1, 32-bit float RGB, raw. 32-bit channels are linear-light float; they are
    // clamped to [0,1] and sRGB-encoded for display (validated against the 8-bit version
    // of the same OIIO image). HDR values above 1.0 clip to white.
    //   0.0 -> 0
    //   0.5 -> round(sRGB(0.5) * 255) = 188
    //   2.0 -> clamp 1.0 -> 255
    let header = psd_header(1, 1, 3, 32, 3);
    let mut red = Vec::new();
    push_f32(&mut red, 0.0);
    let mut green = Vec::new();
    push_f32(&mut green, 0.5);
    let mut blue = Vec::new();
    push_f32(&mut blue, 2.0);
    let image_data = raw_image_data(&[&red, &green, &blue]);
    let no_layers = {
        let mut s = Vec::new();
        push_u32(&mut s, 0);
        s
    };
    let bytes = assemble(header, &no_layers, &image_data);

    let psd = Psd::from_bytes(&bytes).expect("32-bit float PSD should parse");
    assert_eq!(psd.rgba(), vec![0, 188, 255, 255]);
}

#[test]
fn empty_layer_info_block_does_not_overread() {
    // Section length is non-zero (4) but the layer-info length inside it is 0: a
    // flattened file with a global-mask-only section. The parser must not read a
    // phantom layer count past the section. (Reproduces the 6K.psd panic.)
    let header = psd_header(1, 1, 3, 8, 3);
    let mut layer_section = Vec::new();
    push_u32(&mut layer_section, 4); // section length = 4
    push_u32(&mut layer_section, 0); // layer info length = 0 (no layers)
    let image_data = raw_image_data(&[&[10], &[20], &[30]]);
    let bytes = assemble(header, &layer_section, &image_data);

    let psd = Psd::from_bytes(&bytes).expect("flattened PSD should parse");
    assert_eq!(psd.layers().len(), 0);
    assert_eq!(psd.rgba(), vec![10, 20, 30, 255]);
}

#[test]
fn zip_compressed_layer_degrades_and_composite_still_renders() {
    // A layer whose channels are ZIP-compressed (2). We can't decode ZIP, but parsing
    // must not panic and the (raw) composite must still render.
    let header = psd_header(2, 2, 3, 8, 3);
    let layer_section = one_layer_section(2, 2, 2); // compression 2 == zip
    let r = [255u8; 4];
    let g = [128u8; 4];
    let b = [0u8; 4];
    let image_data = raw_image_data(&[&r, &g, &b]);
    let bytes = assemble(header, &layer_section, &image_data);

    let psd = Psd::from_bytes(&bytes).expect("PSD with a zip layer should still parse");
    assert_eq!(&psd.rgba()[0..4], &[255, 128, 0, 255]);
}

#[test]
fn zip_compressed_composite_is_a_clean_error_not_a_panic() {
    // The merged image data itself is ZIP-compressed: we cannot render it, but we must
    // return Err rather than unwind.
    let header = psd_header(1, 1, 3, 8, 3);
    let no_layers = {
        let mut s = Vec::new();
        push_u32(&mut s, 0);
        s
    };
    let mut image_data = Vec::new();
    push_u16(&mut image_data, 2); // compression 2 == zip without prediction
    image_data.extend_from_slice(&[0u8; 8]); // some bytes
    let bytes = assemble(header, &no_layers, &image_data);

    assert!(
        Psd::from_bytes(&bytes).is_err(),
        "a zip composite must be a clean Err, not a panic"
    );
}

#[test]
fn oversized_raw_channel_does_not_overflow_rgba_buffer() {
    // A malformed header can declare a smaller canvas than the channel data actually
    // contains. Writing the channel into the RGBA buffer must be bounds-checked and
    // drop the overflow rather than panicking.
    let header = psd_header(1, 1, 3, 8, 3); // claims 1x1 (RGBA buffer is 4 bytes)
    // ...but provide 4 bytes per channel (16x too much data).
    let red = [10u8, 11, 12, 13];
    let green = [20u8, 21, 22, 23];
    let blue = [30u8, 31, 32, 33];
    let image_data = raw_image_data(&[&red, &green, &blue]);
    let no_layers = {
        let mut s = Vec::new();
        push_u32(&mut s, 0);
        s
    };
    let bytes = assemble(header, &no_layers, &image_data);

    let psd = Psd::from_bytes(&bytes).expect("PSD with oversized channels should parse");
    // Only the first pixel fits; the result is well-formed and matches it.
    assert_eq!(psd.rgba(), vec![10, 20, 30, 255]);
}

#[test]
fn truncating_a_valid_psd_at_any_offset_never_panics() {
    // Build a valid one-layer PSD, confirm it parses, then truncate it at every byte
    // offset and assert that parsing never unwinds (Ok or Err are both fine).
    let header = psd_header(2, 2, 3, 8, 3);
    let layer_section = one_layer_section(2, 2, 0); // raw layer
    let r = [255u8; 4];
    let g = [128u8; 4];
    let b = [0u8; 4];
    let image_data = raw_image_data(&[&r, &g, &b]);
    let full = assemble(header, &layer_section, &image_data);

    Psd::from_bytes(&full).expect("baseline buffer must parse");

    for cut in 0..=full.len() {
        let slice = full[..cut].to_vec();
        let result = std::panic::catch_unwind(|| {
            let _ = Psd::from_bytes(&slice);
        });
        assert!(
            result.is_ok(),
            "Psd::from_bytes panicked on a buffer truncated to {cut} bytes"
        );
    }
}

#[test]
fn truncating_a_valid_rle_psd_at_any_offset_never_panics() {
    // The composite RLE path computes channel offsets from the header (channel_data_start
    // depends on psd_height, not the buffer length), so a truncated RLE image-data section
    // must be sliced defensively. The raw fuzz above never exercises this path.
    let header = psd_header(2, 2, 3, 8, 3);
    let no_layers = {
        let mut s = Vec::new();
        push_u32(&mut s, 0);
        s
    };
    let image_data = rle_image_data(2, 3);
    let full = assemble(header, &no_layers, &image_data);

    Psd::from_bytes(&full).expect("baseline RLE buffer must parse");

    for cut in 0..=full.len() {
        let slice = full[..cut].to_vec();
        let result = std::panic::catch_unwind(|| {
            let _ = Psd::from_bytes(&slice);
        });
        assert!(
            result.is_ok(),
            "Psd::from_bytes panicked on an RLE buffer truncated to {cut} bytes"
        );
    }
}
