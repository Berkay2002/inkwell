# History r1: the Version history panel

First round for this surface (new: the user has not seen it). It continues the approved look of home, the docked assistant and organize: a real
Windows 11 desktop app, rounded and smooth (8-12 px corners), warm palette, one ink-blue accent, no sparkles.
Function from the task: every change (by you, by the assistant, or outside Inkwell) becomes a version; a timeline
grouped by day with named versions pinned on top; each version shows label, author, time and page chips; selecting
one shows its change list; a change opens a before/after view of that page with the differences marked; actions
Restore (never destructive, makes a new version on top), Save a copy, Name this version, Compare with current.

Idea: the document remembers every state it has been in, and shows you exactly what changed on the page.
For: someone who let the assistant fill a lease, or edited a contract over a week, and wants to see or undo a
change with confidence. Feel: calm, trustworthy, precise, native.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4;
accent ink blue #2E4B8F (selected version, primary button, the "after" side). Changed regions are marked with a soft
ink-blue rounded outline and a very light blue tint, never red/green (colour never carries meaning alone: removed
text is struck through, added text underlined).
Type: Segoe UI Variable for the interface; times and page chips small, tabular figures.
Copy: document "Lease agreement.pdf" (5 pages). Pinned: "Signed by tenant" (named, 2 Oct). Today: "Filled tenant
details" (Assistant, 14:32, p. 1, p. 3); "Saved" (You, 14:20, p. 2); "Changed outside Inkwell" (11:05, p. 4).
Yesterday: "Added 3 comments" (You, 16:48, p. 4); "Rotated page 5" (You, 16:40, p. 5). Change list of "Filled tenant
details": "p. 1 · Tenant name: blank → A. Novak"; "p. 1 · Start date set to 1 Nov 2026"; "p. 3 · Text: 2 words
changed"; "p. 3 · Signature field left empty". Actions: "Restore", "Save a copy", "Name this version", "Compare with
current". Panel header "Version history"; filter "All · You · Assistant · Outside".
Motion (for the build): panel slides in 240 ms ease-out like the assistant's; selecting a version expands its change
list with a 160 ms height ease; the before/after slider follows the pointer 1:1 (no easing); "Show differences"
fades the outlines in 120 ms. Reduced motion: no slide or fade, the settled states.
Must not appear: git branch graphs, commit hashes, code diff colours (red/green), sparkle icons, purple, gradients,
glow, avatars, emoji, sharp corners, browser chrome.

Verdict (2026-10-10): the standard pattern, per the brief: "take reference from other similar apps
from Adobe, Google and Microsoft and Github/Gitlab etc. There are alot of ways of showing diffs and so on, just take
the most standard way of doing it..." Adopted: a-timeline as the panel, "Compare" opens b-compare full-window with GitHub's image-diff modes
"2-up | Swipe | Onion skin" (c's slider = Swipe). Conventions, no new ones:
- Panel = Google Docs / Word version history: versions by day (Today, Yesterday, date), named versions on top plus an
  "Only show named versions" toggle, author + time per row; clicking a version shows the document at that version
  read-only with its changes highlighted, top bar "Restore this version" + "Save a copy"; row overflow menu: Name this
  version, Compare with current, Save a copy.
- Highlighting: added = green, removed = red strike/outline, changed = amber/blue (Word/GitHub colours in our tokens),
  never colour alone: +/− glyphs or labels too. A "1 of 4" previous/next change stepper (Word Compare, Acrobat Compare).
- Compare = Acrobat Compare Files + Word Compare + GitHub: two pages side by side with synced scroll; change list on the
  right grouped by page and type (Text, Images, Annotations, Pages, Form fields, Metadata) with filters; clicking scrolls
  both sides; text changes as GitHub's word-level inline diff; page-level summary at the top ("Page 3 inserted",
  "Pages 5-6 moved to 9").
This overrides the round's "no red/green" line for diff marks only; the rest of the UI keeps one ink-blue accent.

Render notes: codex's copy step truncated b and c; they were recovered from ~/.codex/generated_images. Slips to
ignore (the build sets all text and icons): red PDF tab icons (a, b), a sparkle on "Assistant" (b), invented copy and
page counts ("/ 12", "Final version", "Restore this change"; Restore always restores a whole version), bright blue
instead of ink blue #2E4B8F.

## Shared

Render: a flat screenshot of one Windows 11 desktop application window, 1440 x 960, seen straight on, filling the
image with a thin margin of plain desktop around it. It is a PDF app called Inkwell with the document "Lease
agreement.pdf" open and its Version history open. It must look like a real, shipping native desktop app, not a web
page.
Window: Windows 11 style with rounded window corners; a slim title bar with a small ink-drop mark, a tab
"Lease agreement.pdf" (active, rounded) and a "+" button, and the window controls on the right. Under it a slim
toolbar with rounded icon buttons (sidebar, zoom, page indicator "1 / 5" in a small rounded field), and on the right
a rounded clock button shown active (version history) next to an "Assistant" button.
Document: a lease agreement on white paper with a soft shadow: title "Residential Lease Agreement", short clauses,
a line "Tenant: A. Novak" and a line "Start date: 1 November 2026".
Shape language: everything rounded and smooth: 8-12 px radius on panels, rows, chips, buttons; soft diffuse shadows;
no sharp corners anywhere.
Palette: window #F4F2ED; raised surfaces white #FFFFFF; panel #FAF8F4; text #1A1C20; secondary text #6B675F;
borders #E3DED4; one accent, ink blue #2E4B8F, used sparingly. Changed regions on the page are soft ink-blue rounded
outlines with a very light blue tint. No other hues: no red, no green.
Type: interface in a smooth humanist sans like Segoe UI Variable; small, crisp, well-spaced like a polished native
app; times and page numbers in tabular figures.
The first attached image is the style reference: match its window, colours, type and rounded shapes; do not copy its
assistant content.
Must not appear: git branch graphs, commit hashes, code-diff red and green, sparkle or magic icons, robot icons,
purple, violet, gradients, glow, glass, avatars, emoji, square corners, browser chrome, device frame, watermark,
extra text.

## a-timeline
ref: ../../approved/app-assistant.png
The version history is a docked right panel about 400 px wide, a rounded white surface floating 8 px from the window
edges with a soft shadow, like a native side panel. Header "Version history" with a close glyph, and under it a small
rounded segmented filter "All · You · Assistant · Outside" with All selected. A "Pinned" group with one row
"Signed by tenant", a tiny bookmark glyph and "2 Oct". Then day headings "Today" and "Yesterday" in small secondary
text, and the versions as a quiet vertical timeline: a thin line on the left with a small round dot per version.
Each row: label in medium weight, under it the author word ("Assistant", "You", "Outside Inkwell") and the time in
secondary text, and small rounded page chips "p. 1" "p. 3" on the right. The row "Filled tenant details" is selected
(light ink-blue tint, rounded) and expanded: its four change lines, each with a page chip, the first one hovered,
then a row of rounded buttons: "Restore" (ink-blue filled), "Save a copy", "Name this version", and a text link
"Compare with current". The document on the left shows page 1 with the "Tenant: A. Novak" and "Start date" lines
outlined by soft ink-blue rounded boxes.

## b-compare
ref: ../../approved/app-assistant.png
A full-window compare view replaces the reading view. The document area shows page 1 twice, side by side, labelled
in small rounded tags above each page: "Signed by tenant · 2 Oct" on the left and "Filled tenant details · Today
14:32" on the right. On the left page the tenant line is blank, on the right it reads "Tenant: A. Novak"; the changed
lines on both pages are outlined by soft ink-blue rounded boxes, and a thin curved connector joins each pair. Above
the pages a rounded toolbar: "Side by side | Slider | Overlay" segmented pill with Side by side selected, a
"Show differences" toggle switched on, and "Previous change" / "Next change" arrows with "1 of 4". Along the bottom of
the window a horizontal version track: a thin rounded rail with small dots for each version grouped under "Yesterday"
and "Today", two of them highlighted as the compared pair with small rounded handles. On the right a narrow rounded
panel lists the four changes with page chips, and at its bottom "Restore" (ink-blue filled) and "Save a copy".

## c-slider
ref: ../../approved/app-assistant.png
The version history is a docked right panel about 380 px wide (rounded white surface, soft shadow). Instead of text
rows, each version shows a small page thumbnail of the page it changed, with the changed region marked by a tiny
ink-blue rounded outline, and to the right of it the label, author word and time. Versions are grouped under
"Pinned", "Today" and "Yesterday". The selected version "Filled tenant details" is raised with a soft shadow and an
ink-blue rounded border, with a compact action row under it: "Restore" (ink-blue filled), "Save a copy", "Name",
and a "..." button. The document on the left shows page 1 with a vertical before/after slider across it: left of a
thin ink-blue rounded handle with a small grip, the page as it was (tenant line blank), right of it the page as it is
now (Tenant: A. Novak); small rounded tags "Before · 2 Oct" and "After · Today 14:32" sit at the top of each side.
A small floating rounded pill under the page: "Show differences" toggle and "1 of 4 changes" with arrows.
