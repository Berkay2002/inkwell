"""Makes the bundled fallback fonts in src-tauri/fonts/ (see third_party/fonts/README.md).

Usage: python subset_fonts.py <notofonts.github.io checkout> <noto-cjk checkout> <out dir>
Needs fontTools (pip install fonttools). The checkouts only need the files named below
(git sparse-checkout works).
"""
import os
import subprocess
import sys

LATIN = ",".join([
    "U+0000-024F", "U+0250-02FF", "U+0300-036F", "U+0370-03FF", "U+0400-052F", "U+1E00-1EFF",
    "U+2000-206F", "U+20A0-20CF", "U+2100-214F", "U+2150-218F", "U+2190-21FF", "U+2200-22FF",
    "U+2460-24FF", "U+2500-257F", "U+25A0-25FF", "U+2600-26FF", "U+FB00-FB06", "U+FFFD",
])
CJK_EXTRA = "U+0020,U+3000-30FF,U+FF00-FFEF,U+3100-312F,U+31F0-31FF,U+3130-318F"


def cjk_chars(path):
    """Every character of GB 2312, JIS X 0208 and KS X 1001, written to `path`."""
    out = set()
    for codec, lead, trail in [("gb2312", range(0xA1, 0xF8), range(0xA1, 0xFF)),
                               ("euc_jp", range(0xA1, 0xF5), range(0xA1, 0xFF)),
                               ("euc_kr", range(0xA1, 0xFE), range(0xA1, 0xFF))]:
        for a in lead:
            for b in trail:
                try:
                    out.add(bytes([a, b]).decode(codec))
                except UnicodeDecodeError:
                    pass
    with open(path, "w", encoding="utf-8") as f:
        f.write("".join(sorted(out)))


def subset(src, dst, *args):
    subprocess.run(["pyftsubset", src, f"--output-file={dst}", "--layout-features=", "--no-hinting", *args], check=True)


def main():
    noto, cjk, out = sys.argv[1:4]
    for name in ["NotoSans-Regular", "NotoSans-Bold", "NotoSans-Italic", "NotoSans-BoldItalic"]:
        subset(os.path.join(noto, "fonts/NotoSans/hinted/ttf", name + ".ttf"), os.path.join(out, name + ".ttf"), f"--unicodes={LATIN}")
    for name in ["NotoSansMono-Regular", "NotoSansMono-Bold"]:
        subset(os.path.join(noto, "fonts/NotoSansMono/hinted/ttf", name + ".ttf"), os.path.join(out, name + ".ttf"), f"--unicodes={LATIN}")
    chars = os.path.join(os.path.dirname(__file__), "cjk-chars.txt")
    cjk_chars(chars)
    subset(os.path.join(cjk, "Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf"), os.path.join(out, "NotoSansCJK-Regular.otf"),
           f"--text-file={chars}", f"--unicodes={CJK_EXTRA}")


if __name__ == "__main__":
    main()
