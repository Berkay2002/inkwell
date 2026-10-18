"""Builds samples/bookmarks-links-source.pdf for study 14 (bookmarks and links).
4 pages (Letter), headings in two font sizes (24 pt and 16 pt Helvetica-Bold), body text 11 pt,
3 nested bookmarks (Chapter 1 > Section 1.1 > Section 1.1.1), and on page 1 three /Link annotations:
  - URI link, visible border (/Border [0 0 1], /C blue)
  - GoTo link to page 3 (/A << /S /GoTo /D [page /XYZ ...] >>), visible border
  - GoTo link to page 4 (/Dest array form) with an invisible border (/Border [0 0 0], no /C)
Needs pikepdf. Usage: python make_bookmarks_links.py [out.pdf]   (default: ../bookmarks-links-source.pdf)
"""
import sys, pathlib
import pikepdf
from pikepdf import Pdf, Name, Dictionary, Array, String

OUT = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 and not sys.argv[1].startswith('--') else pathlib.Path(__file__).resolve().parent.parent / 'bookmarks-links-source.pdf'
pdf = Pdf.new()
fb = pdf.make_indirect(Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name('/Helvetica-Bold')))
fr = pdf.make_indirect(Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica))

PAGES = [
    ('Chapter 1: Introduction', 24, [('Section 1.1: Background', 16)],
     ['This is page one of the bookmark and link study sample.', 'The blue box below is a web link, the lines after it are internal links.']),
    ('Chapter 1 continued', 24, [('Section 1.1: Background', 16)],
     ['Page two holds the Section 1.1 heading.', 'Bookmark 2 points here.']),
    ('Section 1.1.1: Details', 16, [],
     ['Page three holds the deepest heading (16 pt).', 'The first GoTo link on page one lands here.']),
    ('Chapter 2: Appendix', 24, [],
     ['Page four. The invisible-border link on page one lands here.']),
]
page_objs = []
for i, (h, hs, subs, body) in enumerate(PAGES):
    lines = [f'BT /FB {hs} Tf 72 700 Td ({h}) Tj ET']
    y = 660
    for s, ss in subs:
        lines.append(f'BT /FB {ss} Tf 72 {y} Td ({s}) Tj ET'); y -= 40
    for t in body:
        lines.append(f'BT /FR 11 Tf 72 {y} Td ({t}) Tj ET'); y -= 18
    if i == 0:
        lines.append('BT /FR 11 Tf 72 540 Td (Visit the Example site) Tj ET')
        lines.append('BT /FR 11 Tf 72 500 Td (Jump to Details on page 3) Tj ET')
        lines.append('BT /FR 11 Tf 72 460 Td (Jump to Appendix on page 4) Tj ET')
    lines.append(f'BT /FR 9 Tf 300 40 Td (Page {i + 1}) Tj ET')
    content = pdf.make_stream('\n'.join(lines).encode('latin-1'))
    page = pdf.make_indirect(Dictionary(Type=Name.Page, MediaBox=Array([0, 0, 612, 792]),
        Resources=Dictionary(Font=Dictionary(FB=fb, FR=fr)), Contents=content))
    page_objs.append(page)
pdf.Root.Pages = pdf.make_indirect(Dictionary(Type=Name.Pages, Kids=Array(page_objs), Count=len(page_objs)))
for p in page_objs: p.Parent = pdf.Root.Pages

# links on page 1
def link(rect, **kw):
    return pdf.make_indirect(Dictionary(Type=Name.Annot, Subtype=Name.Link, Rect=Array(rect), **kw))
l1 = link([72, 534, 200, 552], Border=Array([0, 0, 1]), C=Array([0, 0, 1]), H=Name.I,
          A=Dictionary(S=Name.URI, URI=String('https://example.com/')))
l2 = link([72, 494, 220, 512], Border=Array([0, 0, 1]), C=Array([1, 0, 0]), H=Name.I,
          A=Dictionary(S=Name.GoTo, D=Array([page_objs[2], Name.XYZ, 72, 720, 0])))
l3 = link([72, 454, 230, 472], Border=Array([0, 0, 0]), Dest=Array([page_objs[3], Name.Fit]))
page_objs[0].Annots = Array([l1, l2, l3])

# outlines: Chapter 1 > Section 1.1 > Section 1.1.1
def item(title, dest, parent):
    return pdf.make_indirect(Dictionary(Title=String(title), Parent=parent, Dest=dest))
outlines = pdf.make_indirect(Dictionary(Type=Name.Outlines))
a = item('Chapter 1: Introduction', Array([page_objs[0], Name.XYZ, 72, 720, 0]), outlines)
b = item('Section 1.1: Background', Array([page_objs[1], Name.XYZ, 72, 660, 0]), a)
c = item('Section 1.1.1: Details', Array([page_objs[2], Name.XYZ, 72, 700, 0]), b)
a.First = a.Last = b; a.Count = 2
b.First = b.Last = c; b.Count = 1
outlines.First = outlines.Last = a; outlines.Count = 3
pdf.Root.Outlines = outlines
pdf.Root.PageMode = Name.UseOutlines
pdf.docinfo = pdf.make_indirect(Dictionary(Title=String('Bookmarks and links study sample'), Producer=String('pikepdf (Inkwell study generator)')))
pdf.save(OUT, static_id=True)
print('wrote', OUT)


# ---------------------------------------------------------------------------------------------------------------
# Optional: python make_bookmarks_links.py --variants <outdir>
# Writes bm-variants.pdf (outline states/styles/actions) and link-variants.pdf (border/colour/highlight/flag variants).
# These were opened in Acrobat only to see how it draws them; they are not committed.
if len(sys.argv) > 2 and sys.argv[1] == '--variants':
    outdir = pathlib.Path(sys.argv[2])
    v = Pdf.new()
    fr2 = v.make_indirect(Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name('/Helvetica')))
    def mkpage(text_lines):
        c = v.make_stream('\n'.join(f'BT /F 12 Tf 72 {720 - 24 * i} Td ({t}) Tj ET' for i, t in enumerate(text_lines)).encode('latin-1'))
        return v.make_indirect(Dictionary(Type=Name.Page, MediaBox=Array([0, 0, 612, 792]),
                               Resources=Dictionary(Font=Dictionary(F=fr2)), Contents=c))
    pg = [mkpage(['variant page 1']), mkpage(['variant page 2']), mkpage(['variant page 3'])]
    v.Root.Pages = v.make_indirect(Dictionary(Type=Name.Pages, Kids=Array(pg), Count=3))
    for p in pg: p.Parent = v.Root.Pages
    v.Root.Names = Dictionary(Dests=Dictionary(Names=Array([String('nd1'), Array([pg[2], Name.Fit])])))
    root = v.make_indirect(Dictionary(Type=Name.Outlines))
    def it(title, parent, **kw):
        d = Dictionary(Title=String(title), Parent=parent)
        for k_, val in kw.items(): d[('/' + k_)] = val
        return v.make_indirect(d)
    def link_chain(items, parent):
        for a_, b_ in zip(items, items[1:]):
            a_.Next = b_; b_.Prev = a_
        parent.First = items[0]; parent.Last = items[-1]
    top = []
    p1 = it('Collapsed parent (Count -2)', root, Dest=Array([pg[0], Name.Fit]))
    c1 = it('child red bold', p1, Dest=Array([pg[0], Name.Fit]), C=Array([1, 0, 0]), F=2)
    c2 = it('child blue italic', p1, Dest=Array([pg[0], Name.Fit]), C=Array([0, 0, 1]), F=1)
    link_chain([c1, c2], p1); p1.Count = -2
    p2 = it('Expanded parent (Count 2)', root, Dest=Array([pg[1], Name.Fit]))
    c3 = it('child green bold+italic', p2, Dest=Array([pg[1], Name.Fit]), C=Array([0, 0.6, 0]), F=3)
    c4 = it('child with URI action', p2, A=Dictionary(S=Name.URI, URI=String('https://example.com/')))
    link_chain([c3, c4], p2); p2.Count = 2
    p3 = it('named destination nd1 (page 3)', root, Dest=String('nd1'))
    p4 = it('Named action NextPage', root, A=Dictionary(S=Name.Named, N=Name.NextPage))
    p5 = it('GoToR other.pdf page 1', root, A=Dictionary(S=Name.GoToR, F=String('other.pdf'), D=Array([0, Name.Fit])))
    p6 = it('no dest and no action', root)
    p7 = it('GoTo with action dict, XYZ zoom 150%', root, A=Dictionary(S=Name.GoTo, D=Array([pg[1], Name.XYZ, 0, 792, 1.5])))
    p8 = it('Unicode: Überschrift ñ 日本', root, Dest=Array([pg[0], Name.Fit]))
    p9 = it('A very long bookmark title that should show whether the panel wraps, truncates or scrolls horizontally in the navigation pane', root, Dest=Array([pg[0], Name.Fit]))
    p10 = it('FitH top 400 (page 2)', root, Dest=Array([pg[1], Name.FitH, 400]))
    link_chain([p1, p2, p3, p4, p5, p6, p7, p8, p9, p10], root)
    root.Count = 10 + 2     # p1 collapsed (its 2 children hidden), p2 open (2 children shown)
    v.Root.Outlines = root
    v.Root.PageMode = Name.UseOutlines
    v.save(outdir / 'bm-variants.pdf', static_id=True)

    w = Pdf.new()
    fr3 = w.make_indirect(Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name('/Helvetica')))
    labels = ['L1 default /Border [0 0 1] no /C', 'L2 /Border [0 0 4] /C green', 'L3 /BS dashed 2 /C red',
              'L4 /Border [0 0 0] /C blue (invisible)', 'L5 /H /P (push) /C black', 'L6 /H /O (outline) /C red /Border 2',
              'L7 /F 2 (hidden) /Border 1', 'L8 /F 4 print, rect zero height', 'L9 /A Launch notepad', 'L10 /C [] transparent']
    content = '\n'.join(f'BT /F 11 Tf 72 {700 - 40 * i} Td ({t}) Tj ET' for i, t in enumerate(labels))
    page = w.make_indirect(Dictionary(Type=Name.Page, MediaBox=Array([0, 0, 612, 792]),
                           Resources=Dictionary(Font=Dictionary(F=fr3)), Contents=w.make_stream(content.encode('latin-1'))))
    w.Root.Pages = w.make_indirect(Dictionary(Type=Name.Pages, Kids=Array([page]), Count=1)); page.Parent = w.Root.Pages
    uri = lambda: Dictionary(S=Name.URI, URI=String('https://example.com/'))
    def ln(i, **kw):
        y = 700 - 40 * i
        return w.make_indirect(Dictionary(Type=Name.Annot, Subtype=Name.Link, Rect=Array([70, y - 4, 330, y + 12]), A=uri(), **kw))
    ann = [ln(0, Border=Array([0, 0, 1])),
           ln(1, Border=Array([0, 0, 4]), C=Array([0, 0.7, 0])),
           ln(2, BS=Dictionary(W=2, S=Name.D, D=Array([3, 2])), C=Array([1, 0, 0])),
           ln(3, Border=Array([0, 0, 0]), C=Array([0, 0, 1])),
           ln(4, H=Name.P, C=Array([0, 0, 0]), Border=Array([0, 0, 1])),
           ln(5, H=Name.O, C=Array([1, 0, 0]), Border=Array([0, 0, 2])),
           ln(6, F=2, Border=Array([0, 0, 1])),
           w.make_indirect(Dictionary(Type=Name.Annot, Subtype=Name.Link, Rect=Array([70, 700 - 280, 330, 700 - 280]), F=4,
                                      A=uri(), Border=Array([0, 0, 1]))),
           w.make_indirect(Dictionary(Type=Name.Annot, Subtype=Name.Link, Rect=Array([70, 700 - 320 - 4, 330, 700 - 320 + 12]),
                                      A=Dictionary(S=Name.Launch, F=String('notepad.exe')), Border=Array([0, 0, 1]))),
           ln(9, C=Array([]), Border=Array([0, 0, 1]))]
    page.Annots = Array(ann)
    w.save(outdir / 'link-variants.pdf', static_id=True)
    print('wrote variants to', outdir)
