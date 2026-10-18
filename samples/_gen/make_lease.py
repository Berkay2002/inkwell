"""Writes "Lease agreement.pdf" (12 pages, plain PDF, no libraries): the document of the Organize
design (design/approved/organize.png) for screenshots. Run: python make_lease.py <out.pdf>"""
import sys

W, H = 612, 792

def esc(t):
    return t.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")

def text(x, y, size, t, bold=False):
    return f"BT /{'F2' if bold else 'F1'} {size} Tf {x} {y} Td ({esc(t)}) Tj ET\n"

def clauses(items):
    out, y = "", 720
    for head, body in items:
        out += text(64, y, 13, head, True)
        y -= 26
        for line in body:
            out += text(80, y, 10.5, line)
            y -= 16
        y -= 18
    return out

LOREM = [
    "The Tenant shall comply with all applicable laws, ordinances and rules,",
    "and shall use the Premises only for the purpose described in this Lease.",
    "Notices under this clause shall be given in writing to the other party.",
]

pages = []
# 1 cover
pages.append(text(64, 660, 30, "Residential", True) + text(64, 622, 30, "Lease Agreement", True)
             + "0.18 0.29 0.56 RG 2 w 64 600 m 548 600 l S\n"
             + "0.85 0.86 0.88 rg 64 330 484 240 re f\n"
             + text(64, 290, 12, "1234 Maple Street") + text(64, 272, 12, "Austin, TX 78701"))
heads = [
    ["1. PARTIES", "2. PREMISES"], ["3. TERM", "4. RENT", "5. SECURITY DEPOSIT"],
    ["6. USE OF PREMISES", "7. MAINTENANCE AND REPAIRS", "8. ALTERATIONS"],
    ["9. UTILITIES", "10. INSURANCE", "11. ASSIGNMENT AND SUBLETTING"],
    ["12. DEFAULT", "13. TERMINATION", "14. MISCELLANEOUS"],
    ["15. ENTIRE AGREEMENT", "16. NOTICES", "17. SEVERABILITY"],
    ["18. GOVERNING LAW", "19. COUNTERPARTS"],
]
for hs in heads:
    pages.append(clauses([(h, LOREM) for h in hs]))
# 9 signatures
pages.append(text(64, 720, 13, "SIGNATURES", True) + text(64, 680, 11, "Landlord:", True)
             + text(64, 660, 11, "Maple Property Management LLC")
             + "0 0 0 RG 0.8 w 64 600 m 300 600 l S 64 440 m 300 440 l S\n"
             + text(64, 580, 10, "By: Sarah Chen") + text(64, 520, 11, "Tenant:", True) + text(64, 420, 10, "Name: Alex Johnson"))
# 10-12 scanned appendices: grey paper; the last one is turned sideways
for name, title in [("APPENDIX A", "PROPERTY FLOOR PLAN"), ("APPENDIX B", "INVENTORY LIST"), ("APPENDIX C", "BUILDING RULES")]:
    pages.append("0.86 0.86 0.85 rg 0 0 612 792 re f\n0.3 g 0.3 G\n"
                 + text(220, 720, 16, name, True) + text(220, 700, 11, title)
                 + "1.5 w 90 220 432 440 re S 90 440 m 300 440 l S 300 220 m 300 660 l S\n")

objs = []
def add(o):
    objs.append(o)
    return len(objs)

fonts = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
bold = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>")
pages_id = len(objs) + 1 + 2 * len(pages)
kids = []
for i, c in enumerate(pages):
    data = c.encode("latin-1")
    s = add(f"<< /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream")
    rot = " /Rotate 90" if i == len(pages) - 1 else ""
    kids.append(add(f"<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {W} {H}]{rot} /Contents {s} 0 R "
                    f"/Resources << /Font << /F1 {fonts} 0 R /F2 {bold} 0 R >> >> >>"))
assert add(f"<< /Type /Pages /Kids [{' '.join(f'{k} 0 R' for k in kids)}] /Count {len(kids)} >>") == pages_id
cat = add(f"<< /Type /Catalog /Pages {pages_id} 0 R >>")
info = add("<< /Title (Lease agreement) >>")

out = bytearray(b"%PDF-1.7\n")
offs = []
for i, o in enumerate(objs, 1):
    offs.append(len(out))
    out += f"{i} 0 obj\n".encode() + (o if isinstance(o, bytes) else o.encode()) + b"\nendobj\n"
x = len(out)
out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
out += b"".join(f"{o:010d} 00000 n \n".encode() for o in offs)
out += f"trailer\n<< /Size {len(objs) + 1} /Root {cat} 0 R /Info {info} 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
open(sys.argv[1] if len(sys.argv) > 1 else "Lease agreement.pdf", "wb").write(out)
