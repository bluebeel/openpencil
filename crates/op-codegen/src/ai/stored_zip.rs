//! Minimal STORED-only `.zip` encoder — pure std, zero deps.
//!
//! The web host downloads the AI structure bundle as a `.zip`
//! (TS `structure-bundle.ts` encodes with `uzip`); the desktop host
//! uses the `zip` crate, but pulling that crate into the wasm build
//! would cost bundle bytes against the 1 MiB gzip ceiling for a
//! single write-only, no-compression use. Entries are STORED
//! (method 0) — the bundle is JSON + already-compressed images, and
//! the artifact is gzipped by the transport anyway.
//!
//! Layout per APPNOTE.TXT: [local header + data]* then the central
//! directory and the end-of-central-directory record. No zip64 —
//! bundles are far below the 4 GiB / 65535-entry thresholds.

/// IEEE CRC-32 (reflected, poly 0xEDB88320) — the zip checksum.
const fn crc32_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ (0xEDB8_8320 & (value & 1).wrapping_neg());
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}
const CRC32: [u32; 256] = crc32_table();
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF;
    for &byte in bytes {
        crc = (crc >> 8) ^ CRC32[((crc ^ u32::from(byte)) & 0xff) as usize];
    }
    !crc
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Encode `files` as a STORED zip archive. Entry order is preserved.
pub fn build_stored_zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    // Local header30 + central header46 + two UTF-8 names + raw bytes,
    // followed by EOCD22. A single final allocation avoids growing/copying
    // the full image archive repeatedly. Overflow keeps the legacy grow path.
    let capacity = files
        .iter()
        .try_fold(22usize, |total, (name, data)| {
            total
                .checked_add(76)?
                .checked_add(name.len().checked_mul(2)?)?
                .checked_add(data.len())
        })
        .unwrap_or(0);
    let mut out = Vec::with_capacity(capacity);
    // (name, crc, size, local-header offset) for the central directory.
    let mut entries: Vec<(&str, u16, u32, u32, u32)> = Vec::with_capacity(files.len());

    for (name, data) in files {
        let flags = if name.is_ascii() { 0 } else { 1 << 11 };
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        push_u32(&mut out, 0x0403_4B50); // local file header signature
        push_u16(&mut out, 20); // version needed: 2.0
        push_u16(&mut out, flags); // UTF-8 filename when needed
        push_u16(&mut out, 0); // method: STORED
        push_u16(&mut out, 0); // mod time
        push_u16(&mut out, 0); // mod date
        push_u32(&mut out, crc);
        push_u32(&mut out, size); // compressed size == size (STORED)
        push_u32(&mut out, size);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0); // extra len
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        entries.push((name, flags, crc, size, offset));
    }

    let cd_offset = out.len() as u32;
    for (name, flags, crc, size, offset) in &entries {
        push_u32(&mut out, 0x0201_4B50); // central directory signature
        push_u16(&mut out, 20); // version made by
        push_u16(&mut out, 20); // version needed
        push_u16(&mut out, *flags); // same UTF-8 filename flag
        push_u16(&mut out, 0); // method
        push_u16(&mut out, 0); // mod time
        push_u16(&mut out, 0); // mod date
        push_u32(&mut out, *crc);
        push_u32(&mut out, *size);
        push_u32(&mut out, *size);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0); // extra len
        push_u16(&mut out, 0); // comment len
        push_u16(&mut out, 0); // disk number
        push_u16(&mut out, 0); // internal attrs
        push_u32(&mut out, 0); // external attrs
        push_u32(&mut out, *offset);
        out.extend_from_slice(name.as_bytes());
    }
    let cd_size = out.len() as u32 - cd_offset;

    push_u32(&mut out, 0x0605_4B50); // end of central directory
    push_u16(&mut out, 0); // disk number
    push_u16(&mut out, 0); // cd start disk
    push_u16(&mut out, entries.len() as u16);
    push_u16(&mut out, entries.len() as u16);
    push_u32(&mut out, cd_size);
    push_u32(&mut out, cd_offset);
    push_u16(&mut out, 0); // comment len
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vectors() {
        // Reference values from the IEEE CRC-32 ("123456789" check).
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn round_trips_through_the_zip_crate() {
        let files = vec![
            ("manifest.json".to_string(), br#"{"k":1}"#.to_vec()),
            ("assets/a.png".to_string(), vec![0x89, 0x50, 0x4E, 0x47]),
            ("views/raw.json".to_string(), b"[]".to_vec()),
        ];
        let bytes = build_stored_zip(&files);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
        assert_eq!(archive.len(), 3);
        for (name, data) in &files {
            use std::io::Read;
            let mut entry = archive.by_name(name).expect("entry present");
            let mut got = Vec::new();
            entry.read_to_end(&mut got).expect("read");
            assert_eq!(&got, data, "{name} round-trips");
        }
    }

    #[test]
    fn empty_archive_is_a_bare_eocd() {
        let bytes = build_stored_zip(&[]);
        assert_eq!(bytes.len(), 22, "EOCD record only");
        assert_eq!(&bytes[0..4], &[0x50, 0x4B, 0x05, 0x06]);
    }
}
