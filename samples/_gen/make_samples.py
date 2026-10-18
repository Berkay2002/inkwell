"""Builds the Acrobat-study sample PDFs (run from anywhere; needs pdflatex, ImageMagick `magick`, pikepdf).
Outputs go to the parent folder (samples)."""
import subprocess, shutil, pathlib, sys
import pikepdf
from pikepdf import Pdf, Name, Dictionary, Array, String, Stream, AttachedFileSpec

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent
TMP = HERE / '_build'
TMP.mkdir(exist_ok=True)

def latex(name, tex):
    p = TMP / f'{name}.tex'
    p.write_text(tex, encoding='utf-8')
    for _ in range(2):
        subprocess.run(['pdflatex', '-interaction=nonstopmode', '-halt-on-error', '--enable-installer', p.name],
                       cwd=TMP, check=True, capture_output=True)
    return TMP / f'{name}.pdf'

# --- redact-source: latex base + attachment, hidden OCG layer, comment annotation
shutil.copy(HERE / 'redact-source.tex', TMP / 'redact-base.tex')
base = latex('redact-base', (HERE / 'redact-source.tex').read_text(encoding='utf-8'))
pdf = Pdf.open(base)
# attachment
pdf.attachments['secret-notes.txt'] = AttachedFileSpec(pdf, b'Attachment body: SSN 123-45-6789\n', mime_type='text/plain',
                                                       description='Planted attachment')
# hidden OCG (optional content) layer with text on page 1
ocg = pdf.make_indirect(Dictionary(Type=Name.OCG, Name=String('Hidden Layer')))
pdf.Root.OCProperties = Dictionary(OCGs=Array([ocg]), D=Dictionary(OFF=Array([ocg]), Order=Array([ocg])))
page = pdf.pages[0]
font = pdf.make_indirect(Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica))
page.Resources.Properties = Dictionary(OC1=ocg)
if '/Font' not in page.Resources: page.Resources.Font = Dictionary()
page.Resources.Font.HF = font
extra = b'/OC /OC1 BDC BT /HF 12 Tf 72 100 Td (HIDDEN-LAYER-SECRET 555-66-7777) Tj ET EMC\n'
contents = page.Contents
if isinstance(contents, pikepdf.Array):
    page.Contents = Array(list(contents) + [pdf.make_stream(extra)])
else:
    page.Contents = Array([contents, pdf.make_stream(extra)])
# sticky-note comment with sensitive text
annot = pdf.make_indirect(Dictionary(Type=Name.Annot, Subtype=Name.Text, Rect=[400, 700, 420, 720],
                                     Contents=String('Comment: call 555-010-4477'), T=String('Reviewer'), Name=Name.Comment))
page.Annots = Array(list(page.get('/Annots', [])) + [annot])
pdf.save(OUT / 'redact-source.pdf')

# --- form-source (hyperref form)
shutil.copy(latex('form-src', r'''\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\usepackage{hyperref}
\begin{document}
\begin{Form}
\noindent Full name: \TextField[name=fullname,width=3in,bordercolor=0 0 0]{}\\[6pt]
Email: \TextField[name=email,width=3in,bordercolor=0 0 0]{}\\[6pt]
Subscribe: \CheckBox[name=subscribe,width=12pt,height=12pt,bordercolor=0 0 0]{}\\[6pt]
Plan: \ChoiceMenu[combo,name=plan,width=2in,bordercolor=0 0 0]{}{Free,Pro,Team}\\[6pt]
Contact by: \ChoiceMenu[radio,name=contact,bordercolor=0 0 0]{}{Email=e,Phone=p}\\[6pt]
\PushButton[name=submit,width=1in,height=0.3in]{Submit}
\end{Form}
\end{document}
'''), OUT / 'form-source.pdf')

# --- flat-form-source: a *non-interactive* form look (labels + lines) for Prepare Form auto-detect
shutil.copy(latex('flatform-src', r'''\documentclass[11pt]{article}
\usepackage[margin=1in]{geometry}
\begin{document}
\section*{Registration form}
\noindent Full name: \underline{\hspace{3in}}\\[10pt]
Email: \underline{\hspace{3in}}\\[10pt]
Date of birth: \underline{\hspace{1.5in}}\\[10pt]
\fbox{\phantom{X}} I agree to the terms \quad \fbox{\phantom{X}} Send me updates\\[10pt]
Signature: \underline{\hspace{3in}}
\end{document}
'''), OUT / 'flatform-source.pdf')

# --- pages-a / pages-b (labelled pages for organize)
def labelled(name, prefix, n):
    body = '\n'.join(rf'\newpage\thispagestyle{{empty}}\vspace*{{3in}}\centerline{{\fontsize{{80}}{{90}}\selectfont {prefix}{i}}}'
                     for i in range(1, n + 1))
    return latex(name, rf'\documentclass{{article}}\usepackage[margin=1in]{{geometry}}\begin{{document}}{body}\end{{document}}')
shutil.copy(labelled('pages-a', 'A', 4), OUT / 'pages-a.pdf')
shutil.copy(labelled('pages-b', 'B', 3), OUT / 'pages-b.pdf')

# --- scan-source: image-only page
subprocess.run(['magick', '-size', '1275x1650', 'xc:white', '-font', 'Arial', '-pointsize', '36', '-fill', 'black',
                '-annotate', '+100+200', 'Scanned document test page.',
                '-annotate', '+100+270', 'The quick brown fox jumps over the lazy dog.',
                '-annotate', '+100+340', 'Invoice 20260042   Total: $1,234.56',
                '-annotate', '+100+410', 'Contact: jane.tester@example.com',
                '-rotate', '0.6', '-density', '150', '-units', 'PixelsPerInch', str(TMP / 'scan.png')], check=True)
subprocess.run(['magick', str(TMP / 'scan.png'), '-density', '150', '-units', 'PixelsPerInch', '-compress', 'jpeg', '-quality', '85',
                str(OUT / 'scan-source.pdf')], check=True)

# --- compress-source: noisy photos so size is meaningful
subprocess.run(['magick', '-size', '1600x1200', 'plasma:fractal', '-quality', '97', str(TMP / 'photo1.jpg')], check=True)
subprocess.run(['magick', '-size', '1200x900', 'plasma:red-blue', '-quality', '97', str(TMP / 'photo2.jpg')], check=True)
shutil.copy(latex('compress-src', r'''\documentclass{article}\usepackage[margin=0.7in]{geometry}\usepackage{graphicx}
\begin{document}
Compression sample page one.\\
\includegraphics[width=\textwidth]{photo1.jpg}
\newpage Page two.\\
\includegraphics[width=\textwidth]{photo2.jpg}
\end{document}
'''), OUT / 'compress-source.pdf')

# --- edit-source: several fonts, an image, a bullet list
subprocess.run(['magick', '-size', '400x200', 'gradient:#2a6fdb-#e0407a', str(TMP / 'logo.png')], check=True)
shutil.copy(latex('edit-src', r'''\documentclass[12pt]{article}\usepackage[margin=1in]{geometry}\usepackage{graphicx}
\begin{document}
\section*{Edit PDF sample}
Serif body text in Computer Modern. \textbf{Bold run.} \textit{Italic run.}\\
{\sffamily Sans-serif line for font substitution tests.}\\
{\ttfamily Monospace line: x = 42;}

\medskip
\includegraphics[width=2in]{logo.png}

\begin{itemize}
\item First bullet item
\item Second bullet item
\end{itemize}

A longer paragraph that wraps across several lines so that reflow behaviour of the Edit PDF text boxes can be observed
when characters are inserted or deleted in the middle of the paragraph, with a few more words to be safe.
\end{document}
'''), OUT / 'edit-source.pdf')

for p in sorted(OUT.glob('*.pdf')):
    print(p.name, p.stat().st_size, len(Pdf.open(p).pages), 'pages')
