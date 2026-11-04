#!/usr/bin/env python
"""Generate a tiny randomly-initialised Qwen3.5 VLM and convert it to GGUF
(text model + mmproj) with llama.cpp's converter. Output quality is irrelevant;
the files exist to exercise the DocuLens engine path (design §4.2).

Layout (override the root with DOCULENS_SCRATCH):
  <root>/engine/llama.cpp   — llama.cpp checkout (tag from engines/manifest.json; provides gguf-py + converter)
  <root>/tiny-qwen35/hf     — generated HF model (transformers ≥ 5.22, torch CPU, tokenizers)
  <root>/tiny-qwen35/models/tiny-qwen35/{tiny-qwen35-F16.gguf, mmproj-F16.gguf, LICENSE}
Copy the last folder to crates/doculens-core/tests/fixtures/ to refresh the committed fixture.
"""
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(os.environ.get("DOCULENS_SCRATCH", pathlib.Path(__file__).resolve().parent.parent / "target" / "tiny-qwen35-work"))
LLAMA = ROOT / "engine" / "llama.cpp"
OUT = ROOT / "tiny-qwen35"
HF = OUT / "hf"
PY = sys.executable

SPECIALS = [
    "<|endoftext|>", "<|im_start|>", "<|im_end|>", "<|object_ref_start|>", "<|object_ref_end|>",
    "<|box_start|>", "<|box_end|>", "<|quad_start|>", "<|quad_end|>", "<|vision_start|>",
    "<|vision_end|>", "<|vision_pad|>", "<|image_pad|>", "<|video_pad|>", "<think>", "</think>",
]
QWEN_PATTERN = r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+"
TEMPLATE = (
    "{%- for message in messages %}{{ '<|im_start|>' + message.role + '\\n' }}"
    "{%- if message.content is string %}{{ message.content }}"
    "{%- else %}{%- for c in message.content %}"
    "{%- if c.type == 'image' or c.type == 'image_url' %}<|vision_start|><|image_pad|><|vision_end|>"
    "{%- elif c.type == 'text' %}{{ c.text }}{%- endif %}{%- endfor %}{%- endif %}"
    "{{ '<|im_end|>\\n' }}{%- endfor %}"
    "{%- if add_generation_prompt %}{{ '<|im_start|>assistant\\n' }}"
    "{%- if enable_thinking is defined and enable_thinking is false %}{{ '<think>\\n\\n</think>\\n\\n' }}{%- endif %}{%- endif %}"
)


def corpus():
    lines = [
        "The quick brown fox jumps over the lazy dog.",
        "Invoice number 2026-0042 total amount 1,250,000 KRW due 2026-11-30.",
        "세금계산서 공급가액 1,000,000원 세액 100,000원 합계 1,100,000원",
        "영수증 발행일 2026년 10월 5일 거래처 주식회사 도큐렌즈",
        "계약서 제1조 목적 본 계약은 갑과 을 사이의 용역 제공에 관한 사항을 정한다.",
        "Reply with JSON: {\"ok\": true, \"answer\": \"hello\"}",
        "Hello world! How are you today? I am fine, thank you.",
        "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "가나다라마바사아자차카타파하 각 항목을 확인하세요.",
        "Say hello. Count: one two three four five six seven.",
    ]
    out = []
    for i in range(200):
        out.append(lines[i % len(lines)] + f" {i}")
    return out


def build_tokenizer():
    from tokenizers import Regex, Tokenizer, decoders, models, pre_tokenizers, trainers
    from transformers import PreTrainedTokenizerFast

    tok = Tokenizer(models.BPE())
    tok.pre_tokenizer = pre_tokenizers.Sequence([
        pre_tokenizers.Split(Regex(QWEN_PATTERN), behavior="isolated"),
        pre_tokenizers.ByteLevel(add_prefix_space=False, use_regex=False),
    ])
    tok.decoder = decoders.ByteLevel()
    trainer = trainers.BpeTrainer(
        vocab_size=1536, special_tokens=SPECIALS, initial_alphabet=pre_tokenizers.ByteLevel.alphabet(), show_progress=False,
    )
    tok.train_from_iterator(corpus(), trainer)
    fast = PreTrainedTokenizerFast(
        tokenizer_object=tok,
        eos_token="<|im_end|>",
        pad_token="<|endoftext|>",
        additional_special_tokens=[s for s in SPECIALS if s not in ("<|im_end|>", "<|endoftext|>")],
        chat_template=TEMPLATE,
        model_max_length=4096,
        clean_up_tokenization_spaces=False,
    )
    HF.mkdir(parents=True, exist_ok=True)
    fast.save_pretrained(HF)
    return fast


def build_model(tokenizer):
    import torch
    from transformers import Qwen3_5Config, Qwen3_5ForConditionalGeneration

    tid = tokenizer.convert_tokens_to_ids
    vocab = len(tokenizer)
    text_cfg = dict(
        model_type="qwen3_5_text", vocab_size=vocab, hidden_size=64, intermediate_size=128, num_hidden_layers=4,
        num_attention_heads=2, num_key_value_heads=1, head_dim=64, hidden_act="silu", max_position_embeddings=4096,
        rms_norm_eps=1e-6, tie_word_embeddings=False, attention_bias=False, attention_dropout=0.0,
        rope_parameters={"rope_theta": 10000.0, "partial_rotary_factor": 0.25, "rope_type": "default",
                         "mrope_section": [3, 3, 2], "mrope_interleaved": True},
        linear_conv_kernel_dim=4, linear_key_head_dim=32, linear_value_head_dim=32, linear_num_key_heads=2,
        linear_num_value_heads=4, layer_types=["linear_attention"] * 3 + ["full_attention"],
        eos_token_id=tid("<|im_end|>"), pad_token_id=tid("<|endoftext|>"),
    )
    vision_cfg = dict(
        model_type="qwen3_5_vision", depth=2, hidden_size=32, intermediate_size=64, num_heads=2, in_channels=3,
        patch_size=16, spatial_merge_size=2, temporal_patch_size=2, out_hidden_size=64, num_position_embeddings=64,
        hidden_act="gelu_pytorch_tanh", initializer_range=0.02,
    )
    cfg = Qwen3_5Config(
        text_config=text_cfg, vision_config=vision_cfg,
        image_token_id=tid("<|image_pad|>"), video_token_id=tid("<|video_pad|>"),
        vision_start_token_id=tid("<|vision_start|>"), vision_end_token_id=tid("<|vision_end|>"),
        tie_word_embeddings=False,
    )
    cfg.architectures = ["Qwen3_5ForConditionalGeneration"]
    torch.manual_seed(0)
    model = Qwen3_5ForConditionalGeneration(cfg)
    model = model.to(torch.float32)
    n = sum(p.numel() for p in model.parameters())
    print(f"model params: {n:,}")
    model.save_pretrained(HF, safe_serialization=True)
    # llama.cpp converter expects the top-level config to carry these too
    conf = json.loads((HF / "config.json").read_text())
    conf.setdefault("torch_dtype", "float32")
    (HF / "config.json").write_text(json.dumps(conf, indent=2))
    (HF / "preprocessor_config.json").write_text(json.dumps({
        "image_processor_type": "Qwen2VLImageProcessor", "do_resize": True, "do_rescale": True, "do_normalize": True,
        "do_convert_rgb": True, "image_mean": [0.5, 0.5, 0.5], "image_std": [0.5, 0.5, 0.5],
        "patch_size": 16, "merge_size": 2, "temporal_patch_size": 2,
        "size": {"shortest_edge": 4096, "longest_edge": 65536}, "min_pixels": 4096, "max_pixels": 65536,
    }, indent=2))
    (HF / "LICENSE").write_text("Apache-2.0 (synthetic test fixture, random weights)\n")


def convert(mmproj: bool, outfile: pathlib.Path):
    code = f"""
import sys
sys.path.insert(0, {str(LLAMA)!r}); sys.path.insert(0, {str(LLAMA / 'gguf-py')!r})
import conversion.base as b
b.TextModel.get_vocab_base_pre = lambda self, tokenizer: "qwen2"
import convert_hf_to_gguf as c
sys.argv = ["convert_hf_to_gguf.py", {str(HF)!r}, "--outfile", {str(outfile)!r}, "--outtype", "f16"] + (["--no-mtp"] if not {mmproj!r} else []) + ({['--mmproj']!r} if {mmproj!r} else [])
c.main()
"""
    subprocess.run([PY, "-c", code], check=True, cwd=str(LLAMA))


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    tokenizer = build_tokenizer()
    print("vocab:", len(tokenizer))
    build_model(tokenizer)
    models_dir = OUT / "models" / "tiny-qwen35"
    models_dir.mkdir(parents=True, exist_ok=True)
    convert(False, models_dir / "tiny-qwen35-F16.gguf")
    convert(True, models_dir / "mmproj-F16.gguf")
    (models_dir / "LICENSE").write_text("Apache-2.0 (synthetic test fixture, random weights)\n")
    for p in sorted(models_dir.iterdir()):
        print(p.name, p.stat().st_size)


if __name__ == "__main__":
    main()
