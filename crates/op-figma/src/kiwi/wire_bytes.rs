//! Immutable lossless backing for already-validated deferred ranges.
use std::{
    borrow::Cow,
    io::{Read, Write},
    sync::Arc,
};
#[derive(Debug)]
pub(crate) struct WireBytes {
    bytes: Box<[u8]>,
    decoded: Option<usize>,
}
impl WireBytes {
    pub fn capture(raw: &[u8]) -> Arc<Self> {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(raw).expect("Vec writer");
        let packed = encoder.finish().expect("Vec writer");
        let (decoded, bytes) = if packed.len() + 64 < raw.len() {
            (Some(raw.len()), packed.into_boxed_slice())
        } else {
            (None, raw.into())
        };
        Arc::new(Self { bytes, decoded })
    }
    pub fn read(&self) -> Cow<'_, [u8]> {
        match self.decoded {
            None => Cow::Borrowed(&self.bytes),
            Some(length) => {
                let mut out = Vec::with_capacity(length);
                flate2::read::DeflateDecoder::new(&self.bytes[..])
                    .read_to_end(&mut out)
                    .expect("immutable range compressed in-process");
                assert_eq!(out.len(), length);
                Cow::Owned(out)
            }
        }
    }
    pub fn retained(&self) -> usize {
        std::mem::size_of::<Self>() + 16 + self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_wire_exact_unknown_bytes_and_copy_on_write() {
        let raw: Vec<_> = (0..8192).map(|i| (i % 17) as u8).collect();
        let packed = WireBytes::capture(&raw);
        assert!(packed.retained() < raw.len());
        let mut owned = packed.read();
        assert_eq!(&*owned, raw);
        owned.to_mut()[0] ^= 1;
        assert_eq!(&*packed.read(), raw);
        let shared = packed.clone();
        assert_eq!(&*shared.read(), raw);
        for small in [vec![], vec![0xff], vec![0x80; 16]] {
            let value = WireBytes::capture(&small);
            assert_eq!(&*value.read(), small);
        }
        fn bounds<T: Send + Sync>() {}
        bounds::<WireBytes>();
    }
}
