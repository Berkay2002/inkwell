"""Generates the OCR benchmark scans (image-only PDFs) with word-level ground truth.

    python scripts/ocr-bench/make_cases.py <out_dir>

For every case it writes <out_dir>/<id>.pdf (one image-only page, JPEG/Flate via Pillow) and <id>.json:
    {"id", "dpi", "page_pt": [w, h], "words": [{"t": "Word", "box": [x0, y0, x1, y1]}]}
Boxes are in PDF points with the origin at the top-left (y down), in reading order.
Nothing here is committed as a binary: re-run it to regenerate.
"""
import json
import math
import random
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

FONTS = "C:/Windows/Fonts/"
LETTER_PT = (612.0, 792.0)

EN = (
    "The committee reviewed the quarterly results and agreed that revenue grew faster than expected, "
    "while operating costs remained flat. Several members asked for a detailed breakdown of the regional "
    "figures before the next meeting, and the chairman promised to circulate the report by Friday. "
    "In addition, the board approved the new travel policy, which limits reimbursement to economy fares "
    "unless the trip exceeds six hours. Questions about the policy should be sent to the finance office."
)
SV_EN = (
    "Årsredovisningen för räkenskapsåret visar ett överskott på grund av lägre kostnader för underhåll "
    "och ökade intäkter från försäljning. Styrelsen föreslår att vinsten förs över i ny räkning. "
    "The auditors have confirmed that the accounts give a true and fair view of the company. "
    "Möte hålls på måndag klockan 14:30 i konferensrummet på första våningen. Välkommen! "
    "Åke Öberg och Märta Ågren ansvarar för protokollet; frågor skickas till kansliet."
)
DIGITS = [
    "Invoice 20260042 Total: $1,234.56 due 2026-11-30",
    "Contact: jane.tester@example.com or +46 8 123 45 67",
    "Subtotal $12,345.00 VAT 25% $3,086.25 Total $15,431.25",
    "Order #A-77310 shipped 03/14/2026 weight 12.5 kg price 99.99",
    "Account 1234 5678 9012 3456 balance -$1,000,000.00",
    "Ref: INV-2026-0042 (see page 3) amount 1.234,56 kr",
]


def font(name, pt, dpi):
    return ImageFont.truetype(FONTS + name, round(pt * dpi / 72))


def layout(words, fnt, x0, y0, x1, line_h, draw, dpi, out, y_limit):
    """Greedy word wrap; draws each word and appends its ground-truth box (points, y down)."""
    x, y = x0, y0
    space = draw.textlength(" ", font=fnt)
    for w in words:
        wl = draw.textlength(w, font=fnt)
        if x + wl > x1:
            x, y = x0, y + line_h
        if y > y_limit:
            break
        draw.text((x, y), w, font=fnt, fill=0)
        b = draw.textbbox((x, y), w, font=fnt)
        out.append({"t": w, "box": [b[0] * 72 / dpi, b[1] * 72 / dpi, b[2] * 72 / dpi, b[3] * 72 / dpi]})
        x += wl + space
    return y


def page(dpi, size_pt=LETTER_PT):
    w, h = round(size_pt[0] * dpi / 72), round(size_pt[1] * dpi / 72)
    im = Image.new("L", (w, h), 255)
    return im, ImageDraw.Draw(im), w, h


def case_clean(dpi, pt=11):
    im, d, w, h = page(dpi)
    f = font("arial.ttf", pt, dpi)
    words, out = (EN + " " + EN).split(), []
    m = round(72 * dpi / 72)
    layout(words, f, m, m, w - m, round(pt * 1.4 * dpi / 72), d, dpi, out, h - m)
    return im, out


def case_twocol():
    dpi = 300
    im, d, w, h = page(dpi)
    f = font("times.ttf", 10, dpi)
    m, gap = 300, 150
    colw = (w - 2 * m - gap) // 2
    words = (EN + " " + EN + " " + EN).split()
    out = []
    lh = round(10 * 1.35 * dpi / 72)
    # left column takes words until it is full, right column continues
    left, right = [], []
    n_left = _fit(words, f, d, m, m, m + colw, lh, h - m)
    layout(words[:n_left], f, m, m, m + colw, lh, d, dpi, left, h - m)
    layout(words[n_left:], f, m + colw + gap, m, w - m, lh, d, dpi, right, h - m)
    return im, left + right


def _fit(words, fnt, draw, x0, y0, x1, line_h, y_limit):
    x, y, n = x0, y0, 0
    sp = draw.textlength(" ", font=fnt)
    for wd in words:
        wl = draw.textlength(wd, font=fnt)
        if x + wl > x1:
            x, y = x0, y + line_h
        if y > y_limit:
            break
        x += wl + sp
        n += 1
    return n


def case_table():
    dpi = 300
    im, d, w, h = page(dpi)
    f = font("calibri.ttf", 11, dpi)
    out = []
    cols = [300, 900, 1500, 1900, 2250]
    heads = ["Item", "Description", "Qty", "Price", "Total"]
    rows = [
        ["A-100", "Hex bolt M8", "120", "$0.35", "$42.00"],
        ["A-101", "Washer 8mm", "300", "$0.05", "$15.00"],
        ["B-220", "Steel bracket", "18", "$4.20", "$75.60"],
        ["C-330", "Aluminium rail 2m", "6", "$18.75", "$112.50"],
        ["D-410", "Cable tie pack", "40", "$1.99", "$79.60"],
        ["E-500", "Power supply 24V", "2", "$129.00", "$258.00"],
        ["F-612", "Terminal block", "75", "$0.85", "$63.75"],
        ["G-700", "Enclosure IP65", "4", "$36.40", "$145.60"],
    ]
    rh = 90
    top = 400
    for ri, row in enumerate([heads] + rows):
        y = top + ri * rh
        d.line([(cols[0] - 30, y - 15), (cols[-1] + 250, y - 15)], fill=0, width=3)
        for ci, cell in enumerate(row):
            d.text((cols[ci], y), cell, font=f, fill=0)
            out.extend(_word_boxes(d, f, cols[ci], y, cell, dpi))
    y_end = top + (len(rows) + 1) * rh - 15
    d.line([(cols[0] - 30, y_end), (cols[-1] + 250, y_end)], fill=0, width=3)
    for c in cols + [cols[-1] + 250]:
        d.line([(c - 30, top - 15), (c - 30, y_end)], fill=0, width=3)
    return im, out


def _word_boxes(d, f, x, y, text, dpi, draw=False):
    res, sp = [], d.textlength(" ", font=f)
    for tok in text.split(" "):
        if draw:
            d.text((x, y), tok, font=f, fill=0)
        b = d.textbbox((x, y), tok, font=f)
        res.append({"t": tok, "box": [b[0] * 72 / dpi, b[1] * 72 / dpi, b[2] * 72 / dpi, b[3] * 72 / dpi]})
        x += d.textlength(tok, font=f) + sp
    return res


def case_sv():
    dpi = 300
    im, d, w, h = page(dpi)
    f = font("georgia.ttf", 11, dpi)
    out = []
    layout((SV_EN + " " + SV_EN).split(), f, 300, 300, w - 300, round(11 * 1.45 * dpi / 72), d, dpi, out, h - 300)
    return im, out


def case_digits():
    dpi = 300
    im, d, w, h = page(dpi)
    f = font("cour.ttf", 11, dpi)
    out = []
    y = 400
    for line in DIGITS * 3:
        out.extend(_word_boxes(d, f, 300, y, line, dpi, draw=True))
        y += round(11 * 1.8 * dpi / 72)
    return im, out


def skew(im, gt, dpi, deg):
    w, h = im.size
    rot = im.rotate(-deg, resample=Image.BICUBIC, fillcolor=255)  # clockwise by deg
    cx, cy = w / 2, h / 2
    a = math.radians(deg)
    out = []
    for g in gt:
        x0, y0, x1, y1 = [v * dpi / 72 for v in g["box"]]
        pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        # rotating the image clockwise by deg (y down): x' = cx + (x-cx)cos - (y-cy)sin, ...
        r = [(cx + (x - cx) * math.cos(a) - (y - cy) * math.sin(a), cy + (x - cx) * math.sin(a) + (y - cy) * math.cos(a)) for x, y in pts]
        xs, ys = [p[0] for p in r], [p[1] for p in r]
        out.append({"t": g["t"], "box": [min(xs) * 72 / dpi, min(ys) * 72 / dpi, max(xs) * 72 / dpi, max(ys) * 72 / dpi]})
    return rot, out


def noisy(im):
    rnd = random.Random(7)
    im = im.filter(ImageFilter.GaussianBlur(0.9))
    px = im.load()
    w, h = im.size
    for _ in range(w * h // 60):  # speckles
        x, y = rnd.randrange(w), rnd.randrange(h)
        px[x, y] = rnd.choice((0, 255, 90))
    # gaussian-ish noise via a second pass
    data = im.getdata()
    noisy_px = [max(0, min(255, int(v + rnd.gauss(0, 16)))) for v in data]
    im.putdata(noisy_px)
    return im


def save(out_dir, cid, im, gt, dpi, note, quality=None):
    pdf = out_dir / f"{cid}.pdf"
    kw = {"resolution": dpi}
    im.save(pdf, "PDF", **kw)
    meta = {"id": cid, "note": note, "dpi": dpi, "page_pt": [im.size[0] * 72 / dpi, im.size[1] * 72 / dpi], "words": gt}
    (out_dir / f"{cid}.json").write_text(json.dumps(meta, ensure_ascii=False), encoding="utf-8")
    print(cid, im.size, len(gt), "words")


def main():
    out_dir = Path(sys.argv[1])
    out_dir.mkdir(parents=True, exist_ok=True)

    im, gt = case_clean(300)
    save(out_dir, "c01_clean300", im, gt, 300, "Arial 11pt, 300 dpi, two English paragraphs")

    im150 = im.resize((im.size[0] // 2, im.size[1] // 2), Image.LANCZOS)
    save(out_dir, "c02_clean150", im150, gt, 150, "same page downsampled to 150 dpi")

    r, g = skew(im, gt, 300, 2.0)
    save(out_dir, "c03_skew2deg", r, g, 300, "same page rotated 2 degrees")

    n = noisy(im.copy())
    save(out_dir, "c04_noise", n, gt, 300, "blur + speckles + gaussian noise")

    im2, gt2 = case_twocol()
    save(out_dir, "c05_twocol", im2, gt2, 300, "Times 10pt, two columns")

    im3, gt3 = case_table()
    save(out_dir, "c06_table", im3, gt3, 300, "ruled table, Calibri 11pt")

    im4, gt4 = case_sv()
    save(out_dir, "c07_sv_en", im4, gt4, 300, "Swedish + English, a/ä/ö, Georgia 11pt")

    im5, gt5 = case_digits()
    save(out_dir, "c08_numbers", im5, gt5, 300, "Courier 11pt: currency, dates, e-mail, ids")

    ims, gts = case_clean(300, pt=7.5)
    save(out_dir, "c09_small", ims, gts, 300, "Arial 7.5pt at 300 dpi")


if __name__ == "__main__":
    main()
