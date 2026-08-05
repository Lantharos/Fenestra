use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};

pub(crate) fn guest_preview_data_url(
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<String, String> {
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "guest frame is too large to capture".to_string())?;
    if width == 0 || height == 0 || bytes.len() != expected_len {
        return Err("guest has no frame to capture".to_string());
    }
    let mut rgb = Vec::with_capacity(expected_len / 4 * 3);
    for pixel in bytes.chunks_exact(4) {
        let inverse_alpha = 255_u16 - u16::from(pixel[3]);
        let composite =
            |channel: u8| (u16::from(channel) + (10 * inverse_alpha + 127) / 255).min(255) as u8;
        rgb.extend_from_slice(&[
            composite(pixel[2]),
            composite(pixel[1]),
            composite(pixel[0]),
        ]);
    }
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Sub)
        .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
        .map_err(|error| format!("failed to encode guest preview: {error}"))?;
    Ok(format!("data:image/png;base64,{}", base64_encode(&png)))
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[((value >> 18) & 0x3f) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 0x3f) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(value & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}
