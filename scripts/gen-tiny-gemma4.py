#!/usr/bin/env python
"""Generate a tiny randomly-initialised Gemma 4 VLM as GGUF (text model + mmproj)
without torch / transformers: the tensors are written directly with `gguf`
(PyPI, from llama.cpp) and the vocabulary is a small SPM-style BPE trained with
`tokenizers`. Output quality is irrelevant; the files exist to exercise the
DocuLens engine path (chat template, thinking off, JSON grammar, image input)
with the architecture the registry ships since 0.4.0 (design §4.2).

Requirements: `pip install gguf tokenizers numpy` (Apache-2.0 / MIT, no torch).

Layout (override the root with DOCULENS_SCRATCH):
  <root>/tiny-gemma4/models/tiny-gemma4/{tiny-gemma4-F16.gguf, mmproj-F16.gguf, LICENSE}
Copy the last folder to crates/doculens-core/tests/fixtures/ to refresh the committed fixture.

Shapes follow llama.cpp b12066 (`src/models/gemma4.cpp`, `tools/mtmd/models/gemma4v.cpp`,
`conversion/gemma.py`): a dense Gemma 4 (no per-layer embeddings, no MoE) with the
sliding-window / global attention layout, "proportional" RoPE frequencies for the
global layers, and a GEMMA4V projector (ViT → 3×3 average pooling → RMS norm → linear).
"""
import json
import os
import pathlib
import sys

import numpy as np

ROOT = pathlib.Path(
    os.environ.get("DOCULENS_SCRATCH", pathlib.Path(__file__).resolve().parent.parent / "target" / "tiny-gemma4-work")
)
OUT = ROOT / "tiny-gemma4"

# --- text model hyper-parameters ------------------------------------------------
N_LAYER = 4
SWA_PATTERN = [True, True, True, False]  # 3 sliding-window layers + 1 global layer (Gemma 4: 5:1 / 4:1)
N_EMBD = 64
N_FF = 128
N_HEAD = 2
N_HEAD_KV = 1
HEAD_DIM_SWA = 32
HEAD_DIM_FULL = 64
SLIDING_WINDOW = 64
CTX = 4096
ROPE_BASE_FULL = 1_000_000.0
ROPE_BASE_SWA = 10_000.0
PARTIAL_ROTARY_FULL = 0.25  # "proportional" rope on global layers (p-RoPE)
RMS_EPS = 1e-6

# --- vision tower (GEMMA4V) ------------------------------------------------------
V_EMBD = 32
V_FF = 64
V_LAYER = 2
V_HEAD = 2
PATCH = 16
POOL = 3  # pooling_kernel_size → one token per 48×48 px
POS_SIZE = 128  # position tables: up to 128 patches per side (2,048 px)

SPECIALS = [
    "<pad>", "<eos>", "<bos>", "<unk>",
    "<|turn>", "<turn|>", "<|image>", "<image|>", "<|image|>", "<|audio|>", "<|video|>",
    "<|think|>", "<|channel>", "<channel|>", "<|tool>", "<tool|>", "<|tool_call>", "<tool_call|>",
    "<|tool_response>", "<tool_response|>", "<|\"|>", "<start_of_image>", "<end_of_image>",
]
# Rendered as text so the chat parser can read them (converter: USER_DEFINED).
VISIBLE = {"<|channel>", "<channel|>", "<|tool_call>", "<tool_call|>", "<|tool_response>", "<tool_response|>", "<|\"|>"}

# Compact Gemma 4 chat template with the official control tokens (system turn, image
# placeholder, thinking channel). llama.cpp selects its Gemma 4 chat parser when the
# template renders `'<|tool_call>call:'` and the "OpenAI Chat Completions" comment.
TEMPLATE = (
    "{{- bos_token -}}"
    "{%- set ns = namespace(prev_message_type=None) -%}"
    "{%- set loop_messages = messages -%}"
    "{%- if (enable_thinking is defined and enable_thinking) or tools or messages[0]['role'] in ['system', 'developer'] -%}"
    "{{- '<|turn>system\\n' -}}"
    "{%- if enable_thinking is defined and enable_thinking -%}{{- '<|think|>\\n' -}}{%- endif -%}"
    "{%- if messages[0]['role'] in ['system', 'developer'] -%}"
    "{{- messages[0]['content'] | trim -}}{%- set loop_messages = messages[1:] -%}"
    "{%- endif -%}"
    "{%- if tools -%}{%- for tool in tools -%}{{- '<|tool>' + (tool | tojson) + '<tool|>' -}}{%- endfor -%}{%- endif -%}"
    "{{- '<turn|>\\n' -}}"
    "{%- endif -%}"
    "{%- for message in loop_messages -%}"
    "{%- if message['role'] != 'tool' -%}"
    "{%- set role = 'model' if message['role'] == 'assistant' else message['role'] -%}"
    "{{- '<|turn>' + role + '\\n' -}}"
    "{%- if message['tool_calls'] -%}"
    "{%- for tool_call in message['tool_calls'] -%}"
    "{{- '<|tool_call>call:' + tool_call['function']['name'] + '{' + (tool_call['function']['arguments'] | tojson) + '}<tool_call|>' -}}"
    "{%- endfor -%}"
    "{%- endif -%}"
    "{#- OpenAI Chat Completions: tool results are role 'tool' messages -#}"
    "{%- if message['content'] is string -%}"
    "{{- message['content'] | trim -}}"
    "{%- elif message['content'] is sequence -%}"
    "{%- for item in message['content'] -%}"
    "{%- if item['type'] == 'text' -%}{{- item['text'] | trim -}}"
    "{%- elif item['type'] == 'image' -%}{{- '<|image|>' -}}"
    "{%- endif -%}"
    "{%- endfor -%}"
    "{%- endif -%}"
    "{{- '<turn|>\\n' -}}"
    "{%- else -%}"
    "{{- '<|turn>user\\n<|tool_response>response:' + (message['name'] | default('tool')) + '{' + (message['content'] | tojson) + '}<tool_response|><turn|>\\n' -}}"
    "{%- endif -%}"
    "{%- endfor -%}"
    "{%- if add_generation_prompt -%}"
    "{{- '<|turn>model\\n' -}}"
    "{%- if not enable_thinking | default(false) -%}{{- '<|channel>thought\\n<channel|>' -}}{%- endif -%}"
    "{%- endif -%}"
)


def corpus():
    lines = [
        "The quick brown fox jumps over the lazy dog.",
        "Invoice number 2026-0042 total amount 1,250,000 KRW due 2026-11-30.",
        "세금계산서 공급가액 1,000,000원 세액 100,000원 합계 1,100,000원",
        "영수증 발행일 2026년 10월 5일 거래처 주식회사 도큐렌즈",
        "계약서 제1조 목적 본 계약은 갑과 을 사이의 용역 제공에 관한 사항을 정한다.",
        "제안요청서 제안서 과업지시서 요구사항 정의서 소프트웨어 개발 계약서 견적서 회의록 검수확인서",
        "Reply with JSON: {\"ok\": true, \"answer\": \"hello\", \"doc_type\": \"estimate\", \"confidence\": 0.9}",
        "Hello world! How are you today? I am fine, thank you.",
        "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "가나다라마바사아자차카타파하 각 항목을 확인하세요. 문서 유형 분류 추출 요약 검색",
        "Say hello. Count: one two three four five six seven.",
        "What is this? Describe the page. Extract the fields as JSON. 이 문서의 유형은 무엇입니까?",
        "```json\n{\"doc_type\": \"rfp\", \"confidence\": 0.75, \"fields\": [{\"key\": \"total\", \"value\": \"1,100,000\"}]}\n```",
    ]
    out = []
    for i in range(200):
        out.append(lines[i % len(lines)] + f" {i}")
    return out


def build_tokenizer():
    """SPM-style BPE: spaces become ▁ (llama.cpp escapes whitespace for the gemma4
    pre-tokenizer), merges run on whole lines (`[^\\n]+|[\\n]+`), raw UTF-8."""
    from tokenizers import Regex, Tokenizer, models, normalizers, pre_tokenizers, trainers

    tok = Tokenizer(models.BPE(unk_token=None))
    tok.normalizer = normalizers.Replace(" ", "▁")
    tok.pre_tokenizer = pre_tokenizers.Split(Regex(r"[^\n]+|[\n]+"), behavior="isolated")
    alphabet = sorted({c for line in corpus() for c in line.replace(" ", "▁")} | set("{}[]:,\"'\\/\n\t<>|=+-_()%#@!?.;`~^&*$") | {"▁"})
    trainer = trainers.BpeTrainer(
        vocab_size=1536, special_tokens=SPECIALS, initial_alphabet=alphabet, show_progress=False, limit_alphabet=4096
    )
    tok.train_from_iterator(corpus(), trainer)
    data = json.loads(tok.to_str())
    vocab = data["model"]["vocab"]  # token → id
    merges = data["model"]["merges"]
    merges = [m if isinstance(m, str) else f"{m[0]} {m[1]}" for m in merges]
    tokens = [None] * len(vocab)
    for text, idx in vocab.items():
        tokens[idx] = text
    assert all(t is not None for t in tokens)
    return tokens, merges


def write_text_model(path: pathlib.Path, tokens, merges):
    import gguf

    rng = np.random.default_rng(0)
    w = gguf.GGUFWriter(str(path), "gemma4")
    w.add_name("tiny-gemma4")
    w.add_file_type(gguf.LlamaFileType.MOSTLY_F16)
    w.add_quantization_version(gguf.GGML_QUANT_VERSION)
    w.add_string("general.license", "apache-2.0")
    w.add_string("general.license.link", "https://huggingface.co/google/gemma-4-E4B-it")
    w.add_context_length(CTX)
    w.add_embedding_length(N_EMBD)
    w.add_block_count(N_LAYER)
    w.add_feed_forward_length(N_FF)
    w.add_head_count(N_HEAD)
    w.add_head_count_kv([N_HEAD_KV] * N_LAYER)
    w.add_key_length(HEAD_DIM_FULL)
    w.add_value_length(HEAD_DIM_FULL)
    w.add_key_length_swa(HEAD_DIM_SWA)
    w.add_value_length_swa(HEAD_DIM_SWA)
    w.add_sliding_window(SLIDING_WINDOW)
    w.add_sliding_window_pattern(SWA_PATTERN)
    w.add_shared_kv_layers(0)
    w.add_embedding_length_per_layer_input(0)
    w.add_layer_norm_rms_eps(RMS_EPS)
    w.add_rope_freq_base(ROPE_BASE_FULL)
    w.add_float32("gemma4.rope.freq_base_swa", ROPE_BASE_SWA)
    w.add_rope_dimension_count(HEAD_DIM_FULL)
    w.add_rope_dimension_count_swa(HEAD_DIM_SWA)
    w.add_vocab_size(len(tokens))

    # tokenizer
    w.add_tokenizer_model("gemma4")
    w.add_tokenizer_pre("gemma4")
    w.add_token_list(tokens)
    w.add_token_scores([0.0] * len(tokens))
    types = []
    for t in tokens:
        if t in VISIBLE:
            types.append(gguf.TokenType.USER_DEFINED)
        elif t in SPECIALS:
            types.append(gguf.TokenType.CONTROL)
        else:
            types.append(gguf.TokenType.NORMAL)
    w.add_token_types(types)
    w.add_token_merges(merges)
    w.add_bos_token_id(tokens.index("<bos>"))
    w.add_eos_token_id(tokens.index("<eos>"))
    w.add_pad_token_id(tokens.index("<pad>"))
    w.add_unk_token_id(tokens.index("<unk>"))
    w.add_add_bos_token(True)
    w.add_add_eos_token(False)
    w.add_add_space_prefix(False)
    w.add_chat_template(TEMPLATE)

    def f16(shape, scale=0.02):
        return (rng.standard_normal(shape) * scale).astype(np.float16)

    def norm(n):
        return (1.0 + rng.standard_normal(n) * 0.01).astype(np.float32)

    # Tied output head: zero the control-token rows so the random model's argmax is never a
    # control token (`<channel|>` would otherwise stop the chat parser on every reply).
    tok_embd = f16((len(tokens), N_EMBD), 0.5)
    for i, t in enumerate(tokens):
        if t in SPECIALS:
            tok_embd[i] = 0
    w.add_tensor("token_embd.weight", tok_embd)
    w.add_tensor("output_norm.weight", norm(N_EMBD))
    # "proportional" RoPE on the global layers: rotate the first head_dim*0.25 dims,
    # freeze the rest (converter: [1.0]*n_rot + [1e30]*n_unrot over head_dim/2 pairs).
    n_rot = int(HEAD_DIM_FULL * PARTIAL_ROTARY_FULL / 2)
    freqs = np.array([1.0] * n_rot + [1e30] * (HEAD_DIM_FULL // 2 - n_rot), dtype=np.float32)
    w.add_tensor("rope_freqs.weight", freqs)
    for i in range(N_LAYER):
        hd = HEAD_DIM_SWA if SWA_PATTERN[i] else HEAD_DIM_FULL
        w.add_tensor(f"blk.{i}.attn_norm.weight", norm(N_EMBD))
        w.add_tensor(f"blk.{i}.attn_q.weight", f16((N_HEAD * hd, N_EMBD)))
        w.add_tensor(f"blk.{i}.attn_k.weight", f16((N_HEAD_KV * hd, N_EMBD)))
        w.add_tensor(f"blk.{i}.attn_v.weight", f16((N_HEAD_KV * hd, N_EMBD)))
        w.add_tensor(f"blk.{i}.attn_output.weight", f16((N_EMBD, N_HEAD * hd)))
        w.add_tensor(f"blk.{i}.attn_q_norm.weight", norm(hd))
        w.add_tensor(f"blk.{i}.attn_k_norm.weight", norm(hd))
        w.add_tensor(f"blk.{i}.post_attention_norm.weight", norm(N_EMBD))
        w.add_tensor(f"blk.{i}.ffn_norm.weight", norm(N_EMBD))
        w.add_tensor(f"blk.{i}.ffn_gate.weight", f16((N_FF, N_EMBD)))
        w.add_tensor(f"blk.{i}.ffn_up.weight", f16((N_FF, N_EMBD)))
        w.add_tensor(f"blk.{i}.ffn_down.weight", f16((N_EMBD, N_FF)))
        w.add_tensor(f"blk.{i}.post_ffw_norm.weight", norm(N_EMBD))
    w.write_header_to_file()
    w.write_kv_data_to_file()
    w.write_tensors_to_file()
    w.close()


def write_mmproj(path: pathlib.Path):
    import gguf

    rng = np.random.default_rng(1)
    w = gguf.GGUFWriter(str(path), "clip")
    w.add_name("tiny-gemma4 mmproj")
    w.add_file_type(gguf.LlamaFileType.MOSTLY_F16)
    w.add_quantization_version(gguf.GGML_QUANT_VERSION)
    w.add_string("general.license", "apache-2.0")
    w.add_bool("clip.has_vision_encoder", True)
    w.add_string("clip.projector_type", "gemma4v")
    w.add_bool("clip.use_gelu", True)
    w.add_uint32("clip.vision.projection_dim", N_EMBD)
    w.add_uint32("clip.vision.image_size", 224)  # unused by the dynamic-size preprocessor
    w.add_uint32("clip.vision.patch_size", PATCH)
    w.add_uint32("clip.vision.embedding_length", V_EMBD)
    w.add_uint32("clip.vision.feed_forward_length", V_FF)
    w.add_uint32("clip.vision.block_count", V_LAYER)
    w.add_uint32("clip.vision.attention.head_count", V_HEAD)
    w.add_float32("clip.vision.attention.layer_norm_epsilon", 1e-6)
    w.add_array("clip.vision.image_mean", [0.0, 0.0, 0.0])
    w.add_array("clip.vision.image_std", [1.0, 1.0, 1.0])
    w.add_uint32("clip.vision.projector.scale_factor", POOL)

    def f16(shape, scale=0.02):
        return (rng.standard_normal(shape) * scale).astype(np.float16)

    def norm(n):
        return (1.0 + rng.standard_normal(n) * 0.01).astype(np.float32)

    w.add_tensor("v.patch_embd.weight", f16((V_EMBD, 3, PATCH, PATCH)))
    w.add_tensor("v.position_embd.weight", (rng.standard_normal((2, POS_SIZE, V_EMBD)) * 0.02).astype(np.float32))
    for i in range(V_LAYER):
        w.add_tensor(f"v.blk.{i}.ln1.weight", norm(V_EMBD))
        w.add_tensor(f"v.blk.{i}.attn_q.weight", f16((V_EMBD, V_EMBD)))
        w.add_tensor(f"v.blk.{i}.attn_k.weight", f16((V_EMBD, V_EMBD)))
        w.add_tensor(f"v.blk.{i}.attn_v.weight", f16((V_EMBD, V_EMBD)))
        w.add_tensor(f"v.blk.{i}.attn_out.weight", f16((V_EMBD, V_EMBD)))
        w.add_tensor(f"v.blk.{i}.ln2.weight", norm(V_EMBD))
        w.add_tensor(f"v.blk.{i}.ffn_up.weight", f16((V_FF, V_EMBD)))
        w.add_tensor(f"v.blk.{i}.ffn_down.weight", f16((V_EMBD, V_FF)))
    w.add_tensor("v.post_ln.weight", norm(V_EMBD))
    w.add_tensor("mm.input_projection.weight", f16((N_EMBD, V_EMBD)))
    w.write_header_to_file()
    w.write_kv_data_to_file()
    w.write_tensors_to_file()
    w.close()


def main():
    models_dir = OUT / "models" / "tiny-gemma4"
    models_dir.mkdir(parents=True, exist_ok=True)
    tokens, merges = build_tokenizer()
    print("vocab:", len(tokens), "merges:", len(merges))
    write_text_model(models_dir / "tiny-gemma4-F16.gguf", tokens, merges)
    write_mmproj(models_dir / "mmproj-F16.gguf")
    (models_dir / "LICENSE").write_text("Apache-2.0 (synthetic test fixture, random weights)\n")
    for p in sorted(models_dir.iterdir()):
        print(p.name, p.stat().st_size)


if __name__ == "__main__":
    sys.exit(main())
