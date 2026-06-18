use crate::psd_channel::PsdChannelCompression;
use crate::sections::file_header_section::PsdVersion;
use crate::sections::PsdCursor;
use crate::PsdDepth;
use thiserror::Error;

/// Represents an malformed image data
#[derive(Debug, PartialEq, Error)]
pub enum ImageDataSectionError {
    #[error(
        r#"Only 8 and 16 bit depths are supported at the moment.
    If you'd like to see 1 and 32 bit depths supported - please open an issue."#
    )]
    UnsupportedDepth,

    #[error("{compression} is an invalid layer channel compression. Must be 0, 1, 2 or 3")]
    InvalidCompression { compression: u16 },

    #[error(
        r#"ZIP-compressed merged image data is not supported.
    The composite cannot be decoded; the file degrades to a fallback thumbnail."#
    )]
    UnsupportedCompression,
}

/// The ImageDataSection comes from the final section in the PSD that contains the pixel data
/// of the final PSD image (the one that comes from combining all of the layers).
///
/// # [Adobe Docs](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/)
///
/// The last section of a Photoshop file contains the image pixel data.
/// Image data is stored in planar order: first all the red data, then all the green data, etc.
/// Each plane is stored in scan-line order, with no pad bytes,
///
/// | Length   | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
/// |----------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
/// | 2        | Compression method: <br> 0 = Raw image data <br> 1 = RLE compressed the image data starts with the byte counts for all the scan lines (rows * channels), with each count stored as a two-byte value. The RLE compressed data follows, with each scan line compressed separately. The RLE compression is the same compression algorithm used by the Macintosh ROM routine PackBits , and the TIFF standard. <br> 2 = ZIP without prediction <br> 3 = ZIP with prediction. |
/// | Variable | The image data. Planar order = RRR GGG BBB, etc.                                                                                                                                                                                                                                                                                                                                                                                                                         |
#[derive(Debug)]
pub struct ImageDataSection {
    /// The compression method for the image.
    pub(crate) compression: PsdChannelCompression,
    /// The red channel of the final image
    pub(crate) red: ChannelBytes,
    /// The green channel of the final image
    pub(crate) green: Option<ChannelBytes>,
    /// the blue channel of the final image
    pub(crate) blue: Option<ChannelBytes>,
    /// the alpha channel of the final image.
    /// If there is no alpha channel then it is a fully opaque image.
    pub(crate) alpha: Option<ChannelBytes>,
}

impl ImageDataSection {
    /// Create an ImageDataSection from the bytes in the corresponding section in a PSD file
    /// (including the length market)
    pub fn from_bytes(
        bytes: &[u8],
        depth: PsdDepth,
        psd_height: u32,
        channel_count: u8,
        version: PsdVersion,
    ) -> Result<ImageDataSection, ImageDataSectionError> {
        let mut cursor = PsdCursor::new(bytes);
        let channel_count = channel_count as usize;

        let compression = cursor.read_u16();
        let compression = PsdChannelCompression::new(compression)
            .ok_or(ImageDataSectionError::InvalidCompression { compression })?;

        let (red, green, blue, alpha) = match compression {
            PsdChannelCompression::RawData => {
                if !matches!(
                    depth,
                    PsdDepth::Eight | PsdDepth::Sixteen | PsdDepth::ThirtyTwo
                ) {
                    return Err(ImageDataSectionError::UnsupportedDepth);
                }

                // First 2 bytes were the compression marker.
                let channel_bytes = bytes.get(2..).unwrap_or(&[]);
                let bytes_per_channel = if channel_count == 0 {
                    0
                } else {
                    channel_bytes.len() / channel_count
                };

                // Map every channel down to 8 bits per sample. 16-bit samples are two
                // big-endian bytes (keep the high byte); 32-bit samples are linear-light
                // floats, clamped and sRGB-encoded (see `linear_f32_to_u8`). (Previously
                // only red was converted, leaving the other channels at full width and
                // overflowing the RGBA buffer on render.)
                let convert = |plane: &[u8]| -> Vec<u8> {
                    match depth {
                        PsdDepth::Sixteen => plane.chunks_exact(2).map(|s| s[0]).collect(),
                        PsdDepth::ThirtyTwo => plane.chunks_exact(4).map(linear_f32_to_u8).collect(),
                        _ => plane.to_vec(),
                    }
                };
                let plane = |i: usize| -> &[u8] {
                    let start = i * bytes_per_channel;
                    channel_bytes
                        .get(start..start + bytes_per_channel)
                        .unwrap_or(&[])
                };

                let red = ChannelBytes::RawData(convert(plane(0)));
                let green = (channel_count >= 2).then(|| ChannelBytes::RawData(convert(plane(1))));
                let blue = (channel_count >= 3).then(|| ChannelBytes::RawData(convert(plane(2))));
                let alpha = (channel_count >= 4).then(|| ChannelBytes::RawData(convert(plane(3))));

                (red, green, blue, alpha)
            }
            // # [Adobe Docs](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/)
            //
            // RLE compressed the image data starts with the byte counts for all the scan lines
            // (rows * channels), with each count stored as a two-byte value. The RLE compressed
            // data follows, with each scan line compressed separately. The RLE compression is
            // the same compression algorithm used by the Macintosh ROM routine PackBits,
            // and the TIFF standard.
            PsdChannelCompression::RleCompressed => {
                let mut red_byte_count = 0;
                let mut green_byte_count = if channel_count >= 2 { Some(0) } else { None };
                let mut blue_byte_count = if channel_count >= 3 { Some(0) } else { None };
                let mut alpha_byte_count = if channel_count == 4 { Some(0) } else { None };

                // Each scanline byte-count entry is 2 bytes in a PSD and 4 bytes in a PSB.
                let scanline_count_bytes = version.rle_scanline_len_bytes();
                let mut read_scanline_count = |cursor: &mut PsdCursor| -> usize {
                    if version.is_psb() {
                        cursor.read_u32() as usize
                    } else {
                        cursor.read_u16() as usize
                    }
                };

                for _ in 0..psd_height {
                    red_byte_count += read_scanline_count(&mut cursor);
                }

                if let Some(ref mut green_byte_count) = green_byte_count {
                    for _ in 0..psd_height {
                        *green_byte_count += read_scanline_count(&mut cursor);
                    }
                }

                if let Some(ref mut blue_byte_count) = blue_byte_count {
                    for _ in 0..psd_height {
                        *blue_byte_count += read_scanline_count(&mut cursor);
                    }
                }

                if let Some(ref mut alpha_byte_count) = alpha_byte_count {
                    for _ in 0..psd_height {
                        *alpha_byte_count += read_scanline_count(&mut cursor);
                    }
                }

                // 2 bytes for compression level, then the per-scanline byte counts for each
                // channel (one entry per scanline of each channel).
                // We're skipping over the bytes that describe the length of each scanline since
                // we don't currently use them. We might re-think this in the future when we
                // implement serialization of a Psd back into bytes.. But not a concern at the
                // moment.
                let channel_data_start =
                    2 + (channel_count * psd_height as usize * scanline_count_bytes);

                let (red_start, red_end) =
                    (channel_data_start, channel_data_start + red_byte_count);

                // Channel offsets derive from the header (channel_data_start depends on
                // psd_height, not the buffer length), so a truncated or malformed RLE
                // section can push these past the end. Slice defensively — an out-of-range
                // channel degrades to empty rather than panicking (mirrors the layer path).
                let red = bytes.get(red_start..red_end).unwrap_or(&[]).into();

                let green = match green_byte_count {
                    Some(green_byte_count) => {
                        let green_start = red_end;
                        let green_end = green_start + green_byte_count;
                        Some(ChannelBytes::RleCompressed(
                            bytes.get(green_start..green_end).unwrap_or(&[]).into(),
                        ))
                    }
                    None => None,
                };

                let blue = match blue_byte_count {
                    Some(blue_byte_count) => {
                        let blue_start = red_end + green_byte_count.unwrap();
                        let blue_end = blue_start + blue_byte_count;
                        Some(ChannelBytes::RleCompressed(
                            bytes.get(blue_start..blue_end).unwrap_or(&[]).into(),
                        ))
                    }
                    None => None,
                };

                let alpha = match alpha_byte_count {
                    Some(alpha_byte_count) => {
                        let alpha_start =
                            red_end + green_byte_count.unwrap() + blue_byte_count.unwrap();
                        let alpha_end = alpha_start + alpha_byte_count;
                        Some(ChannelBytes::RleCompressed(
                            bytes.get(alpha_start..alpha_end).unwrap_or(&[]).into(),
                        ))
                    }
                    None => None,
                };

                (ChannelBytes::RleCompressed(red), green, blue, alpha)
            }
            // ZIP-compressed composites are rare (Photoshop almost always writes the
            // merged image as raw or RLE). We don't decode ZIP, so return a clean error
            // and let the caller fall back rather than unwinding.
            PsdChannelCompression::ZipWithoutPrediction
            | PsdChannelCompression::ZipWithPrediction => {
                return Err(ImageDataSectionError::UnsupportedCompression)
            }
        };

        Ok(ImageDataSection {
            compression,
            red,
            green,
            blue,
            alpha,
        })
    }
}

/// Convert a big-endian 32-bit float sample to an 8-bit display value.
///
/// 32-bit PSD channels are linear-light floating point (the mode Photoshop uses for HDR),
/// so they can't be bit-shifted like 16-bit. We clamp to [0, 1] and apply the sRGB
/// transfer function. Validated against the OIIO `psd_rgb_8` vs `psd_rgb_32` pair (the
/// same image at both depths): sRGB reproduces the 8-bit reference, whereas a Reinhard
/// tone-map (what the EXR extractor uses) noticeably under-exposes SDR-range content.
/// HDR values above 1.0 clip to white — acceptable for a thumbnail. NaN/negative inputs
/// saturate to 0 via the clamp and the saturating `as u8` cast.
fn linear_f32_to_u8(sample: &[u8]) -> u8 {
    let v = f32::from_be_bytes([sample[0], sample[1], sample[2], sample[3]]).clamp(0.0, 1.0);
    let srgb = if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    // Round (not truncate): float error otherwise drops pure white to 254, and rounding
    // is the less-biased quantization. srgb is in [0, 1], so this stays within 0..=255.
    (srgb * 255.0).round() as u8
}

#[derive(Debug, Clone)]
pub enum ChannelBytes {
    RawData(Vec<u8>),
    RleCompressed(Vec<u8>),
}
