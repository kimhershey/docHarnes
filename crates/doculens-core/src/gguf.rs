//! GGUF container parsing (design §3.3.4 / §3.3.5): header, metadata and
//! tensor descriptors are read without touching the tensor data, so a 3–8 GB
//! file is inspected in milliseconds. Also provides full-file hashing
//! (SHA-256 + BLAKE3 in one pass) and a cheap fingerprint for change
//! detection in the drop-in model folder.
//!
//! Format reference: ggml `docs/gguf.md` (versions 2 and 3; version 1 is
//! not supported by llama.cpp any more and is rejected here as well).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::error::{CoreError, Result};
use crate::memory::ModelArch;

/// "GGUF" as a little-endian u32.
pub const GGUF_MAGIC: u32 = 0x4655_4747;
pub const DEFAULT_ALIGNMENT: u32 = 32;

// Sanity bounds so a corrupt header cannot make us allocate without limit.
const MAX_KV: u64 = 1 << 20;
const MAX_TENSORS: u64 = 1 << 20;
const MAX_STRING: u64 = 64 << 20;
const MAX_ARRAY: u64 = 1 << 28;
const MAX_DIMS: u32 = 8;
/// Nested arrays deeper than this are rejected (stack-overflow guard).
const MAX_ARRAY_DEPTH: u32 = 4;
/// Arrays longer than this (vocabularies, merges, token types) are skipped
/// and summarised as [`GgufValue::LargeArray`].
pub const INLINE_ARRAY_LIMIT: u64 = 4096;

/// Tensor name suffixes whose precision matters for Gated-DeltaNet models
/// (design §3.3.4 / appendix B-1). They are expected to stay F32/F16/BF16/Q8_0.
pub const GDN_SENSITIVE_SUFFIXES: [&str; 6] = ["ssm_alpha", "ssm_beta", "ssm_a", "ssm_dt", "ssm_norm", "ssm_conv1d"];

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GgufValue {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    F32(f32),
    Bool(bool),
    String(String),
    Array(Vec<GgufValue>),
    /// Array that was not materialised (longer than [`INLINE_ARRAY_LIMIT`]).
    LargeArray {
        element_type: u32,
        len: u64,
    },
    U64(u64),
    I64(i64),
    F64(f64),
}

impl GgufValue {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            GgufValue::U8(v) => Some(u64::from(*v)),
            GgufValue::U16(v) => Some(u64::from(*v)),
            GgufValue::U32(v) => Some(u64::from(*v)),
            GgufValue::U64(v) => Some(*v),
            GgufValue::I8(v) if *v >= 0 => Some(*v as u64),
            GgufValue::I16(v) if *v >= 0 => Some(*v as u64),
            GgufValue::I32(v) if *v >= 0 => Some(*v as u64),
            GgufValue::I64(v) if *v >= 0 => Some(*v as u64),
            _ => None,
        }
    }
    pub fn as_u32(&self) -> Option<u32> {
        self.as_u64().and_then(|v| u32::try_from(v).ok())
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            GgufValue::F32(v) => Some(f64::from(*v)),
            GgufValue::F64(v) => Some(*v),
            other => other.as_u64().map(|v| v as f64),
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            GgufValue::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            GgufValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[GgufValue]> {
        match self {
            GgufValue::Array(a) => Some(a),
            _ => None,
        }
    }
    /// A scalar, or the largest element of a (short) numeric array — used for
    /// per-layer keys such as `attention.head_count_kv` on hybrid models.
    pub fn as_u32_or_max_of_array(&self) -> Option<u32> {
        if let Some(a) = self.as_array() {
            return a.iter().filter_map(GgufValue::as_u32).max();
        }
        self.as_u32()
    }

    fn type_id(&self) -> u32 {
        match self {
            GgufValue::U8(_) => 0,
            GgufValue::I8(_) => 1,
            GgufValue::U16(_) => 2,
            GgufValue::I16(_) => 3,
            GgufValue::U32(_) => 4,
            GgufValue::I32(_) => 5,
            GgufValue::F32(_) => 6,
            GgufValue::Bool(_) => 7,
            GgufValue::String(_) => 8,
            GgufValue::Array(_) | GgufValue::LargeArray { .. } => 9,
            GgufValue::U64(_) => 10,
            GgufValue::I64(_) => 11,
            GgufValue::F64(_) => 12,
        }
    }
}

// ---------------------------------------------------------------------------
// Tensors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TensorInfo {
    pub name: String,
    pub dims: Vec<u64>,
    pub ggml_type: u32,
    pub offset: u64,
}

impl TensorInfo {
    pub fn type_name(&self) -> &'static str {
        ggml_type_name(self.ggml_type)
    }
    /// Element count (saturating: crafted dimensions must not overflow).
    pub fn n_elements(&self) -> u64 {
        self.dims.iter().try_fold(1u64, |acc, d| acc.checked_mul(*d)).unwrap_or(u64::MAX)
    }
}

/// `ggml_type` enum → name (ggml.h).
pub fn ggml_type_name(t: u32) -> &'static str {
    match t {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        6 => "Q5_0",
        7 => "Q5_1",
        8 => "Q8_0",
        9 => "Q8_1",
        10 => "Q2_K",
        11 => "Q3_K",
        12 => "Q4_K",
        13 => "Q5_K",
        14 => "Q6_K",
        15 => "Q8_K",
        16 => "IQ2_XXS",
        17 => "IQ2_XS",
        18 => "IQ3_XXS",
        19 => "IQ1_S",
        20 => "IQ4_NL",
        21 => "IQ3_S",
        22 => "IQ2_S",
        23 => "IQ4_XS",
        24 => "I8",
        25 => "I16",
        26 => "I32",
        27 => "I64",
        28 => "F64",
        29 => "IQ1_M",
        30 => "BF16",
        34 => "TQ1_0",
        35 => "TQ2_0",
        39 => "MXFP4",
        40 => "NVFP4",
        41 => "Q1_0",
        42 => "Q2_0",
        43 => "F8_E4M3",
        _ => "UNKNOWN",
    }
}

/// True for the types considered "full precision enough" for GDN state tensors.
pub fn is_high_precision_type(t: u32) -> bool {
    matches!(t, 0 | 1 | 8 | 28 | 30 | 26)
}

/// `general.file_type` (llama_ftype) → quantisation label, e.g. 15 → "Q4_K_M".
pub fn file_type_name(ft: u32) -> Option<&'static str> {
    Some(match ft {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4_MOE",
        39 => "NVFP4",
        40 => "Q1_0",
        41 => "Q2_0",
        42 => "F8_E4M3",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GgufHeader {
    pub version: u32,
    pub alignment: u32,
    pub metadata: BTreeMap<String, GgufValue>,
    pub tensors: Vec<TensorInfo>,
    /// Byte offset where tensor data starts (aligned).
    pub data_offset: u64,
    pub file_size: u64,
}

impl GgufHeader {
    pub fn get(&self, key: &str) -> Option<&GgufValue> {
        self.metadata.get(key)
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(GgufValue::as_str)
    }
    pub fn get_u32(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(GgufValue::as_u32_or_max_of_array)
    }
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(GgufValue::as_u64)
    }
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(GgufValue::as_bool)
    }
    pub fn architecture(&self) -> Option<&str> {
        self.get_str("general.architecture")
    }
    /// `<architecture>.<suffix>` lookup.
    pub fn arch_key(&self, suffix: &str) -> Option<&GgufValue> {
        let arch = self.architecture()?;
        self.get(&format!("{arch}.{suffix}"))
    }
    pub fn arch_u32(&self, suffix: &str) -> Option<u32> {
        self.arch_key(suffix).and_then(GgufValue::as_u32_or_max_of_array)
    }
}

struct Cursor<R: Read> {
    inner: R,
    pos: u64,
}

impl<R: Read> Cursor<R> {
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.inner.read_exact(buf)?;
        self.pos += buf.len() as u64;
        Ok(())
    }
    fn u8(&mut self) -> io::Result<u8> {
        let mut b = [0u8; 1];
        self.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        let mut b = [0u8; 2];
        self.read_exact(&mut b)?;
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&mut self) -> io::Result<u32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> io::Result<u64> {
        let mut b = [0u8; 8];
        self.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn skip(&mut self, n: u64) -> io::Result<()> {
        let mut remaining = n;
        let mut buf = [0u8; 8192];
        while remaining > 0 {
            let take = remaining.min(buf.len() as u64) as usize;
            self.read_exact(&mut buf[..take])?;
            remaining -= take as u64;
        }
        Ok(())
    }
}

fn bad(path: &Path, reason: impl Into<String>) -> CoreError {
    CoreError::gguf(path, reason)
}

fn scalar_size(ty: u32) -> Option<u64> {
    Some(match ty {
        0..=1 | 7 => 1,
        2..=3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        _ => return None,
    })
}

fn read_string<R: Read>(c: &mut Cursor<R>, path: &Path, what: &str) -> Result<String> {
    let len = c.u64().map_err(|e| bad(path, format!("{what}: {e}")))?;
    if len > MAX_STRING {
        return Err(bad(path, format!("{what}: string length {len} exceeds limit")));
    }
    let mut buf = vec![0u8; len as usize];
    c.read_exact(&mut buf).map_err(|e| bad(path, format!("{what}: {e}")))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_value<R: Read>(c: &mut Cursor<R>, ty: u32, path: &Path, key: &str, depth: u32) -> Result<GgufValue> {
    let io_err = |e: io::Error| bad(path, format!("key `{key}`: {e}"));
    if depth > MAX_ARRAY_DEPTH {
        return Err(bad(path, format!("key `{key}`: arrays nested deeper than {MAX_ARRAY_DEPTH}")));
    }
    Ok(match ty {
        0 => GgufValue::U8(c.u8().map_err(io_err)?),
        1 => GgufValue::I8(c.u8().map_err(io_err)? as i8),
        2 => GgufValue::U16(c.u16().map_err(io_err)?),
        3 => GgufValue::I16(c.u16().map_err(io_err)? as i16),
        4 => GgufValue::U32(c.u32().map_err(io_err)?),
        5 => GgufValue::I32(c.u32().map_err(io_err)? as i32),
        6 => GgufValue::F32(f32::from_bits(c.u32().map_err(io_err)?)),
        7 => GgufValue::Bool(c.u8().map_err(io_err)? != 0),
        8 => GgufValue::String(read_string(c, path, key)?),
        9 => {
            let elem_ty = c.u32().map_err(io_err)?;
            let len = c.u64().map_err(io_err)?;
            if len > MAX_ARRAY {
                return Err(bad(path, format!("key `{key}`: array length {len} exceeds limit")));
            }
            if len > INLINE_ARRAY_LIMIT {
                // Skip without materialising. Strings and nested arrays have to be walked.
                match scalar_size(elem_ty) {
                    Some(sz) => c.skip(len * sz).map_err(io_err)?,
                    None => {
                        for _ in 0..len {
                            read_value(c, elem_ty, path, key, depth + 1)?;
                        }
                    }
                }
                GgufValue::LargeArray { element_type: elem_ty, len }
            } else {
                let mut items = Vec::with_capacity(len as usize);
                for _ in 0..len {
                    items.push(read_value(c, elem_ty, path, key, depth + 1)?);
                }
                GgufValue::Array(items)
            }
        }
        10 => GgufValue::U64(c.u64().map_err(io_err)?),
        11 => GgufValue::I64(c.u64().map_err(io_err)? as i64),
        12 => GgufValue::F64(f64::from_bits(c.u64().map_err(io_err)?)),
        other => return Err(bad(path, format!("key `{key}`: unknown value type {other}"))),
    })
}

/// Reads the GGUF header (metadata + tensor infos). Tensor data is never read.
pub fn read_header(path: &Path) -> Result<GgufHeader> {
    let file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let file_size = file.metadata().map_err(|e| CoreError::io(path, e))?.len();
    let mut c = Cursor { inner: BufReader::with_capacity(1 << 16, file), pos: 0 };

    let magic = c.u32().map_err(|e| bad(path, format!("magic: {e}")))?;
    if magic != GGUF_MAGIC {
        return Err(bad(path, "not a GGUF file (bad magic)"));
    }
    let version = c.u32().map_err(|e| bad(path, format!("version: {e}")))?;
    if !(2..=3).contains(&version) {
        return Err(bad(path, format!("unsupported GGUF version {version}")));
    }
    let n_tensors = c.u64().map_err(|e| bad(path, format!("tensor count: {e}")))?;
    let n_kv = c.u64().map_err(|e| bad(path, format!("kv count: {e}")))?;
    if n_tensors > MAX_TENSORS || n_kv > MAX_KV {
        return Err(bad(path, format!("implausible counts (tensors {n_tensors}, kv {n_kv})")));
    }

    let mut metadata = BTreeMap::new();
    for _ in 0..n_kv {
        let key = read_string(&mut c, path, "metadata key")?;
        let ty = c.u32().map_err(|e| bad(path, format!("key `{key}` type: {e}")))?;
        let value = read_value(&mut c, ty, path, &key, 0)?;
        metadata.insert(key, value);
    }

    let alignment = metadata.get("general.alignment").and_then(GgufValue::as_u32).unwrap_or(DEFAULT_ALIGNMENT);
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(bad(path, format!("invalid general.alignment {alignment}")));
    }

    let mut tensors = Vec::with_capacity(n_tensors.min(1 << 16) as usize);
    for i in 0..n_tensors {
        let name = read_string(&mut c, path, "tensor name")?;
        let n_dims = c.u32().map_err(|e| bad(path, format!("tensor {i} dims: {e}")))?;
        if n_dims > MAX_DIMS {
            return Err(bad(path, format!("tensor `{name}`: {n_dims} dimensions")));
        }
        let mut dims = Vec::with_capacity(n_dims as usize);
        for _ in 0..n_dims {
            dims.push(c.u64().map_err(|e| bad(path, format!("tensor `{name}`: {e}")))?);
        }
        let ggml_type = c.u32().map_err(|e| bad(path, format!("tensor `{name}` type: {e}")))?;
        let offset = c.u64().map_err(|e| bad(path, format!("tensor `{name}` offset: {e}")))?;
        tensors.push(TensorInfo { name, dims, ggml_type, offset });
    }

    let header_end = c.pos;
    let align = u64::from(alignment);
    let data_offset = header_end.div_ceil(align) * align;
    if data_offset > file_size {
        return Err(bad(path, "file truncated before tensor data"));
    }
    // Every tensor must end inside the file (a half-copied GGUF has a complete header).
    let data_end = tensors.iter().map(|t| t.offset.saturating_add(tensor_bytes(t))).max().unwrap_or(0);
    if data_offset.saturating_add(data_end) > file_size {
        return Err(bad(
            path,
            format!(
                "truncated: tensor data needs {} bytes, file has {file_size}",
                data_offset.saturating_add(data_end)
            ),
        ));
    }
    Ok(GgufHeader { version, alignment, metadata, tensors, data_offset, file_size })
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GgufKind {
    /// A language model (text or the LLM half of a VLM).
    Llm,
    /// Multimodal projector (`general.architecture = "clip"`).
    Mmproj,
    #[default]
    Unknown,
}

/// What the app needs to know about a GGUF without loading it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelMeta {
    pub kind: GgufKind,
    pub architecture: String,
    pub name: Option<String>,
    pub file_type: Option<u32>,
    /// Quantisation label (`general.file_type`, else the dominant tensor type).
    pub quantization: Option<String>,
    pub license: Option<String>,
    pub license_link: Option<String>,
    pub source_url: Option<String>,
    pub source_repo: Option<String>,
    pub has_chat_template: bool,
    pub block_count: Option<u32>,
    pub context_length: Option<u32>,
    pub embedding_length: Option<u32>,
    pub head_count: Option<u32>,
    pub head_count_kv: Option<u32>,
    pub key_length: Option<u32>,
    pub full_attention_interval: Option<u32>,
    pub split_count: Option<u16>,
    pub split_no: Option<u16>,
    pub projector_type: Option<String>,
    pub has_vision_encoder: Option<bool>,
    pub has_audio_encoder: Option<bool>,
    pub tensor_count: u64,
    /// ggml type name → number of tensors.
    pub tensor_types: BTreeMap<String, u64>,
    /// GDN-sensitive tensor suffix → set of ggml types found (joined by ",").
    pub gdn_tensor_types: BTreeMap<String, String>,
    /// GDN-sensitive suffixes stored below Q8_0 precision (warning).
    pub gdn_low_precision: Vec<String>,
    pub parameter_count: Option<u64>,
    pub data_bytes: u64,
}

impl ModelMeta {
    pub fn is_hybrid(&self) -> bool {
        self.full_attention_interval.is_some_and(|i| i > 1)
    }
    /// Hyper-parameters for the VRAM estimate (`memory::estimate`).
    pub fn arch_params(&self) -> Option<ModelArch> {
        let n_layer = self.block_count?;
        let n_head_kv = self.head_count_kv.or(self.head_count)?;
        let head_dim = match (self.key_length, self.embedding_length, self.head_count) {
            (Some(k), _, _) => k,
            (None, Some(e), Some(h)) if h > 0 => e / h,
            _ => return None,
        };
        Some(ModelArch {
            n_layer,
            full_attention_interval: self.full_attention_interval.unwrap_or(1).max(1),
            n_head_kv,
            head_dim,
        })
    }
}

/// Builds the summary from a parsed header.
pub fn summarize(h: &GgufHeader) -> ModelMeta {
    let arch = h.architecture().unwrap_or("").to_string();
    let kind = match arch.as_str() {
        "" => GgufKind::Unknown,
        "clip" => GgufKind::Mmproj,
        _ => GgufKind::Llm,
    };
    let mut tensor_types: BTreeMap<String, u64> = BTreeMap::new();
    let mut gdn: BTreeMap<String, std::collections::BTreeSet<&'static str>> = BTreeMap::new();
    let mut gdn_low: std::collections::BTreeSet<String> = Default::default();
    let mut data_bytes = 0u64;
    for t in &h.tensors {
        *tensor_types.entry(t.type_name().to_string()).or_default() += 1;
        if let Some(suffix) = gdn_suffix(&t.name) {
            gdn.entry(suffix.to_string()).or_default().insert(t.type_name());
            if !is_high_precision_type(t.ggml_type) {
                gdn_low.insert(suffix.to_string());
            }
        }
        data_bytes = data_bytes.max(t.offset.saturating_add(tensor_bytes(t)));
    }
    let file_type = h.get_u32("general.file_type");
    let quantization =
        file_type.and_then(file_type_name).map(str::to_string).or_else(|| dominant_weight_type(&h.tensors));
    let has_chat_template = h.get_str("tokenizer.chat_template").is_some_and(|t| !t.trim().is_empty());

    ModelMeta {
        kind,
        architecture: arch,
        name: h.get_str("general.name").map(str::to_string),
        file_type,
        quantization,
        license: h.get_str("general.license").map(str::to_string),
        license_link: h.get_str("general.license.link").map(str::to_string),
        source_url: h.get_str("general.source.url").map(str::to_string),
        source_repo: h.get_str("general.source.huggingface.repository").map(str::to_string),
        has_chat_template,
        block_count: h.arch_u32("block_count"),
        context_length: h.arch_u32("context_length"),
        embedding_length: h.arch_u32("embedding_length"),
        head_count: h.arch_u32("attention.head_count"),
        head_count_kv: h.arch_u32("attention.head_count_kv"),
        key_length: h.arch_u32("attention.key_length"),
        full_attention_interval: h.arch_u32("full_attention_interval"),
        split_count: h.get_u32("split.count").and_then(|v| u16::try_from(v).ok()),
        split_no: h.get_u32("split.no").and_then(|v| u16::try_from(v).ok()),
        projector_type: h.get_str("clip.projector_type").map(str::to_string),
        has_vision_encoder: h.get_bool("clip.has_vision_encoder"),
        has_audio_encoder: h.get_bool("clip.has_audio_encoder"),
        tensor_count: h.tensors.len() as u64,
        tensor_types,
        gdn_tensor_types: gdn.into_iter().map(|(k, v)| (k, v.into_iter().collect::<Vec<_>>().join(","))).collect(),
        gdn_low_precision: gdn_low.into_iter().collect(),
        parameter_count: h.get_u64("general.parameter_count"),
        data_bytes,
    }
}

fn gdn_suffix(name: &str) -> Option<&'static str> {
    if !name.starts_with("blk.") {
        return None;
    }
    let last = name.rsplit('.').nth(if name.ends_with(".weight") || name.ends_with(".bias") { 1 } else { 0 })?;
    GDN_SENSITIVE_SUFFIXES.iter().copied().find(|s| *s == last)
}

/// Block size / type size for the common ggml types (bytes per block, elements per block).
fn type_traits(t: u32) -> Option<(u64, u64)> {
    Some(match t {
        0 => (4, 1),
        1 | 30 => (2, 1),
        28 => (8, 1),
        24 => (1, 1),
        25 => (2, 1),
        26 => (4, 1),
        27 => (8, 1),
        2 => (18, 32),
        3 => (20, 32),
        6 => (22, 32),
        7 => (24, 32),
        8 => (34, 32),
        9 => (36, 32),
        10 => (84, 256),
        11 => (110, 256),
        12 => (144, 256),
        13 => (176, 256),
        14 => (210, 256),
        15 => (292, 256),
        16 => (66, 256),
        17 => (74, 256),
        18 => (98, 256),
        19 => (50, 256),
        20 => (18, 32),
        21 => (110, 256),
        22 => (82, 256),
        23 => (136, 256),
        29 => (56, 256),
        39 => (17, 32),
        _ => return None,
    })
}

fn tensor_bytes(t: &TensorInfo) -> u64 {
    match type_traits(t.ggml_type) {
        Some((block_bytes, block_elems)) => t.n_elements().div_ceil(block_elems).saturating_mul(block_bytes),
        None => 0,
    }
}

fn dominant_weight_type(tensors: &[TensorInfo]) -> Option<String> {
    let mut by_type: BTreeMap<u32, u64> = BTreeMap::new();
    for t in tensors {
        if t.dims.len() >= 2 {
            *by_type.entry(t.ggml_type).or_default() += t.n_elements();
        }
    }
    by_type.into_iter().max_by_key(|(_, n)| *n).map(|(ty, _)| ggml_type_name(ty).to_string())
}

/// Convenience: header + summary in one call.
pub fn inspect(path: &Path) -> Result<ModelMeta> {
    Ok(summarize(&read_header(path)?))
}

/// `name-00001-of-00003.gguf` → (1, 3).
pub fn split_index_from_name(name: &str) -> Option<(u16, u16)> {
    let stem = name.strip_suffix(".gguf")?;
    let (_, tail) = stem.rsplit_once('-')?;
    let total: u16 = tail.parse().ok()?;
    let head = &stem[..stem.len() - tail.len() - 1];
    let head = head.strip_suffix("-of")?;
    let (_, idx) = head.rsplit_once('-')?;
    let idx: u16 = idx.parse().ok()?;
    (idx >= 1 && idx <= total).then_some((idx, total))
}

/// True for `mmproj*.gguf` (the router pairs projectors by this prefix).
pub fn is_mmproj_name(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with("mmproj") && name.to_ascii_lowercase().ends_with(".gguf")
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHashes {
    pub size: u64,
    pub sha256: String,
    pub blake3: String,
}

/// Hashes the whole file (SHA-256 and BLAKE3 in one pass). `progress(done,
/// total)` is called every few MiB; returning `false` cancels (→ `Ok(None)`).
pub fn hash_file(path: &Path, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<Option<FileHashes>> {
    let file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let total = file.metadata().map_err(|e| CoreError::io(path, e))?.len();
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut sha = sha2::Sha256::new();
    let mut b3 = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let mut since_report = 0u64;
    if !progress(0, total) {
        return Ok(None);
    }
    loop {
        let n = reader.read(&mut buf).map_err(|e| CoreError::io(path, e))?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        b3.update(&buf[..n]);
        done += n as u64;
        since_report += n as u64;
        if since_report >= 8 << 20 {
            since_report = 0;
            if !progress(done, total) {
                return Ok(None);
            }
        }
    }
    progress(done, total);
    Ok(Some(FileHashes { size: done, sha256: hex(&sha.finalize()), blake3: b3.finalize().to_hex().to_string() }))
}

/// SHA-256 only (used after downloads).
pub fn sha256_file(path: &Path, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<Option<String>> {
    Ok(hash_file(path, progress)?.map(|h| h.sha256))
}

/// Cheap identity for change detection: BLAKE3 over size, first and last
/// MiB. A re-quantised replacement with the same name changes it; touching
/// mtime alone does not.
pub fn quick_fingerprint(path: &Path) -> Result<String> {
    const WINDOW: u64 = 1 << 20;
    let mut file = File::open(path).map_err(|e| CoreError::io(path, e))?;
    let size = file.metadata().map_err(|e| CoreError::io(path, e))?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut buf = vec![0u8; WINDOW.min(size) as usize];
    file.read_exact(&mut buf).map_err(|e| CoreError::io(path, e))?;
    hasher.update(&buf);
    if size > WINDOW {
        file.seek(SeekFrom::Start(size - WINDOW)).map_err(|e| CoreError::io(path, e))?;
        let mut tail = vec![0u8; WINDOW as usize];
        file.read_exact(&mut tail).map_err(|e| CoreError::io(path, e))?;
        hasher.update(&tail);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Minimal writer (tests, fixtures, tooling)
// ---------------------------------------------------------------------------

/// Writes a syntactically valid GGUF v3 file with the given metadata and
/// zero-filled tensors. Used by tests and by the fixture generator; it is not
/// a general-purpose converter.
pub fn write_minimal_gguf(path: &Path, metadata: &[(&str, GgufValue)], tensors: &[(&str, &[u64], u32)]) -> Result<()> {
    fn put_string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    fn put_value(out: &mut Vec<u8>, v: &GgufValue) {
        match v {
            GgufValue::U8(x) => out.push(*x),
            GgufValue::I8(x) => out.push(*x as u8),
            GgufValue::U16(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::I16(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::U32(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::I32(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::F32(x) => out.extend_from_slice(&x.to_bits().to_le_bytes()),
            GgufValue::Bool(x) => out.push(u8::from(*x)),
            GgufValue::String(s) => put_string(out, s),
            GgufValue::Array(items) => {
                let ty = items.first().map(GgufValue::type_id).unwrap_or(4);
                out.extend_from_slice(&ty.to_le_bytes());
                out.extend_from_slice(&(items.len() as u64).to_le_bytes());
                for it in items {
                    put_value(out, it);
                }
            }
            GgufValue::LargeArray { element_type, .. } => {
                out.extend_from_slice(&element_type.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
            }
            GgufValue::U64(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::I64(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::F64(x) => out.extend_from_slice(&x.to_bits().to_le_bytes()),
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
    out.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    for (k, v) in metadata {
        put_string(&mut out, k);
        out.extend_from_slice(&v.type_id().to_le_bytes());
        put_value(&mut out, v);
    }
    let mut offset = 0u64;
    let mut sizes = Vec::new();
    for (name, dims, ty) in tensors {
        put_string(&mut out, name);
        out.extend_from_slice(&(dims.len() as u32).to_le_bytes());
        for d in *dims {
            out.extend_from_slice(&d.to_le_bytes());
        }
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        let info = TensorInfo { name: name.to_string(), dims: dims.to_vec(), ggml_type: *ty, offset };
        let bytes = tensor_bytes(&info).max(1);
        let padded = bytes.div_ceil(u64::from(DEFAULT_ALIGNMENT)) * u64::from(DEFAULT_ALIGNMENT);
        sizes.push(padded);
        offset += padded;
    }
    let align = u64::from(DEFAULT_ALIGNMENT);
    let data_start = (out.len() as u64).div_ceil(align) * align;
    out.resize(data_start as usize, 0);
    let total: u64 = sizes.iter().sum();
    out.resize((data_start + total) as usize, 0);
    let mut f = File::create(path).map_err(|e| CoreError::io(path, e))?;
    f.write_all(&out).map_err(|e| CoreError::io(path, e))?;
    Ok(())
}

/// Metadata for a synthetic Qwen3.5-style text model (tests / fixtures).
pub fn synthetic_qwen35_metadata(
    name: &str,
    file_type: u32,
    with_chat_template: bool,
) -> Vec<(&'static str, GgufValue)> {
    let mut m = vec![
        ("general.architecture", GgufValue::String("qwen35".into())),
        ("general.name", GgufValue::String(name.to_string())),
        ("general.file_type", GgufValue::U32(file_type)),
        ("general.license", GgufValue::String("apache-2.0".into())),
        ("general.license.link", GgufValue::String("https://huggingface.co/Qwen/Qwen3.5-4B".into())),
        ("qwen35.block_count", GgufValue::U32(32)),
        ("qwen35.context_length", GgufValue::U32(262_144)),
        ("qwen35.embedding_length", GgufValue::U32(2560)),
        ("qwen35.attention.head_count", GgufValue::U32(16)),
        ("qwen35.attention.head_count_kv", GgufValue::U32(4)),
        ("qwen35.attention.key_length", GgufValue::U32(256)),
        ("qwen35.full_attention_interval", GgufValue::U32(4)),
    ];
    if with_chat_template {
        m.push((
            "tokenizer.chat_template",
            GgufValue::String("{% for m in messages %}{{ m.content }}{% endfor %}".into()),
        ));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn roundtrip_header_and_summary() {
        let dir = tmp();
        let p = dir.path().join("m.gguf");
        let meta = synthetic_qwen35_metadata("Qwen3.5 4B test", 15, true);
        let tensors: Vec<(&str, &[u64], u32)> = vec![
            ("token_embd.weight", &[2560, 1000], 12),
            ("blk.0.attn_q.weight", &[2560, 4096], 12),
            ("blk.0.ssm_alpha.weight", &[2560, 32], 0),
            ("blk.0.ssm_a", &[32], 0),
            ("blk.1.ssm_beta.weight", &[2560, 32], 12), // low precision on purpose
            ("output_norm.weight", &[2560], 0),
        ];
        write_minimal_gguf(&p, &meta, &tensors).unwrap();
        let h = read_header(&p).unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.tensors.len(), 6);
        assert_eq!(h.architecture(), Some("qwen35"));
        assert_eq!(h.data_offset % 32, 0);
        let s = summarize(&h);
        assert_eq!(s.kind, GgufKind::Llm);
        assert_eq!(s.quantization.as_deref(), Some("Q4_K_M"));
        assert!(s.has_chat_template);
        assert_eq!(s.block_count, Some(32));
        assert_eq!(s.full_attention_interval, Some(4));
        assert!(s.is_hybrid());
        let arch = s.arch_params().unwrap();
        assert_eq!(arch.n_head_kv, 4);
        assert_eq!(arch.head_dim, 256);
        assert_eq!(arch.n_full_attention_layers(), 8);
        assert_eq!(s.gdn_tensor_types.get("ssm_alpha").map(String::as_str), Some("F32"));
        assert_eq!(s.gdn_low_precision, vec!["ssm_beta".to_string()]);
        assert_eq!(s.tensor_types.get("Q4_K"), Some(&3));
        assert!(s.data_bytes > 0);
    }

    #[test]
    fn mmproj_and_split_detection() {
        let dir = tmp();
        let p = dir.path().join("mmproj-F16.gguf");
        let meta = vec![
            ("general.architecture", GgufValue::String("clip".into())),
            ("clip.projector_type", GgufValue::String("qwen3vl_merger".into())),
            ("clip.has_vision_encoder", GgufValue::Bool(true)),
            ("split.count", GgufValue::U16(0)),
        ];
        write_minimal_gguf(&p, &meta, &[("v.patch_embd.weight", &[3, 14, 14, 1024], 1)]).unwrap();
        let s = inspect(&p).unwrap();
        assert_eq!(s.kind, GgufKind::Mmproj);
        assert_eq!(s.projector_type.as_deref(), Some("qwen3vl_merger"));
        assert_eq!(s.has_vision_encoder, Some(true));
        assert_eq!(s.quantization.as_deref(), Some("F16"));
        assert!(is_mmproj_name("mmproj-F16.gguf"));
        assert!(is_mmproj_name("MMPROJ-Qwen3-VL-4B-Instruct-F16.gguf"));
        assert!(!is_mmproj_name("Qwen3.5-4B-Q6_K.gguf"));
        assert_eq!(split_index_from_name("Qwen3.5-27B-Q4_K_M-00002-of-00003.gguf"), Some((2, 3)));
        assert_eq!(split_index_from_name("Qwen3.5-4B-Q6_K.gguf"), None);
        assert_eq!(split_index_from_name("x-00004-of-00003.gguf"), None);
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        let dir = tmp();
        let p = dir.path().join("bad.gguf");
        std::fs::write(&p, b"not a gguf file at all").unwrap();
        let e = read_header(&p).unwrap_err();
        assert_eq!(e.code(), "invalid_gguf");

        let good = dir.path().join("good.gguf");
        write_minimal_gguf(&good, &synthetic_qwen35_metadata("t", 1, false), &[("a", &[8], 0)]).unwrap();
        let bytes = std::fs::read(&good).unwrap();
        let trunc = dir.path().join("trunc.gguf");
        std::fs::write(&trunc, &bytes[..40]).unwrap();
        assert_eq!(read_header(&trunc).unwrap_err().code(), "invalid_gguf");

        // absurd kv count
        let mut evil = bytes.clone();
        evil[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        std::fs::write(&trunc, &evil).unwrap();
        assert!(read_header(&trunc).is_err());

        // complete header, tensor data cut off (half-copied file)
        let h = read_header(&good).unwrap();
        std::fs::write(&trunc, &bytes[..(h.data_offset as usize + 4)]).unwrap();
        let e = read_header(&trunc).unwrap_err();
        assert!(e.to_string().contains("truncated"), "{e}");

        // deeply nested arrays are rejected instead of overflowing the stack
        let mut nested = GgufValue::U8(1);
        for _ in 0..64 {
            nested = GgufValue::Array(vec![nested]);
        }
        let deep = dir.path().join("deep.gguf");
        write_minimal_gguf(&deep, &[("general.architecture", GgufValue::String("x".into())), ("evil", nested)], &[])
            .unwrap();
        assert!(read_header(&deep).unwrap_err().to_string().contains("nested"));
    }

    #[test]
    fn large_arrays_are_skipped_not_materialised() {
        let dir = tmp();
        let p = dir.path().join("vocab.gguf");
        let tokens: Vec<GgufValue> = (0..5000).map(|i| GgufValue::String(format!("tok{i}"))).collect();
        let mut meta = synthetic_qwen35_metadata("v", 1, false);
        meta.push(("tokenizer.ggml.tokens", GgufValue::Array(tokens)));
        meta.push((
            "qwen35.rope.dimension_sections",
            GgufValue::Array(vec![GgufValue::U32(11), GgufValue::U32(11), GgufValue::U32(10), GgufValue::U32(0)]),
        ));
        write_minimal_gguf(&p, &meta, &[("a", &[8], 0)]).unwrap();
        let h = read_header(&p).unwrap();
        assert!(matches!(h.get("tokenizer.ggml.tokens"), Some(GgufValue::LargeArray { len: 5000, element_type: 8 })));
        assert_eq!(h.get("qwen35.rope.dimension_sections").unwrap().as_array().unwrap().len(), 4);
        // per-layer arrays resolve to their max
        assert_eq!(h.get("qwen35.rope.dimension_sections").unwrap().as_u32_or_max_of_array(), Some(11));
    }

    #[test]
    fn hashes_and_fingerprint() {
        let dir = tmp();
        let p = dir.path().join("h.bin");
        std::fs::write(&p, b"hello world").unwrap();
        let mut calls = 0;
        let h = hash_file(&p, &mut |_, _| {
            calls += 1;
            true
        })
        .unwrap()
        .unwrap();
        assert_eq!(h.sha256, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        assert_eq!(h.size, 11);
        assert_eq!(h.blake3.len(), 64);
        assert!(calls >= 2);
        let cancelled = hash_file(&p, &mut |_, _| false).unwrap();
        assert!(cancelled.is_none());
        let fp1 = quick_fingerprint(&p).unwrap();
        std::fs::write(&p, b"hello worle").unwrap();
        assert_ne!(fp1, quick_fingerprint(&p).unwrap());
    }
}
