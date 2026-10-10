#!/usr/bin/env bash
# Runs ocrbench over the generated cases for every backend/language/dpi combination and scores them.
#   scripts/ocr-bench/run_all.sh <cases_dir> <out_dir>      (from the repo root)
# Needs: cargo build --release --bin ocrbench (in src-tauri), python with Pillow for make_cases.py.
set -e
CASES=$1
OUT=$2
mkdir -p "$OUT"
B=src-tauri/target/release/ocrbench.exe
run() { # backend lang dpi runs workers tag
  "$B" "$1" "$2" "$3" "$4" "$5" "$OUT/$6.json" "$CASES"/c*.pdf
}
echo "== windows en-US @300";  run windows en-US 300 3 1 win-en-300
echo "== windows sv-SE @300";  run windows sv-SE 300 3 1 win-sv-300
echo "== windows en-US @200";  run windows en-US 200 3 1 win-en-200
echo "== windows en-US @150";  run windows en-US 150 3 1 win-en-150
echo "== tesseract eng @300";  run tesseract en 300 2 1 tess-en-300
echo "== tesseract swe+eng @300"; run tesseract sv+en 300 2 1 tess-sv-300
echo "== tesseract eng @200";  run tesseract en 200 2 1 tess-en-200
python scripts/ocr-bench/score.py "$CASES" "$OUT"/*.json
