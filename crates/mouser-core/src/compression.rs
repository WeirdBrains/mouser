use serde::{Deserialize, Serialize};
use thiserror::Error;
use std::io;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompressionAlgo {
    Zstd,
    Lz4,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    pub algorithm: CompressionAlgo,
    pub level: i32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgo::Zstd,
            level: 3,
        }
    }
}

#[derive(Debug, Error)]
pub enum CompressionError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("lz4 error")]
    Lz4,
}

pub fn compress_bytes(cfg: &CompressionConfig, input: &[u8]) -> Result<Vec<u8>, CompressionError> {
    match cfg.algorithm {
        CompressionAlgo::None => Ok(input.to_vec()),
        CompressionAlgo::Zstd => {
            let cursor = io::Cursor::new(input);
            let out = zstd::stream::encode_all(cursor, cfg.level)?;
            Ok(out)
        }
        CompressionAlgo::Lz4 => {
            let compressed = lz4_flex::block::compress_prepend_size(input);
            Ok(compressed)
        }
    }
}

pub fn decompress_bytes(cfg: &CompressionConfig, input: &[u8]) -> Result<Vec<u8>, CompressionError> {
    match cfg.algorithm {
        CompressionAlgo::None => Ok(input.to_vec()),
        CompressionAlgo::Zstd => {
            let cursor = io::Cursor::new(input);
            let out = zstd::stream::decode_all(cursor)?;
            Ok(out)
        }
        CompressionAlgo::Lz4 => {
            let decompressed = lz4_flex::block::decompress_size_prepended(input)
                .map_err(|_| CompressionError::Lz4)?;
            Ok(decompressed)
        }
    }
}

