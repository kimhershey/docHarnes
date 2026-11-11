"""Scores planner result files of different app versions by one rule set (the 0.8.0 golden rules).

  python3 scripts/rescore-planner.py <golden.json> <label>=<results.json> [<label>=<results.json> ...] [--ids a,b] [--md out.md]

A results file is what `doculens-eval planner --out` writes; with `--engine` the primary pass is the model
path. The 0.7.0 binary knows nothing of `expected.confirm / unmet / alt` or of the kinds of turn, so its
own "correct" flags are not comparable — this script recomputes them from the recorded plans.
"""
import json
import sys

NO_SIDE_EFFECT = {"search", "answer", "aggregate", "open", "help", "clarify", "noop", "explain", "compare"}
SETTING_INTENTS = {"set_setting", "set_theme", "set_language", "set_profile", "set_model", "set_inbox_watch", "set_retention"}
KINDS = ["chat", "question", "command", "setting"]
KIND_KO = {"chat": "대화", "question": "질문", "command": "명령", "setting": "설정"}


def contains(actual, expected):
    if expected is None:
        return actual is None
    if isinstance(actual, dict) and isinstance(expected, dict):
        return all(contains(actual.get(k), v) if k in actual else v is None for k, v in expected.items())
    if isinstance(actual, list) and isinstance(expected, list):
        return len(actual) == len(expected) and all(contains(a, e) for a, e in zip(actual, expected))
    return actual == expected and type(actual) is type(expected) or (
        isinstance(actual, (int, float)) and isinstance(expected, (int, float))
        and not isinstance(actual, bool) and not isinstance(expected, bool) and actual == expected)


def kind_of(item):
    if item.get("kind"):
        return item["kind"]
    exp = item["expected"]
    params = exp.get("params") or []
    chat = bool(params) and isinstance(params[0], dict) and params[0].get("mode") == "chat"
    intent = exp["intent"]
    if intent == "noop" or (intent == "answer" and chat):
        return "chat"
    if intent in ("answer", "explain", "summarize", "compare", "search"):
        return "question"
    if intent in SETTING_INTENTS:
        return "setting"
    return "command"


def mismatches(expected, res):
    out = []
    if res["intent"] != expected["intent"]:
        out.append("intent")
    if res["ops"] != expected["ops"]:
        out.append("ops")
        return out
    ops = res.get("operations") or []
    for i, p in enumerate(expected.get("params") or []):
        if p is None or i >= len(ops):
            continue
        if not contains(ops[i], p):
            out.append(f"params[{i}]")
    if expected.get("scope") is not None and not contains(res.get("scope") or {}, expected["scope"]):
        out.append("scope")
    if expected.get("confirm") is not None and bool(res.get("requires_confirmation")) != expected["confirm"]:
        out.append("confirm")
    if expected.get("unmet") is not None and bool((res.get("unmet") or "").strip()) != expected["unmet"]:
        out.append("unmet")
    return out


def judge(expected, res):
    best = mismatches(expected, res)
    for alt in expected.get("alt") or []:
        if not best:
            break
        other = mismatches(alt, res)
        if len(other) < len(best):
            best = other
    return best


def unasked_change(res, wrong):
    if not wrong or res["intent"] == "clarify" or res.get("requires_confirmation"):
        return False
    for op in res.get("operations") or []:
        name = op.get("op")
        if name in NO_SIDE_EFFECT:
            continue
        if name == "organize" and op.get("dry_run"):
            continue
        if name == "review" and op.get("action") == "show":
            continue
        if name == "classify_projects" and not op.get("apply"):
            continue
        return True
    return False


def load_pass(path):
    data = json.load(open(path, encoding="utf-8"))
    return data, {i["id"]: i for i in data["items"]}


def pct(a, b):
    return f"{a}/{b} ({100.0 * a / b:.0f} %)" if b else "–"


def main(argv):
    golden = json.load(open(argv[0], encoding="utf-8"))
    runs, ids, md = [], None, None
    it = iter(argv[1:])
    for a in it:
        if a == "--ids":
            ids = set(next(it).split(","))
        elif a == "--md":
            md = next(it)
        else:
            label, path = a.split("=", 1)
            runs.append((label, *load_pass(path)))
    items = [g for g in golden["items"] if (ids is None or g["id"] in ids) and all(g["id"] in r[2] for r in runs)]
    lines = []
    header = "| 종류 | 항목 수 | " + " | ".join(label for label, _, _ in runs) + " |"
    lines += [header, "|---|---:|" + "---:|" * len(runs)]
    table = {}
    for label, data, by_id in runs:
        per_kind, unasked, lat = {k: [0, 0] for k in KINDS}, 0, []
        rows = {}
        for g in items:
            res = by_id[g["id"]]
            miss = judge(g["expected"], res)
            k = kind_of(g)
            per_kind[k][1] += 1
            per_kind[k][0] += not miss
            ua = unasked_change(res, bool(miss))
            unasked += ua
            lat.append(res.get("latency_ms", 0.0))
            rows[g["id"]] = (miss, ua, res)
        table[label] = (per_kind, unasked, sorted(lat), rows, data)
    for k in KINDS:
        n = table[runs[0][0]][0][k][1]
        if n:
            lines.append(f"| {KIND_KO[k]} | {n} | " + " | ".join(pct(*table[l][0][k]) for l, _, _ in runs) + " |")
    total = len(items)
    lines.append(f"| **전체** | {total} | " + " | ".join(
        "**" + pct(sum(v[0] for v in table[l][0].values()), total) + "**" for l, _, _ in runs) + " |")
    lines.append(f"| 확인 없이 실행됐을 잘못된 변경 | | " + " | ".join(f"{table[l][1]}건" for l, _, _ in runs) + " |")
    lines.append(f"| 응답 시간 중앙값 | | " + " | ".join(
        (f"{table[l][2][len(table[l][2]) // 2] / 1000:.1f} s" if table[l][2] else "–") for l, _, _ in runs) + " |")
    lines.append("")
    lines.append("| id | 문장 | " + " | ".join(label for label, _, _ in runs) + " |")
    lines.append("|---|---|" + "---|" * len(runs))
    for g in items:
        cells = []
        for label, _, _ in runs:
            miss, ua, res = table[label][3][g["id"]]
            ops = ",".join(res["ops"])
            detail = ""
            for op in res.get("operations") or []:
                if op.get("op") == "set_setting":
                    detail = f" {op.get('key')}={json.dumps(op.get('value'), ensure_ascii=False)}"
                elif op.get("op") in ("set_theme", "set_profile", "set_language", "set_model"):
                    detail = " " + json.dumps({k: v for k, v in op.items() if k != "op"}, ensure_ascii=False)
                elif op.get("op") == "answer" and op.get("mode"):
                    detail = f" mode={op['mode']}"
                elif op.get("op") == "summarize":
                    detail = " " + json.dumps({k: v for k, v in op.items() if k != "op"}, ensure_ascii=False)
            mark = "○" if not miss else "✕"
            extra = " · 확인 없이 실행" if ua else (" · 확인 카드" if res.get("requires_confirmation") and res["intent"] != "clarify" else "")
            cells.append(f"{mark} {ops}{detail}{extra}")
        text = g["text"] if len(g["text"]) <= 44 else g["text"][:42] + "…"
        lines.append(f"| {g['id']} | {text} | " + " | ".join(cells) + " |")
    out = "\n".join(lines) + "\n"
    if md:
        open(md, "w", encoding="utf-8").write(out)
    print(out)


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except BrokenPipeError:
        pass
