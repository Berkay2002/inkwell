# App-assistant r2: rounded, native assistant panel

Round 1 (margin / notebook / ledger) was rejected together with home-r1: it read as a website, not a desktop app;
the ask was to take reference from similar apps, with rounded, smooth components and corners. R1 drew the panel as editorial print (hairlines, footnotes, no controls). R2 is a
real desktop app's assistant: rounded, smooth surfaces and controls, modelled on comparable apps' AI side panels
(Acrobat AI Assistant, Edge/Copilot sidebar, Notion AI, Arc, Raycast AI), but without their sparkle/purple clichés.
Kept from r1: the paper-and-ink palette and one ink-blue accent; numbered sources that jump to and highlight the
quote on the page (r1 a's ink underline on the quoted line is worth keeping); follow-up questions; model + effort.
Rendered in parallel with home-r2 using the same shared window, palette and shape language.

Idea: an assistant that answers about the open PDF and always shows where on the page the answer came from.
For: someone reading a long report who wants answers they can check. Feel: soft, calm, trustworthy, native.
Palette/type/shape: as home-r2.
Copy: document "Annual report.pdf", page 2 "Financial highlights" with a table (Revenue $129M / $182M, Gross margin
31% / 38%, Operating loss $26.1M / $9.4M, Cash $61M / $74M). Question: "Why did gross margin improve, and how long can
the company run on its cash?" Answer: "Gross margin rose from 31% to 38%, driven by the move to Gdansk and a better
product mix [1]. Cash of $74 million covers roughly 29 months at the current burn rate [2]." Sources: "p. 2 · driven
by the Gdansk move and a more favourable product mix"; "p. 2 · enough for roughly 29 months at the current burn rate".
Follow-ups: "How did operating loss change?", "What is the 2026 revenue guidance?", "What risks does the company name?".
Composer placeholder "Ask about this document"; model "GPT-6.1 Sol" with effort "Low"; footer "Sent to OpenAI
through your Codex login. Chats are not saved."
Motion (for the build): the answer streams in word by word with a soft fade; clicking a source scrolls the page and
draws a soft rounded highlight on the quoted line; panel slides in 240 ms ease-out. Reduced motion: no slide/fade.
Must not appear: sparkle/star/magic icons, robot/brain icons, purple/violet/lilac, gradients, glow, avatars, emoji,
typing dots, sharp square corners.

Verdict (2026-10-10): approved all three as modes. User: "All should be available because all makes sense based on preference. But docked should probably be default, but it can then be floatable or if you want more focus on the actual PDF etc, you can have it as a c-command." Build: docked (a) default; float (b) and command bar (c) as switchable modes. Approved renders: a, b and c. Ignore render slips: "Claude 3.5 Sonnet" label, bright Windows blue (use ink blue #2E4B8F).

## Shared

Render: a flat screenshot of one Windows 11 desktop application window, 1440 x 960, seen straight on, filling the
image with a thin margin of plain desktop around it. It is a PDF app called Inkwell with a document open and its
assistant panel open on the right. It must look like a real, shipping native desktop app, not a web page.
Window: Windows 11 style with rounded window corners; a slim title bar with a small ink-drop mark, a tab
"Annual report.pdf" (active, rounded) and a "+" button, and the window controls on the right. Under it a slim
toolbar with rounded icon buttons (sidebar, zoom out, "Fit width" rounded dropdown, zoom in, rotate), page indicator
"2 / 6" in a small rounded field, and on the right a rounded "Assistant" toggle button shown active.
Document: left part of the window, the PDF page on white paper with a soft shadow, page 2 "Financial highlights",
a paragraph, and a small neatly set table (Revenue $129M / $182M, Gross margin 31% / 38%, Operating loss $26.1M /
$9.4M, Cash $61M / $74M). The sentence that source 1 quotes has a soft rounded ink-blue highlight behind it with a
tiny rounded "1" badge at its start.
Shape language: everything rounded and smooth: 8-12 px corner radius on panels, messages, chips, buttons and inputs;
pill-rounded composer; soft, diffuse shadows; no sharp corners anywhere.
Palette: window #F4F2ED; raised surfaces white #FFFFFF; panel #FAF8F4; text #1A1C20; secondary text #6B675F;
borders #E3DED4; one accent, ink blue #2E4B8F (source numbers, send button, active toggle), used sparingly.
No other hues.
Type: interface in a smooth humanist sans like Segoe UI Variable; small, crisp, well-spaced like a polished native app.
Must not appear: sparkle, star or magic-wand icons, robot or brain icons, purple, violet, lilac, gradients, glow,
glass, avatars, emoji, typing dots, square corners, browser chrome, device frame, watermark, extra text.

## a-docked
The assistant is a docked right panel about 400 px wide, separated by a soft rounded inset (the panel is a rounded
white surface floating 8 px from the window edges, with a soft shadow). Panel header: "Assistant" in medium weight,
a rounded model picker "GPT-6.1 Sol · Low" and a small "..." and close glyph. The question as a rounded message
bubble aligned right in a very light warm grey. The answer as plain text on the panel (no bubble), with small round
ink-blue number badges 1 and 2 inline. Under the answer a "Sources" group: two rounded rows, each with a page badge
"p. 2" and the quoted snippet, the first one hovered. Below, three rounded follow-up chips in a wrapped row with a
thin border. Bottom: a pill-rounded composer "Ask about this document" with a round ink-blue send button inside it,
and the footer line in tiny secondary text under it.

## b-floating
The document fills the window; the assistant is a floating rounded panel (16 px radius, generous soft shadow) hovering
over the right side of the document area, about 380 px wide and most of the window height, like a native popover
sheet. At its top a segmented pill control "Chat | Summary" with Chat selected, and the model picker. The conversation
uses soft rounded cards: the question in a light card, the answer in a white card with inline round number badges,
and the sources as a horizontal row of two small rounded source cards, each with a tiny page thumbnail, "p. 2" and
the snippet. Follow-up questions as three stacked rounded rows with a small arrow. A pill composer at the bottom with
an attach-selection glyph on the left and a round send button on the right.

## c-command
The assistant lives in a pill-shaped command bar floating at the bottom centre of the document area (like a native
spotlight bar), with "Ask about this document" placeholder, a small rounded model chip "GPT-6.1 Sol · Low" and a round
send button. Above it, anchored to the right edge of the document area, a rounded answer sheet (12 px radius, soft
shadow, about 420 px wide) showing the question in small secondary text at its top, the answer with inline round
number badges, two rounded source rows with "p. 2" badges, and three follow-up chips; a thin rounded line connects
source 1 to the highlighted sentence on the page. Small "Copy" and "New chat" rounded text buttons at the sheet's
bottom right.
