#!/usr/bin/env python3
"""Download and convert HuggingFace datasets to sbh-compatible JSONL.

Usage:
    python3 scripts/fetch_datasets.py [--dataset <name>] [--all]

Datasets:
    deepset       deepset/prompt-injections      (labeled benign/injection)
    cyberec       cyberec/Prompt-injection-dataset
    trustai       TrustAIRLab/in-the-wild-jailbreak-prompts
    hackaprompt   hackaprompt-ai/hackaprompt-dataset

Output: fixtures/<name>.jsonl  (one JSON object per line, {text, label, source})
"""
import json
import sys
from pathlib import Path

FIXTURES = Path(__file__).parent.parent / "fixtures"
FIXTURES.mkdir(exist_ok=True)

DATASETS = {
    "deepset": {
        "hf_name": "deepset/prompt-injections",
        "out": "deepset_prompt_injections.jsonl",
        "splits": ["train"],
        "text_field": "text",
        "label_field": "label",
        "label_map": {0: "benign", 1: "injection"},
    },
    "cyberec": {
        "hf_name": "cyberec/Prompt-injection-dataset",
        "out": "cyberec_prompt_injections.jsonl",
        "splits": ["train"],
        "text_field": "text",
        "label_field": "label",
        "label_map": {0: "benign", 1: "injection"},
        "extra_fields": ["category", "severity"],
    },
    "trustai": {
        "hf_name": "TrustAIRLab/in-the-wild-jailbreak-prompts",
        "hf_config": "jailbreak_2023_12_25",
        "out": "trustai_jailbreaks.jsonl",
        "splits": ["train"],
        "text_field": "prompt",
        "label_field": None,
        "label_map": {},
    },
    "hackaprompt": {
        "hf_name": "hackaprompt/hackaprompt-dataset",
        "out": "hackaprompt.jsonl",
        "splits": ["train"],
        "text_field": "user_input",
        "label_field": None,
        "label_map": {},
    },
}


def fetch(key: str):
    cfg = DATASETS[key]
    try:
        from datasets import load_dataset
    except ImportError:
        print("ERROR: run: pip3 install datasets", file=sys.stderr)
        sys.exit(1)

    print(f"  fetching {cfg['hf_name']} …", flush=True)
    try:
        hf_config = cfg.get("hf_config")
        ds = load_dataset(cfg["hf_name"], hf_config) if hf_config else load_dataset(cfg["hf_name"])
    except Exception as e:
        print(f"  ERROR: {e}", file=sys.stderr)
        return 0

    out_path = FIXTURES / cfg["out"]
    count = 0
    with open(out_path, "w") as f:
        for split_name in cfg["splits"]:
            split = ds.get(split_name)
            if split is None:
                # try first available split
                split = ds[list(ds.keys())[0]]
            for row in split:
                text = row.get(cfg["text_field"], "")
                if not text or not str(text).strip():
                    continue
                label_raw = row.get(cfg["label_field"]) if cfg["label_field"] else None
                label = cfg["label_map"].get(label_raw, str(label_raw) if label_raw is not None else None)
                entry = {"text": str(text).strip(), "source": key}
                if label is not None:
                    entry["label"] = label
                for extra in cfg.get("extra_fields", []):
                    if extra in row and row[extra]:
                        entry[extra] = row[extra]
                f.write(json.dumps(entry) + "\n")
                count += 1

    print(f"  wrote {count} rows → {out_path}")
    return count


def main():
    args = sys.argv[1:]
    if "--all" in args:
        keys = list(DATASETS.keys())
    elif "--dataset" in args:
        idx = args.index("--dataset")
        keys = [args[idx + 1]]
    else:
        keys = list(DATASETS.keys())

    for key in keys:
        if key not in DATASETS:
            print(f"unknown dataset: {key}  (choices: {list(DATASETS.keys())})")
            continue
        fetch(key)


if __name__ == "__main__":
    main()
