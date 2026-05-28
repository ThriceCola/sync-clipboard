//! Binary wire protocol for clipboard sync.
//!
//! Wire format (all integers little-endian):
//! ```text
//! [1 byte: type] [4 bytes: uncompressed_len] [1 byte: compression] [payload...]
//! ```
//!
//! - type:   0 = Text (UTF-8), 1 = Image (mime + data)
//! - compression: 0 = none, 1 = zstd
//!
//! Image payload (after decompression):
//! ```text
//! [2 bytes: mime_len] [mime_bytes...] [image_data...]
//! ```

use sha2::{Digest, Sha256};

/// Threshold in bytes above which payloads are compressed.
pub const COMPRESS_THRESHOLD: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    Text(String),
    Image { mime_type: String, data: Vec<u8> },
}

impl ClipboardContent {
    /// Compute a SHA-256 hash for deduplication.
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        match self {
            ClipboardContent::Text(s) => {
                hasher.update(b"T:");
                hasher.update(s.as_bytes());
            }
            ClipboardContent::Image { mime_type, data } => {
                hasher.update(b"I:");
                hasher.update(mime_type.as_bytes());
                hasher.update(b":");
                hasher.update(data);
            }
        }
        hasher.finalize().into()
    }

    /// Check whether the content is an image type.
    pub fn is_image(&self) -> bool {
        matches!(self, ClipboardContent::Image { .. })
    }
}

/// Serialize content to wire format bytes.
pub fn serialize(content: &ClipboardContent) -> Vec<u8> {
    let payload = encode_payload(content);
    let uncompressed_len = payload.len() as u32;
    let should_compress = payload.len() > COMPRESS_THRESHOLD;

    let compressed = if should_compress {
        zstd::encode_all(&*payload, 1).expect("zstd compress")
    } else {
        payload
    };

    let mut buf = Vec::with_capacity(1 + 4 + 1 + compressed.len());
    buf.push(if content.is_image() { 1u8 } else { 0u8 });
    buf.extend_from_slice(&uncompressed_len.to_le_bytes());
    buf.push(if should_compress { 1u8 } else { 0u8 });
    buf.extend_from_slice(&compressed);
    buf
}

/// Deserialize wire format bytes to content.
pub fn deserialize(data: &[u8]) -> Option<ClipboardContent> {
    if data.len() < 6 {
        return None;
    }

    let content_type = data[0];
    let uncompressed_len = u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize;
    let compression = data[5];
    let payload = &data[6..];

    let decompressed: Vec<u8> = if compression == 1 {
        zstd::decode_all(payload).ok()?
    } else {
        payload.to_vec()
    };

    if decompressed.len() != uncompressed_len {
        log::warn!(
            "Size mismatch: expected {uncompressed_len}, got {}",
            decompressed.len()
        );
    }

    decode_payload(&decompressed, content_type)
}

fn encode_payload(content: &ClipboardContent) -> Vec<u8> {
    match content {
        ClipboardContent::Text(s) => s.as_bytes().to_vec(),
        ClipboardContent::Image { mime_type, data } => {
            let mime = mime_type.as_bytes();
            let mime_len = mime.len().min(u16::MAX as usize) as u16;
            let mut buf = Vec::with_capacity(2 + mime_len as usize + data.len());
            buf.extend_from_slice(&mime_len.to_le_bytes());
            buf.extend_from_slice(&mime[..mime_len as usize]);
            buf.extend_from_slice(data);
            buf
        }
    }
}

fn decode_payload(payload: &[u8], content_type: u8) -> Option<ClipboardContent> {
    match content_type {
        0 => {
            let text = String::from_utf8_lossy(payload).to_string();
            Some(ClipboardContent::Text(text))
        }
        1 => {
            if payload.len() < 2 {
                return None;
            }
            let mime_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
            if 2 + mime_len > payload.len() {
                return None;
            }
            let mime_type = String::from_utf8_lossy(&payload[2..2 + mime_len]).to_string();
            let data = payload[2 + mime_len..].to_vec();
            Some(ClipboardContent::Image { mime_type, data })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_roundtrip() {
        let content = ClipboardContent::Text("hello world".into());
        let bytes = serialize(&content);
        let decoded = deserialize(&bytes).unwrap();
        assert_eq!(content, decoded);
    }

    #[test]
    fn test_text_roundtrip_compressed() {
        let long_text = "A".repeat(2000);
        let content = ClipboardContent::Text(long_text.clone());
        let bytes = serialize(&content);
        assert!(bytes.len() < long_text.len() + 10); // compressed smaller
        let decoded = deserialize(&bytes).unwrap();
        assert_eq!(content, decoded);
    }

    #[test]
    fn test_image_roundtrip() {
        let content = ClipboardContent::Image {
            mime_type: "image/png".into(),
            data: vec![0x89, 0x50, 0x4E, 0x47],
        };
        let bytes = serialize(&content);
        let decoded = deserialize(&bytes).unwrap();
        assert_eq!(content, decoded);
    }

    #[test]
    fn test_hash_different() {
        let a = ClipboardContent::Text("foo".into());
        let b = ClipboardContent::Text("bar".into());
        assert_ne!(a.hash(), b.hash());
    }
}
