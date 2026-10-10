"""Scores ocrbench JSON against make_cases.py ground truth.

    python scripts/ocr-bench/score.py <cases_dir> <result.json>...

Per case: word recall (ground-truth words found, in reading order via SequenceMatcher; and as a bag),
precision, box hit rate (IoU >= 0.5 among matched words) and ms/page. Printed as a markdown table.
"""
import difflib
import json
import sys
from collections import defaultdict
from pathlib import Path


def iou(a, b):
    x0, y0 = max(a[0], b[0]), max(a[1], b[1])
    x1, y1 = min(a[2], b[2]), min(a[3], b[3])
    inter = max(0.0, x1 - x0) * max(0.0, y1 - y0)
    ua = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / ua if ua > 0 else 0.0


def score(gt, words):
    g = [w["t"] for w in gt]
    o = [w["t"] for w in words]
    sm = difflib.SequenceMatcher(a=g, b=o, autojunk=False)
    ordered = sum(b.size for b in sm.get_matching_blocks())
    # bag match with box assignment: each OCR word takes the unused GT word of the same text with best IoU
    by_text = defaultdict(list)
    for i, w in enumerate(gt):
        by_text[w["t"]].append(i)
    used, bag, hits, ious = set(), 0, 0, []
    for w in words:
        best, bi = -1.0, None
        for i in by_text.get(w["t"], []):
            if i in used:
                continue
            v = iou(gt[i]["box"], w["box"])
            if v > best:
                best, bi = v, i
        if bi is not None:
            used.add(bi)
            bag += 1
            ious.append(best)
            hits += best >= 0.5
    n = len(gt)
    return {
        "recall_ordered": ordered / n,
        "recall_bag": bag / n,
        "precision": bag / max(1, len(o)),
        "box_hit": hits / max(1, bag),
        "mean_iou": sum(ious) / max(1, len(ious)),
        "n_gt": n,
        "n_ocr": len(o),
    }


def main():
    cases = Path(sys.argv[1])
    rows = []
    for rp in sys.argv[2:]:
        for r in json.loads(Path(rp).read_text(encoding="utf-8")):
            gt = json.loads((cases / f"{r['case']}.json").read_text(encoding="utf-8"))
            dpi = gt["dpi"]
            words = []
            for pg in r["pages"]:
                for line in pg["lines"]:
                    for w in line:
                        words.append({"t": w["text"], "box": w["bbox"]})
            s = score(gt["words"], words)
            rows.append((r["case"], f"{r['backend']}/{r['lang']}@{r['dpi']}", s, r["ms_per_page"]))
    print("| case | backend | words (gt/ocr) | recall ordered | recall bag | precision | box IoU>=0.5 | mean IoU | ms/page |")
    print("|---|---|---|---|---|---|---|---|---|")
    for case, b, s, ms in rows:
        print(
            f"| {case} | {b} | {s['n_gt']}/{s['n_ocr']} | {s['recall_ordered']*100:.1f}% | {s['recall_bag']*100:.1f}% | "
            f"{s['precision']*100:.1f}% | {s['box_hit']*100:.1f}% | {s['mean_iou']:.2f} | {ms:.0f} |"
        )


if __name__ == "__main__":
    main()
