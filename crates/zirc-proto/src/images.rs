//! What an uploaded image really is, from its first bytes rather than its name or what the app
//! said: its type, and its size as it's shown, after the rotation its camera recorded (it's
//! turned upright when resized, so apps lay it out the way it will arrive). JPEG, PNG, GIF,
//! WebP, HEIC and AVIF; anything else is `None`, and kept as a file.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// An image's type and size as it's shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Probed {
    /// Its media type, such as `image/jpeg`.
    pub mime: String,
    /// Its width in pixels, upright.
    pub width: u32,
    /// Its height in pixels, upright.
    pub height: u32,
}

/// The image `bytes` start, if they start one we can show; cut short or malformed is `None`.
#[must_use]
pub fn probe(bytes: &[u8]) -> Option<Probed> {
    let b = Bytes(bytes);
    png(b)
        .or_else(|| gif(b))
        .or_else(|| jpeg(b))
        .or_else(|| webp(b))
        .or_else(|| heif(b))
}

/// Reads that are `None` past the end, as a probe of a file cut short must be.
#[derive(Clone, Copy)]
struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn len(self) -> usize {
        self.0.len()
    }
    fn at(self, at: usize) -> Option<u8> {
        self.0.get(at).copied()
    }
    fn is(self, at: usize, text: &[u8]) -> bool {
        self.0.get(at..at + text.len()) == Some(text)
    }
    fn be16(self, at: usize) -> Option<u32> {
        Some(u32::from(u16::from_be_bytes(
            self.0.get(at..at + 2)?.try_into().ok()?,
        )))
    }
    fn le16(self, at: usize) -> Option<u32> {
        Some(u32::from(u16::from_le_bytes(
            self.0.get(at..at + 2)?.try_into().ok()?,
        )))
    }
    fn be32(self, at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(self.0.get(at..at + 4)?.try_into().ok()?))
    }
    fn le32(self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.0.get(at..at + 4)?.try_into().ok()?))
    }
    fn le24(self, at: usize) -> Option<u32> {
        let b = self.0.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    }
    /// Where `text` is next, from `from`.
    fn find(self, text: &[u8], from: usize) -> Option<usize> {
        self.0
            .get(from..)?
            .windows(text.len())
            .position(|w| w == text)
            .map(|at| at + from)
    }
}

fn sized(mime: &str, width: u32, height: u32, turned: bool) -> Option<Probed> {
    if width == 0 || height == 0 {
        return None;
    }
    let (width, height) = if turned {
        (height, width)
    } else {
        (width, height)
    };
    Some(Probed {
        mime: mime.to_owned(),
        width,
        height,
    })
}

fn png(b: Bytes) -> Option<Probed> {
    if b.len() < 24 || !b.is(0, b"\x89PNG\r\n\x1a\n") || !b.is(12, b"IHDR") {
        return None;
    }
    sized("image/png", b.be32(16)?, b.be32(20)?, false)
}

fn gif(b: Bytes) -> Option<Probed> {
    if b.len() < 10 || !(b.is(0, b"GIF87a") || b.is(0, b"GIF89a")) {
        return None;
    }
    sized("image/gif", b.le16(6)?, b.le16(8)?, false)
}

fn jpeg(b: Bytes) -> Option<Probed> {
    if b.at(0)? != 0xff || b.at(1)? != 0xd8 {
        return None;
    }
    let mut turned = false;
    let mut at = 2;
    while at + 4 <= b.len() {
        if b.at(at)? != 0xff {
            return None;
        }
        let marker = b.at(at + 1)?;
        if marker == 0xff {
            at += 1;
            continue;
        }
        let length = usize::try_from(b.be16(at + 2)?).ok()?;
        // EXIF, for the camera's orientation: 5 to 8 are turned a quarter.
        if marker == 0xe1 && b.is(at + 4, b"Exif\0\0") {
            let tiff = Bytes(b.0.get(at + 10..(at + 2 + length).min(b.len()))?);
            turned = orientation(tiff).unwrap_or(1) >= 5;
        }
        // A start of frame, but not DHT (C4), JPG (C8) or DAC (CC).
        if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
            return sized("image/jpeg", b.be16(at + 7)?, b.be16(at + 5)?, turned);
        }
        at += 2 + length;
    }
    None
}

/// The orientation tag (0x0112) of a TIFF header's first directory.
fn orientation(tiff: Bytes) -> Option<u32> {
    let little = tiff.is(0, b"II");
    let u16_at = |at| if little { tiff.le16(at) } else { tiff.be16(at) };
    let u32_at = |at| if little { tiff.le32(at) } else { tiff.be32(at) };
    let directory = usize::try_from(u32_at(4)?).ok()?;
    let entries = usize::try_from(u16_at(directory)?).ok()?;
    (0..entries).find_map(|i| {
        let entry = directory + 2 + i * 12;
        (u16_at(entry)? == 0x0112)
            .then(|| u16_at(entry + 8))
            .flatten()
    })
}

fn webp(b: Bytes) -> Option<Probed> {
    if b.len() < 30 || !b.is(0, b"RIFF") || !b.is(8, b"WEBP") {
        return None;
    }
    if b.is(12, b"VP8 ") {
        sized(
            "image/webp",
            b.le16(26)? & 0x3fff,
            b.le16(28)? & 0x3fff,
            false,
        )
    } else if b.is(12, b"VP8L") {
        let bits = b.le32(21)?;
        sized(
            "image/webp",
            (bits & 0x3fff) + 1,
            ((bits >> 14) & 0x3fff) + 1,
            false,
        )
    } else if b.is(12, b"VP8X") {
        sized("image/webp", b.le24(24)? + 1, b.le24(27)? + 1, false)
    } else {
        None
    }
}

/// The brands that say an ISO base media file is HEIC.
const HEIF_BRANDS: [&[u8]; 5] = [b"heic", b"heix", b"heim", b"heis", b"mif1"];

/// HEIC and AVIF: an ISO base media file, its brand saying which, its size in the largest
/// image spatial extent (`ispe`) box, which is the primary image's, and its rotation in `irot`.
fn heif(b: Bytes) -> Option<Probed> {
    if b.len() < 16 || !b.is(4, b"ftyp") {
        return None;
    }
    let ftyp = usize::try_from(b.be32(0)?).ok()?;
    let brands: Vec<&[u8]> = std::iter::once(8)
        .chain((16..ftyp.min(b.len()).saturating_sub(3)).step_by(4))
        .filter_map(|at| b.0.get(at..at + 4))
        .collect();
    let mime = if brands
        .iter()
        .any(|&brand| brand == b"avif" || brand == b"avis")
    {
        "image/avif"
    } else if brands.iter().any(|brand| HEIF_BRANDS.contains(brand)) {
        "image/heic"
    } else {
        return None;
    };
    let (mut width, mut height) = (0u32, 0u32);
    let mut from = 0;
    while let Some(at) = b.find(b"ispe", from) {
        if at + 16 > b.len() {
            break;
        }
        let (w, h) = (b.be32(at + 8)?, b.be32(at + 12)?);
        if u64::from(w) * u64::from(h) > u64::from(width) * u64::from(height) {
            (width, height) = (w, h);
        }
        from = at + 4;
    }
    let turned = b
        .find(b"irot", 0)
        .and_then(|at| b.at(at + 4))
        .is_some_and(|angle| (angle & 0x3) % 2 == 1);
    sized(mime, width, height, turned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    /// A JPEG's start: SOI, maybe an EXIF APP1 with an orientation, then a baseline SOF0.
    fn jpeg_of(width: u16, height: u16, orientation: Option<u16>) -> Vec<u8> {
        let mut parts: Vec<Vec<u8>> = vec![vec![0xff, 0xd8]];
        if let Some(o) = orientation {
            let tiff = bytes(&[
                b"MM",
                &[0, 42],
                &8u32.to_be_bytes(),
                &1u16.to_be_bytes(),
                &0x0112u16.to_be_bytes(),
                &3u16.to_be_bytes(),
                &1u32.to_be_bytes(),
                &o.to_be_bytes(),
                &[0, 0],
                &0u32.to_be_bytes(),
            ]);
            let app1 = bytes(&[b"Exif\0\0", &tiff]);
            let length = u16::try_from(app1.len() + 2).unwrap();
            parts.push(bytes(&[&[0xff, 0xe1], &length.to_be_bytes(), &app1]));
        }
        parts.push(bytes(&[
            &[0xff, 0xc0],
            &17u16.to_be_bytes(),
            &[8],
            &height.to_be_bytes(),
            &width.to_be_bytes(),
            &[3],
            &[0; 9],
        ]));
        parts.concat()
    }

    fn of(mime: &str, width: u32, height: u32) -> Probed {
        Probed {
            mime: mime.into(),
            width,
            height,
        }
    }

    #[test]
    fn reads_each_kind_s_size_turned_as_its_camera_says() {
        let png = bytes(&[
            b"\x89PNG\r\n\x1a\n",
            &13u32.to_be_bytes(),
            b"IHDR",
            &640u32.to_be_bytes(),
            &480u32.to_be_bytes(),
            &[8, 6, 0, 0, 0],
        ]);
        assert_eq!(probe(&png), Some(of("image/png", 640, 480)));
        assert_eq!(
            probe(b"GIF89a\x40\x01\xf0\x00"),
            Some(of("image/gif", 320, 240))
        );
        assert_eq!(
            probe(&jpeg_of(4032, 3024, None)),
            Some(of("image/jpeg", 4032, 3024))
        );
        assert_eq!(
            probe(&jpeg_of(4032, 3024, Some(1))),
            Some(of("image/jpeg", 4032, 3024))
        );
        assert_eq!(
            probe(&jpeg_of(4032, 3024, Some(6))),
            Some(of("image/jpeg", 3024, 4032))
        );
        let webp = bytes(&[
            b"RIFF",
            &[0; 4],
            b"WEBPVP8X",
            &[10, 0, 0, 0],
            &[0; 4],
            &[0xff, 0x04, 0x00],
            &[0x7f, 0x02, 0x00],
        ]);
        assert_eq!(probe(&webp), Some(of("image/webp", 1280, 640)));
        let ispe = |w: u32, h: u32| {
            bytes(&[
                &20u32.to_be_bytes(),
                b"ispe",
                &[0; 4],
                &w.to_be_bytes(),
                &h.to_be_bytes(),
            ])
        };
        let heic = bytes(&[
            &24u32.to_be_bytes(),
            b"ftypheic",
            &[0; 4],
            b"mif1heic",
            &ispe(512, 512),
            &ispe(4032, 3024),
            &9u32.to_be_bytes(),
            b"irot",
            &[1],
        ]);
        assert_eq!(probe(&heic), Some(of("image/heic", 3024, 4032)));
    }

    #[test]
    fn knows_nothing_of_what_isnt_an_image_or_is_cut_short() {
        assert_eq!(probe(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
        assert_eq!(probe(b"%PDF-1.7"), None);
        assert_eq!(probe(&jpeg_of(10, 10, None)[..8]), None);
        assert_eq!(probe(&[]), None);
    }
}
