//! Exact compressed geometry pool; never-referenced blobs stay packed.
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex, OnceLock},
};
pub struct PackedBlob {
    length: usize,
    prefix: [u8; 8],
    encoded: Mutex<Option<Box<[u8]>>>,
    ready: OnceLock<Vec<u8>>,
}
impl std::fmt::Debug for PackedBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackedBlob")
            .field("length", &self.length)
            .finish()
    }
}
impl PackedBlob {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Arc<Self>, Vec<u8>> {
        if bytes.len() < 256 {
            return Err(bytes);
        }
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&bytes).expect("Vec writer");
        let packed = encoder.finish().expect("Vec writer");
        if packed.len() + std::mem::size_of::<Self>() + 32 >= bytes.len() {
            return Err(bytes);
        }
        let mut prefix = [0; 8];
        prefix.copy_from_slice(&bytes[..8]);
        Ok(Arc::new(Self {
            length: bytes.len(),
            prefix,
            encoded: Mutex::new(Some(packed.into_boxed_slice())),
            ready: OnceLock::new(),
        }))
    }
    fn unpack(&self, packed: Box<[u8]>) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.length);
        flate2::read::DeflateDecoder::new(&packed[..])
            .read_to_end(&mut bytes)
            .expect("immutable bytes encoded in-process");
        assert_eq!(bytes.len(), self.length);
        bytes
    }
    pub fn get(&self) -> &[u8] {
        self.ready.get_or_init(|| {
            let packed = self
                .encoded
                .lock()
                .unwrap()
                .take()
                .expect("unique initializer");
            self.unpack(packed)
        })
    }
    pub fn image(&self) -> bool {
        self.length > 8
            && matches!(
                &self.prefix[..2],
                [0x89, 0x50] | [0xff, 0xd8] | [0x47, 0x49] | [0x52, 0x49]
            )
    }
    pub fn into_vec(value: Arc<Self>) -> Vec<u8> {
        match Arc::try_unwrap(value) {
            Ok(mut own) => {
                if let Some(bytes) = own.ready.take() {
                    bytes
                } else {
                    let packed = own.encoded.get_mut().unwrap().take().unwrap();
                    own.unpack(packed)
                }
            }
            Err(shared) => shared.get().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figma_types::BlobOrString;

    #[test]
    fn packed_blob_exact_clone_threaded_reads_and_backing_release() {
        let bytes: Vec<_> = (0..16_384).map(|n| (n % 37) as u8).collect();
        let packed = PackedBlob::from_bytes(bytes.clone()).unwrap();
        assert!(packed.encoded.lock().unwrap().as_ref().unwrap().len() < bytes.len() / 2);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let packed = packed.clone();
                std::thread::spawn(move || packed.get().to_vec())
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), bytes);
        }
        assert!(packed.encoded.lock().unwrap().is_none());
        assert_eq!(
            BlobOrString::Packed(packed.clone()),
            BlobOrString::Bytes(bytes.clone())
        );
        let mut owned = PackedBlob::into_vec(packed.clone());
        owned[0] ^= 1;
        assert_eq!(
            packed.get(),
            bytes,
            "shared immutable bytes keep independent owned results"
        );
        drop(packed.clone());
        assert_eq!(PackedBlob::into_vec(packed), bytes);
        fn bounds<T: Send + Sync>() {}
        bounds::<PackedBlob>();
    }

    #[test]
    fn packed_blob_image_prefix_matches_old_magic_without_inflating_geometry() {
        for prefix in [
            [0x89, 0x50],
            [0xff, 0xd8],
            [0x47, 0x49],
            [0x52, 0x49],
            [0, 0],
        ] {
            let mut bytes = vec![0; 8192];
            bytes[..2].copy_from_slice(&prefix);
            let packed = PackedBlob::from_bytes(bytes.clone()).unwrap();
            let geometry = prefix == [0, 0];
            assert_eq!(
                BlobOrString::Packed(packed.clone()).into_image(),
                (!geometry).then_some(bytes)
            );
            if geometry {
                assert!(packed.ready.get().is_none());
                assert!(packed.encoded.lock().unwrap().is_some());
            }
        }
        assert!(matches!(
            BlobOrString::Bytes(vec![1, 2, 3]).packed(),
            BlobOrString::Bytes(_)
        ));
        assert_eq!(
            BlobOrString::Str("unknown".into()).packed(),
            BlobOrString::Str("unknown".into())
        );
    }
}
