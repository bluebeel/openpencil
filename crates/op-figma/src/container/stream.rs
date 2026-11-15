//! Bounded import reader; codec selection preserves the eager fallback chain.
use super::*;
use std::io::{Cursor, Read};
#[derive(Clone, Copy)]
enum Codec {
    Raw,
    Deflate,
    Zstd,
}
pub(crate) struct Chunk<'a> {
    bytes: &'a [u8],
    codec: Codec,
    pub len: usize,
}
impl<'a> Chunk<'a> {
    pub fn reader(&self) -> Box<dyn Read + 'a> {
        match self.codec {
            Codec::Raw => Box::new(Cursor::new(self.bytes)),
            Codec::Deflate => Box::new(flate2::read::DeflateDecoder::new(self.bytes)),
            Codec::Zstd => Box::new(
                ruzstd::decoding::StreamingDecoder::new(self.bytes)
                    .expect("validated immutable zstd"),
            ),
        }
    }
}
fn validate(bytes: &[u8], codec: Codec) -> Option<usize> {
    let reader: Box<dyn Read> = match codec {
        Codec::Raw => return Some(bytes.len()),
        Codec::Deflate => Box::new(flate2::read::DeflateDecoder::new(bytes)),
        Codec::Zstd => Box::new(ruzstd::decoding::StreamingDecoder::new(bytes).ok()?),
    };
    let count = std::io::copy(
        &mut reader.take(MAX_CHUNK_SIZE as u64 + 1),
        &mut std::io::sink(),
    )
    .ok()?;
    (count <= MAX_CHUNK_SIZE as u64).then_some(count as usize)
}
pub(crate) fn chunk(bytes: &[u8]) -> Chunk<'_> {
    let chosen = if is_png(bytes) {
        Some((Codec::Raw, bytes.len()))
    } else {
        (is_zstd(bytes)
            .then(|| validate(bytes, Codec::Zstd))
            .flatten()
            .map(|n| (Codec::Zstd, n)))
        .or_else(|| validate(bytes, Codec::Deflate).map(|n| (Codec::Deflate, n)))
        .or_else(|| validate(bytes, Codec::Zstd).map(|n| (Codec::Zstd, n)))
    };
    let (codec, len) = chosen.unwrap_or((Codec::Raw, bytes.len()));
    Chunk { bytes, codec, len }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn bounded_codec_selection_matches_eager_fallback_and_truncation() {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(b"unknown fields survive this exact payload")
            .unwrap();
        let deflate = encoder.finish().unwrap();
        let fixtures: &[&[u8]] = &[
            &deflate,
            &deflate[..deflate.len() / 2],
            b"uncompressed source bytes",
            b"\x89PNGsample",
            b"\x28\xb5\x2f\xfd\x20\x05\x29\x00\x00hello",
            b"\x28\xb5\x2f\xfd\x20\x05\x2b\x00\x00a",
            b"\x28\xb5\x2f\xfd\x20\x05\x29\x00\x00he",
            b"",
        ];
        for bytes in fixtures {
            let expected = decompress_chunk(bytes);
            let selected = chunk(bytes);
            assert_eq!(selected.len, expected.len());
            let mut actual = Vec::new();
            selected.reader().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, expected);
        }
    }
}
