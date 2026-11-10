//! VRAM budgeting for GGUF models on llama.cpp (design §2.4, §3.3.3).
//!
//! For hybrid models (Qwen3.5: 3× Gated-DeltaNet : 1× full attention) the KV
//! cache exists only on the full-attention layers; counting every layer
//! overestimates by 4× (the Ollama #19463 bug).

use serde::{Deserialize, Serialize};

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// Compute-buffer allowance added to every estimate.
pub const COMPUTE_BUFFER_BYTES: u64 = 512 * MIB;
/// Safety headroom subtracted from free VRAM before loading.
pub const HEADROOM_BYTES: u64 = GIB;
/// Approximate Gated-DeltaNet recurrent state per linear layer per slot
/// (measured on Qwen3.5-9B; 4B scales with d_inner).
pub const GDN_STATE_PER_LAYER_PER_SLOT: u64 = 2 * MIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KvCacheType {
    F16,
    Q8_0,
}

impl KvCacheType {
    pub fn bytes_per_element(self) -> u64 {
        match self {
            KvCacheType::F16 => 2,
            KvCacheType::Q8_0 => 1,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "f16" => Some(KvCacheType::F16),
            "q8_0" | "q8" => Some(KvCacheType::Q8_0),
            _ => None,
        }
    }
}

/// Gemma-style sliding-window attention layout: most layers keep a bounded KV
/// window, a few global layers keep the whole context, and (Gemma 4) the last
/// layers share the KV of the last layer of their kind and hold none themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwaLayout {
    /// Window size in tokens (`attention.sliding_window`).
    pub window: u32,
    /// Sliding-window layers with their own KV.
    pub n_swa_layers: u32,
    /// Global (full-attention) layers with their own KV.
    pub n_global_layers: u32,
    /// KV heads / head size of the sliding-window layers (`head_count_kv[swa]`, `key_length_swa`).
    pub n_head_kv_swa: u32,
    pub head_dim_swa: u32,
}

/// Architecture hyper-parameters needed for memory estimation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelArch {
    pub n_layer: u32,
    /// 1 for standard transformers; 4 for Qwen3.5 (every 4th layer is full attention).
    pub full_attention_interval: u32,
    /// KV heads / head size of the full-attention (global) layers.
    pub n_head_kv: u32,
    pub head_dim: u32,
    /// Gemma 3/4 sliding-window layout (`None` for standard transformers and GDN hybrids).
    pub swa: Option<SwaLayout>,
}

impl ModelArch {
    /// Gemma 4 E4B: 42 layers, the last 18 share KV → 24 own-KV layers in a 5:1 pattern
    /// (20 sliding-window × 2 KV heads × 256, 4 global × 1 KV head × 512), window 512.
    pub const GEMMA4_E4B: ModelArch = ModelArch {
        n_layer: 42,
        full_attention_interval: 1,
        n_head_kv: 1,
        head_dim: 512,
        swa: Some(SwaLayout { window: 512, n_swa_layers: 20, n_global_layers: 4, n_head_kv_swa: 2, head_dim_swa: 256 }),
    };
    /// Gemma 4 E2B: 35 layers, 20 shared → 15 own-KV layers in a 4:1 pattern (12 + 3), 1 KV head.
    pub const GEMMA4_E2B: ModelArch = ModelArch {
        n_layer: 35,
        full_attention_interval: 1,
        n_head_kv: 1,
        head_dim: 512,
        swa: Some(SwaLayout { window: 512, n_swa_layers: 12, n_global_layers: 3, n_head_kv_swa: 1, head_dim_swa: 256 }),
    };
    /// Gemma 4 12B (unified): 48 layers in a 5:1 pattern (40 sliding × 8 KV heads × 256,
    /// 8 global × 1 KV head × 512), window 1,024 — no shared layers assumed (pinned on the user PC).
    pub const GEMMA4_12B: ModelArch = ModelArch {
        n_layer: 48,
        full_attention_interval: 1,
        n_head_kv: 1,
        head_dim: 512,
        swa: Some(SwaLayout { window: 1024, n_swa_layers: 40, n_global_layers: 8, n_head_kv_swa: 8, head_dim_swa: 256 }),
    };
    /// Qwen3.5-4B / 9B drop-ins: 32 layers, interval 4 → 8 full-attention layers,
    /// 4 KV heads × head_dim 256.
    pub const QWEN35_4B: ModelArch =
        ModelArch { n_layer: 32, full_attention_interval: 4, n_head_kv: 4, head_dim: 256, swa: None };
    /// Qwen3-VL-4B drop-in (standard GQA transformer): 36 layers, 8 KV heads × 128.
    pub const QWEN3_VL_4B: ModelArch =
        ModelArch { n_layer: 36, full_attention_interval: 1, n_head_kv: 8, head_dim: 128, swa: None };

    /// Layers whose KV covers the whole context (own KV only).
    pub fn n_full_attention_layers(&self) -> u32 {
        match self.swa {
            Some(s) => s.n_global_layers,
            None => (self.n_layer / self.full_attention_interval.max(1)).max(1),
        }
    }

    /// Linear-attention (GDN) layers of a Qwen3.5-style hybrid (0 otherwise).
    pub fn n_linear_layers(&self) -> u32 {
        if self.swa.is_some() || self.full_attention_interval <= 1 {
            0
        } else {
            self.n_layer - self.n_full_attention_layers()
        }
    }

    /// KV-cache bytes for `ctx` tokens (one slot, or the unified pool).
    pub fn kv_bytes(&self, ctx: u64, kv: KvCacheType) -> u64 {
        let bpe = kv.bytes_per_element();
        let global = ctx * u64::from(self.n_full_attention_layers()) * u64::from(self.n_head_kv) * u64::from(self.head_dim) * 2 * bpe;
        let sliding = match self.swa {
            Some(s) => {
                ctx.min(u64::from(s.window))
                    * u64::from(s.n_swa_layers)
                    * u64::from(s.n_head_kv_swa)
                    * u64::from(s.head_dim_swa)
                    * 2
                    * bpe
            }
            None => 0,
        };
        global + sliding
    }

    /// Recurrent-state bytes for hybrid models (0 for standard transformers).
    pub fn gdn_state_bytes(&self, slots: u32) -> u64 {
        if self.n_linear_layers() == 0 {
            0
        } else {
            GDN_STATE_PER_LAYER_PER_SLOT * u64::from(self.n_linear_layers()) * u64::from(slots)
        }
    }
}

/// Inputs for a pre-load estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadPlan {
    pub weights_bytes: u64,
    /// Part of `weights_bytes` llama.cpp keeps on the host (Gemma 4's per-layer embedding
    /// table is read lazily from the file, `TENSOR_READ_LAZY`) — not VRAM.
    pub host_bytes: u64,
    pub mmproj_bytes: u64,
    pub ctx_tokens: u64,
    pub kv_type: KvCacheType,
    pub slots: u32,
    pub arch: ModelArch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEstimate {
    pub weights_bytes: u64,
    pub mmproj_bytes: u64,
    pub kv_bytes: u64,
    pub gdn_bytes: u64,
    pub compute_bytes: u64,
    pub headroom_bytes: u64,
    /// Everything above summed.
    pub required_bytes: u64,
}

impl MemoryEstimate {
    pub fn required_mib(&self) -> u64 {
        self.required_bytes.div_ceil(MIB)
    }
    /// True when the model fits into `free_bytes` of VRAM.
    pub fn fits(&self, free_bytes: u64) -> bool {
        free_bytes >= self.required_bytes
    }
}

/// Pre-load estimate (design §3.3.3): GPU-resident weights + mmproj + KV + GDN + 512 MiB + 1 GiB.
pub fn estimate(plan: &LoadPlan) -> MemoryEstimate {
    let kv_bytes = plan.arch.kv_bytes(plan.ctx_tokens, plan.kv_type);
    let gdn_bytes = plan.arch.gdn_state_bytes(plan.slots);
    let gpu_weights = plan.weights_bytes.saturating_sub(plan.host_bytes);
    let required_bytes = gpu_weights + plan.mmproj_bytes + kv_bytes + gdn_bytes + COMPUTE_BUFFER_BYTES + HEADROOM_BYTES;
    MemoryEstimate {
        weights_bytes: gpu_weights,
        mmproj_bytes: plan.mmproj_bytes,
        kv_bytes,
        gdn_bytes,
        compute_bytes: COMPUTE_BUFFER_BYTES,
        headroom_bytes: HEADROOM_BYTES,
        required_bytes,
    }
}

/// Largest power-of-two context (≥ 4096) that fits the budget, or `None`.
pub fn max_context_for_budget(plan: &LoadPlan, free_bytes: u64) -> Option<u64> {
    let mut ctx = plan.ctx_tokens.max(4096);
    while ctx >= 4096 {
        let e = estimate(&LoadPlan { ctx_tokens: ctx, ..*plan });
        if e.fits(free_bytes) {
            return Some(ctx);
        }
        ctx /= 2;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwen35_kv_matches_design_table() {
        let a = ModelArch::QWEN35_4B;
        assert_eq!(a.n_full_attention_layers(), 8);
        assert_eq!(a.n_linear_layers(), 24);
        // 32K ctx, q8_0 -> 512 MiB (design: ≈0.53 GiB incl. rounding)
        assert_eq!(a.kv_bytes(32_768, KvCacheType::Q8_0), 512 * MIB);
        // 8K ctx, F16 -> 256 MiB (design: 0.25 GiB)
        assert_eq!(a.kv_bytes(8_192, KvCacheType::F16), 256 * MIB);
        // 32 KiB/token F16, 16 KiB/token q8_0
        assert_eq!(a.kv_bytes(1, KvCacheType::F16), 32 * 1024);
        assert_eq!(a.gdn_state_bytes(2), 96 * MIB);
    }

    #[test]
    fn gemma4_sliding_window_kv_is_small_and_ple_stays_on_the_host() {
        let a = ModelArch::GEMMA4_E4B;
        assert_eq!(a.n_full_attention_layers(), 4);
        assert_eq!(a.n_linear_layers(), 0);
        assert_eq!(a.gdn_state_bytes(2), 0);
        // 32K ctx, q8_0: 4 global layers × 1 head × 512 → 128 MiB; 20 sliding layers × 512 tokens × 2 × 256 → 10 MiB
        assert_eq!(a.kv_bytes(32_768, KvCacheType::Q8_0), 128 * MIB + 10 * MIB);
        // a short context never pays for more than the window on the sliding layers
        assert_eq!(a.kv_bytes(256, KvCacheType::F16), 256 * 4 * 512 * 2 * 2 + 256 * 20 * 2 * 256 * 2 * 2);
        let e2b = ModelArch::GEMMA4_E2B;
        assert_eq!(e2b.kv_bytes(8_192, KvCacheType::Q8_0), 8_192 * 3 * 512 * 2 + 512 * 12 * 256 * 2);
        // E4B Q4_K_M (4.98 GB file, ≈1.6 GB per-layer embedding table on the host) + F16 mmproj at 32K
        let plan = LoadPlan {
            weights_bytes: 4_750 * MIB,
            host_bytes: 1_525 * MIB,
            mmproj_bytes: 944 * MIB,
            ctx_tokens: 32_768,
            kv_type: KvCacheType::Q8_0,
            slots: 1,
            arch: a,
        };
        let e = estimate(&plan);
        assert_eq!(e.weights_bytes, (4_750 - 1_525) * MIB);
        assert_eq!(e.required_mib(), 3_225 + 944 + 138 + 512 + 1_024);
        assert!(e.fits(6_000 * MIB));
        assert!(!e.fits(5_700 * MIB));
    }

    #[test]
    fn standard_transformer_counts_all_layers_and_no_gdn() {
        let a = ModelArch::QWEN3_VL_4B;
        assert_eq!(a.n_full_attention_layers(), 36);
        // 36 × 8 × 128 × 2 × 2 = 147,456 B/token = 144 KiB
        assert_eq!(a.kv_bytes(1, KvCacheType::F16), 144 * 1024);
        assert_eq!(a.gdn_state_bytes(2), 0);
    }

    #[test]
    fn estimate_for_8gb_card() {
        let plan = LoadPlan {
            weights_bytes: 3_400 * MIB,
            host_bytes: 0,
            mmproj_bytes: 660 * MIB,
            ctx_tokens: 32_768,
            kv_type: KvCacheType::Q8_0,
            slots: 2,
            arch: ModelArch::QWEN35_4B,
        };
        let e = estimate(&plan);
        assert_eq!(e.required_mib(), 3_400 + 660 + 512 + 96 + 512 + 1_024);
        assert!(e.fits(7_126 * MIB));
        assert!(!e.fits(6_000 * MIB));
        assert_eq!(max_context_for_budget(&plan, 6_000 * MIB), Some(16_384));
        assert_eq!(max_context_for_budget(&plan, 5_850 * MIB), Some(8_192));
        assert_eq!(max_context_for_budget(&plan, 5_800 * MIB), Some(4_096));
        assert_eq!(max_context_for_budget(&plan, 1_000 * MIB), None);
    }
}
