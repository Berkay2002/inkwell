#!/usr/bin/env python3
"""Builds docs/study/samples/media-source.pdf: a RichMedia annotation with an H.264 mp4 (and a Flash
file beside it), a Screen annotation with an mp3 rendition, and on page 2 a 3D annotation and a movie
whose file is outside the document. Needs ffmpeg with libx264 and libmp3lame. Standard library only.

  python3 scripts/media-sample.py [out.pdf]
"""
import os, subprocess, sys, tempfile

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "docs", "study", "samples", "media-source.pdf")

with tempfile.TemporaryDirectory() as t:
    mp4, mp3 = os.path.join(t, "clip.mp4"), os.path.join(t, "voice.mp3")
    run = lambda *a: subprocess.run(["ffmpeg", "-v", "error", "-y", *a], check=True)
    # keyframes every 5 frames and the index first: seeking works from the first byte
    run("-f", "lavfi", "-i", "testsrc=size=320x180:rate=15:duration=4", "-f", "lavfi", "-i", "sine=frequency=440:duration=6",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-profile:v", "baseline", "-g", "5", "-crf", "36", "-c:a", "aac", "-b:a", "32k",
        "-movflags", "+faststart", "-shortest", mp4)
    run("-f", "lavfi", "-i", "sine=frequency=330:duration=6", "-c:a", "libmp3lame", "-b:a", "24k", mp3)
    video, audio = open(mp4, "rb").read(), open(mp3, "rb").read()

objs = {}
def stream(extra, data):
    return b"<< /Length %d %s >>\nstream\n" % (len(data), extra.encode()) + data + b"\nendstream"
def text(x, y, s, size=12):
    return f"BT /F1 {size} Tf {x} {y} Td ({s}) Tj ET\n"

objs[1] = b"<< /Type /Catalog /Pages 2 0 R >>"
objs[2] = b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"
objs[3] = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R /Annots [10 0 R 14 0 R] >>"
objs[4] = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R /Annots [17 0 R 18 0 R] >>"
objs[5] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
p1 = text(72, 740, "Media sample, page 1", 18) + text(72, 712, "RichMedia video (MP4, H.264) and Flash file; Screen annotation with an MP3.") + text(72, 380, "Screen annotation with an mp3 rendition:")
p2 = text(72, 740, "Media sample, page 2", 18) + text(72, 712, "3D annotation (not supported) and a movie in a file outside the document (not loaded).")
objs[6] = stream("", p1.encode())
objs[7] = stream("", p2.encode())
# RichMedia: the mp4 is the video instance, the swf rides along in the asset tree
objs[10] = (b"<< /Type /Annot /Subtype /RichMedia /Rect [72 480 392 660] /T (Test video) /F 4 /RichMediaContent << /Assets << /Names "
            b"[(clip.mp4) 11 0 R (player.swf) 12 0 R] >> /Configurations [<< /Subtype /Video /Instances [<< /Subtype /Video /Asset 11 0 R >>] >>] >> >>")
objs[11] = b"<< /Type /Filespec /F (clip.mp4) /UF (clip.mp4) /EF << /F 20 0 R >> >>"
objs[12] = b"<< /Type /Filespec /F (player.swf) /UF (player.swf) /EF << /F 21 0 R >> >>"
objs[20] = stream("/Type /EmbeddedFile /Subtype /video#2Fmp4", video)
objs[21] = stream("/Type /EmbeddedFile /Subtype /application#2Fx-shockwave-flash", b"FWS\x09" + b"\x40\0\0\0" + b"\0" * 64)
# Screen with a media rendition (mp3)
objs[14] = (b"<< /Type /Annot /Subtype /Screen /Rect [72 300 272 360] /T (Test audio) /F 4 "
            b"/A << /S /Rendition /OP 0 /R << /S /MR /C << /S /MCD /CT (audio/mpeg) /D 15 0 R >> >> >> >>")
objs[15] = b"<< /Type /Filespec /F (voice.mp3) /UF (voice.mp3) /EF << /F 22 0 R >> >>"
objs[22] = stream("/Type /EmbeddedFile", audio)
# page 2: 3D with a poster, and a movie that is a path outside the document
objs[17] = b"<< /Type /Annot /Subtype /3D /Rect [72 500 332 660] /Contents (3D model) /F 4 /3DD 23 0 R /AP << /N 24 0 R >> >>"
objs[23] = stream("/Type /3D /Subtype /U3D", b"U3D\0\x18\0\0\0")
objs[24] = stream("/Type /XObject /Subtype /Form /BBox [0 0 260 160] /Resources << /Font << /F1 5 0 R >> >>",
                  b"0.82 0.88 0.97 rg 0 0 260 160 re f 0.2 0.3 0.5 rg 100 50 60 60 re f BT /F1 12 Tf 0 g 12 140 Td (3D poster) Tj ET")
objs[18] = b"<< /Type /Annot /Subtype /Movie /Rect [72 300 272 380] /T (Outside movie) /F 4 /Movie << /F << /Type /Filespec /F (C:\\\\videos\\\\talk.mp4) >> >> >>"

buf = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
offs = {}
for n in sorted(objs):
    offs[n] = len(buf)
    buf += b"%d 0 obj\n" % n + objs[n] + b"\nendobj\n"
top = max(objs) + 1
xref = len(buf)
buf += b"xref\n0 %d\n0000000000 65535 f \n" % top
for n in range(1, top):
    buf += (b"%010d 00000 n \n" % offs[n]) if n in offs else b"0000000000 65535 f \n"
buf += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (top, xref)
open(out, "wb").write(buf)
print(out, len(buf), "bytes")
