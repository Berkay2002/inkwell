"""Writes the PDFs scripts/search-shots.mjs searches (plain PDFs, no libraries, Latin-1 text):
  Insurance policy.pdf, Move-out checklist.pdf, Café menu.pdf (accents), Thousand.pdf (1000 pages), sub/Inspection report.pdf,
  broken.pdf (not a PDF, to be skipped) and notes.txt (not listed).
Run: python scripts/search-samples.py <out-dir>   (Lease agreement.pdf comes from docs/study/samples/_gen/make_lease.py)"""
import os
import sys


def esc(t):
    return t.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


def page_stream(lines, size=11):
    out = "BT /F1 %d Tf 64 720 Td 18 TL\n" % size
    for line in lines:
        out += "(%s) Tj T*\n" % esc(line)
    return out + "ET"


def pdf(pages):
    objs = []
    n = len(pages)
    objs.append("<< /Type /Catalog /Pages 2 0 R >>")
    objs.append("<< /Type /Pages /Kids [%s] /Count %d >>" % (" ".join("%d 0 R" % (4 + 2 * i) for i in range(n)), n))
    objs.append("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
    for i, lines in enumerate(pages):
        s = page_stream(lines)
        objs.append("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents %d 0 R >>" % (5 + 2 * i))
        objs.append("<< /Length %d >>\nstream\n%s\nendstream" % (len(s.encode("latin-1")), s))
    out = bytearray(b"%PDF-1.7\n")
    offs = []
    for i, o in enumerate(objs):
        offs.append(len(out))
        out += ("%d 0 obj\n%s\nendobj\n" % (i + 1, o)).encode("latin-1")
    x = len(out)
    out += ("xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)).encode()
    for o in offs:
        out += ("%010d 00000 n \n" % o).encode()
    out += ("trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, x)).encode()
    return bytes(out)


def write(root, name, data):
    p = os.path.join(root, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        f.write(data)


root = sys.argv[1]
write(root, "Insurance policy.pdf", pdf([
    ["Renter's insurance policy", "Coverage begins on the date shown on the declarations page.",
     "The insurer must give written notice at least thirty days before it cancels this policy."],
    ["Claims", "Give notice of a loss as soon as you can. Late notice may reduce what the policy pays.",
     "Keep receipts for every item that you claim."],
    ["General terms", "No notice is needed for changes that lower your premium."],
]))
write(root, "Move-out checklist.pdf", pdf([
    ["Move-out checklist", "Give your landlord written notice before you move out.",
     "Return every key and remote on the last day."],
    ["Cleaning", "Wipe the oven, the fridge and the windows.", "Take photos of each room before you hand back the keys."],
]))
write(root, "Cafe menu.pdf", pdf([
    ["Caf\xe9 menu", "Un caf\xe9, un Caf\xe9 au lait and one cafe noir.", "Cr\xe8me br\xfbl\xe9e with notice of the day's special."],
]))
write(root, "Thousand.pdf", pdf([
    ["Page %d of the long report." % (i + 1), "The notice period is stated in section %d." % (i % 40 + 1) if i % 10 == 0 else "Nothing to see on this page.",
     "Filler line one two three four five six seven eight nine ten."]
    for i in range(1000)
]))
write(root, "sub/Inspection report.pdf", pdf([
    ["Inspection report", "The tenant received notice of the visit two days before.", "Smoke alarms were tested and worked."],
]))
write(root, "broken.pdf", b"this is not a pdf at all")
write(root, "notes.txt", b"notice me")
print("wrote", root)
