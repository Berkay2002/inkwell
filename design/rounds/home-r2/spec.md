# Home r2: a desktop app, rounded and smooth

Round 1 (desk / contents / blotter) was rejected: "This is not a website, its a desktop app, take reference from other
similar apps, but I want not blocky/edgy components and corners etc, it should be rounded and smooth, same for
assistant UI". R1 composed editorial web pages (huge headline, no app chrome, sharp hairline rules). R2 shows a real
Windows 11 desktop application: title bar with tabs, toolbar or sidebar, and soft rounded surfaces (8-12 px corners,
rounded list rows and buttons, soft shadows), modelled on comparable PDF desktop apps (PDF Expert, PDFgear, UPDF,
Foxit's home) and polished native apps (Windows 11 Files/Photos/Notepad, Arc, Linear desktop). Kept from r1: the
warm-paper palette with one ink-blue accent, real first-page thumbnails of documents, no purple, no sparkles.
The user's verdict overrides the profile's "no cards/boxes" rule for this product: rounded surfaces are wanted.

Idea: a calm home base that gets you back into your documents in one click, and puts the Pro tools one click away.
For: one person on a big Windows desktop, opening PDFs many times a day. Feel: soft, smooth, native, quietly premium.
Palette: window #F4F2ED; raised surfaces #FFFFFF; sidebar #ECE9E2; text #1A1C20; secondary #6B675F; borders #E3DED4;
accent ink blue #2E4B8F (selected item, primary button), used sparingly. Dark theme later from the approved light one.
Type: UI in a smooth humanist sans like Segoe UI Variable or Inter; document titles may use a soft serif like Newsreader.
Copy: app name "Inkwell"; "Open file" (Ctrl O); "Drop a PDF here"; recent files: Quarterly report Q3.pdf (24 pages,
12 min ago), Lease agreement.pdf (pinned, 3 pages, 3 h ago), Invoice 2026-0412.pdf (2 pages, yesterday), Conference
notes.pdf (4 pages, 3 days ago), Thesis draft v4.pdf (86 pages, 1 Oct), Product brochure.pdf (12 pages, 28 Sep);
tools: Organize pages, Edit text, Annotate, Fill & sign, Redact, OCR, Compress, Merge.
Motion (for the build): rows and thumbnails fade up 6 px in sequence (30 ms apart) on launch; hover raises a row with
a soft shadow and 1.01 scale; drop area brightens and its border turns accent on drag-over. Reduced motion: no
movement, only colour changes.
Must not appear: sharp square corners, hard 1 px boxy tables, purple/indigo/violet, gradients, neon, glow, sparkles,
robot icons, emoji, people, marketing-page hero text, browser chrome, laptop frames.

Verdict (2026-10-10): no single pick. User: "Honestly same idea with the starting page, for the b option, it reminds me of Microsoft word etc, I think all makes sense, so its hard to pick." Next: one mix render (d-mix) for approval: a's sidebar frame, b's tool row and Continue reading, c's preview pane for the selected recent file.

Verdict 2 (2026-10-10, supersedes the mix): User: "But if you ask me what is the best landing/starting page, its B. But those other two are also good candidates but for other types of views." Build: b-gallery is the home screen (approved/home.png). a-sidebar becomes the Library view (approved/library.png) and c-split its selected-file preview layout (approved/library-2.png). d-mix dropped.

Verdict 3 (2026-10-10, final): User: "Also yes use the d-mix, it looks good. Best of all." Build d-mix as the home screen (approved/home.png). The sidebar's Recent/Pinned/This PC pages use the library layouts (approved/library.png, library-2.png). b-gallery is superseded.

## Shared

Render: a flat screenshot of one Windows 11 desktop application window, 1440 x 960, seen straight on, filling the
image with a thin margin of plain desktop around it. It is the home screen of a PDF app called Inkwell, before any
document is opened. It must look like a real, shipping native desktop app, not a web page.
Window: Windows 11 style with rounded window corners, a slim custom title bar that holds the app name "Inkwell" with
a small ink-drop mark on the left, a "Home" tab, a "+" new-tab button, and the minimise/maximise/close controls on
the right.
Shape language: everything rounded and smooth: 8-12 px corner radius on panels, list rows, thumbnails, buttons and
inputs; pill-rounded search field; soft, diffuse shadows under raised surfaces; no sharp corners anywhere.
Palette: window #F4F2ED; raised surfaces white #FFFFFF; sidebar #ECE9E2; text #1A1C20; secondary text #6B675F;
borders #E3DED4; one accent, ink blue #2E4B8F, only for the selected item and the primary button. No other hues
(document thumbnails may show their own muted content).
Type: interface in a smooth humanist sans like Segoe UI Variable; document titles may use a soft serif like Newsreader.
Small, crisp, well-spaced interface text, like a polished native app.
Thumbnails: each recent file has a real miniature of its first page (a report cover with a small chart, a lease with
form lines, an invoice table, notes, a thesis title page, a brochure with a photo), on white paper with rounded corners.
Must not appear: square corners, boxy tables, purple, indigo, violet, gradients, glow, glass effects, sparkles,
robots, emoji, people, marketing-style giant headline, browser address bar, device frame, watermark, extra text.

## a-sidebar
Layout like a polished native file-centred app (PDFgear / Windows 11 Files): a left sidebar about 220 px wide on the
sidebar colour, with rounded nav items: "Home" (selected, accent-tinted rounded highlight), "Recent", "Pinned",
"This PC", then a small "Tools" label and rounded items "Organize pages", "Edit text", "Fill & sign", "Redact", "OCR",
"Compress". Main area on the window colour: at the top a wide rounded white panel, the drop area, with a soft
document glyph, "Drop a PDF here", and a rounded ink-blue "Open file" button with "Ctrl O" hint beside it; a rounded
pill search field "Search recent files" top right. Below, "Recent" with a small "Pinned first" toggle, and the six
files as a list of rounded white rows, each with a small first-page thumbnail, the file name, its folder in
secondary text, page count and time on the right, and a pin glyph on the pinned lease.

## b-gallery
Layout like PDF Expert / Preview's welcome: no sidebar. Under the title bar a slim toolbar row with a pill search
field centred and a settings glyph right. A friendly greeting line "Good afternoon" small at top left of the content.
A horizontal row of eight rounded quick-tool buttons (rounded-square white tiles with a simple line icon and a short
label each: Organize, Edit, Annotate, Fill & sign, Redact, OCR, Compress, Merge). Below, "Continue reading" with a
row of four large rounded first-page thumbnails with soft shadows, each with the file name under it and a thin
rounded progress bar showing how far it was read ("p. 12 of 24"). Below that, "Recent" as a compact list of rounded
rows. A rounded dashed-free drop hint at the very bottom: "Drop a PDF anywhere to open it".

## c-split
Layout like a two-pane native browser (Raycast / Finder with preview): left pane about 40 % wide, a rounded white
panel holding a pill search field "Search files", segmented control "Recent | Pinned | Folders" (pill-shaped), and the
file list as rounded rows with tiny thumbnails; "Quarterly report Q3.pdf" is selected with a soft accent-tinted
rounded highlight. Right pane: a large preview of that file's first page on white paper with rounded corners and a
soft shadow, and beside or under it the file details (24 pages, 2.4 MB, Documents\Work, opened 12 min ago) and a row
of rounded buttons: "Open" (ink blue, primary), "Summarize with Assistant", "Show in folder". A small rounded
"Open file  Ctrl O" button at the top right of the window content.

## d-mix
ref: renders/a-sidebar.png, renders/b-gallery.png, renders/c-split.png
The attached images are three earlier mockups of this same home screen; combine them into one calm screen, keeping
their shared look (window, palette, rounded shapes, type). From the first image: the left sidebar with Home (selected),
Recent, Pinned, This PC, and no Tools list in the sidebar. From the second: the "Good afternoon." greeting, the row of
eight rounded quick-tool buttons (Organize, Edit, Annotate, Fill & sign, Redact, OCR, Compress, Merge) and the
"Continue reading" row, but only three thumbnails. Below that, "Recent" as rounded rows like the first image, with
"Quarterly report Q3.pdf" selected; on the right a narrower preview pane like the third image showing that file's first
page, its details and rounded buttons "Open" (ink blue) and "Show in folder". A pill search field at the top and a
small "Open file  Ctrl O" button top right. Generous spacing; nothing crowded. Use plain grey document glyphs, never
red PDF icons; no sparkle icons.
