# App-assistant r1: the margin

The AI Assistant side panel next to an open PDF. The previous attempt (an earlier build, docs/screens/assistant/ on its
branch) read as generic and AI-made. Why: a sparkle icon, a chat-bubble column with a lilac user bubble, a list of grey citation rows in rounded boxes
with purple number pills, pill-shaped suggestion chips, a model and effort dropdown pair in the header, a filled
"Send" button. It is every AI chat sidebar. Kept: what it does (ask about the document, summarise, explain a
selection, answers whose sources are numbered and jump to the quoted lines, three follow-up questions, model
choice, "Sent to OpenAI through your Codex login. Chats are not saved."). Reopened: the whole look. This round is
rendered in parallel with home-r1 using the same palette and type; once home is approved, the pick is re-rendered
with it attached so the two match.

Idea: the assistant writes in the document's margin, like a careful reader's annotations, and every claim is
pinned to the line it came from.
For: someone reading a long report who wants answers they can check. Feel: scholarly, exact, calm, trustworthy.
Palette and type: as home-r1 (paper #F3F0E8, ink #17191D, graphite #6E6A62, hairline #D8D2C4, one accent iron-gall
blue-black #22386B; Newsreader-like serif, Inter-like grotesk, Plex-Mono-like monospace).
Copy:
- Document: "Northwind Robotics, FY2025 Annual Report", page 2 "Financial highlights" with a small table
  (Revenue $129M / $182M, Gross margin 31% / 38%, Operating loss $26.1M / $9.4M, Cash $61M / $74M).
- Question: "Why did gross margin improve, and how long can the company run on its cash?"
- Answer: "Gross margin rose from 31% to 38%, driven by the move to Gdansk and a better product mix [1]. Cash of
  $74 million covers roughly 29 months at the current burn rate [2]."
- Sources: "[1] p. 2 — driven by the Gdansk move and a more favourable product mix"; "[2] p. 2 — enough for
  roughly 29 months at the current burn rate".
- Follow-ups: "How did operating loss change?" / "What is the 2026 revenue guidance?" / "What risks does the company name?"
- Input placeholder: "Ask about this document". Model label: "GPT-6.1 Sol · low". Footer: "Sent to OpenAI through
  your Codex login. Chats are not saved."
Motion (for the build): the answer writes in as it streams, line by line, like ink; when a source is clicked the
page scrolls and a hand-drawn underline is drawn under the quoted line (rendered strokes, played as frames).
Reduced motion: underline shown settled.
Must not appear: sparkle or star icons, robot or brain icons, chat bubbles, avatars, lilac/purple, pill chips,
rounded grey boxes, gradients, glow, glass, typing dots, emoji.

Verdict (2026-10-10): rejected: it read as a website, not a desktop app; take reference from similar apps, with rounded, smooth components and corners (the assistant UI too). Next round: real desktop-app chrome modelled on comparable PDF apps, rounded and smooth components.

## Shared

Render: a flat screenshot of one desktop application window at 1440 px wide, seen straight on, no OS title bar,
no browser chrome, no device frame. It is a PDF reader called Inkwell with a document open and its assistant panel
on the right.
Palette: warm paper ground #F3F0E8 for the app; the PDF page itself is bright white paper; ink #17191D for text;
graphite #6E6A62 for secondary text and thin rules; hairlines #D8D2C4; one accent, iron-gall blue-black #22386B,
used at most once in each area. No other hues.
Type: headings, the answer text and document names in a sharp transitional serif like Newsreader; interface text in
a plain grotesk like Inter; small labels, page numbers, model name and keyboard hints in a small monospace like
IBM Plex Mono.
The open document, on the left two thirds: page 2 of "Northwind Robotics, FY2025 Annual Report", titled "Financial
highlights", a paragraph, and a small four-row table (Revenue $129M / $182M, Gross margin 31% / 38%, Operating loss
$26.1M / $9.4M, Cash $61M / $74M). A thin toolbar above it with only small text: "Annual report.pdf", "2 / 6",
"Fit width", and on the right "Assistant" in ink, active.
Light and depth: soft daylight from the upper left; the PDF page is a real sheet with a slight contact shadow.
The interface is flat type on the paper ground with hairline rules; no boxes.
Must not appear: sparkle or star icons, robot or brain icons, chat bubbles, avatars, purple or lilac, pill-shaped
chips or buttons, rounded grey boxes, gradients, glow, glass, typing dots, emoji, logos, watermarks, extra text.

## a-margin
The assistant panel is the page's outer margin, about a third of the width, separated from the document by a single
hairline, same paper ground, no header bar. At the top in small monospace: "ASSISTANT" on the left, "GPT-6.1 Sol ·
low" on the right. The question appears as a short italic serif line in graphite with a thin rule on its left,
like a quoted note: "Why did gross margin improve, and how long can the company run on its cash?". Below it the
answer in the serif, ink, comfortable reading size, with small superscript source numbers 1 and 2 in the
blue-black accent. Below the answer, under a hairline, the two sources as footnotes in small type: "1  p. 2 —
driven by the Gdansk move and a more favourable product mix", "2  p. 2 — enough for roughly 29 months at the
current burn rate". On the document, the quoted line for source 1 has a hand-drawn blue-black ink underline and a
tiny superscript 1 in the margin beside it. Below the footnotes, three follow-up questions as plain text lines
each starting with an en dash. At the bottom a single hairline input line, "Ask about this document", with
"Enter" in small monospace at its right end, and under it in tiny graphite "Sent to OpenAI through your Codex
login. Chats are not saved."

## b-notebook
The assistant panel on the right is a separate sheet of slightly warmer, lightly ruled notebook paper lying beside
the PDF page, a little overlapping the app's paper ground, with its own soft shadow, as if a reading notebook were
open next to the report. On it, written in the serif as neat typeset notes: a small monospace date-like header
"NOTES ON ANNUAL REPORT.PDF" and "GPT-6.1 Sol · low"; the question as a heading in italic; the answer as a short
paragraph; the two sources as numbered references with page numbers in the left margin of the notebook ("p. 2");
then "Next questions" in small monospace with the three follow-ups listed. A thin ink-blue line runs from source 1's
page number across the gap to the underlined quote on the PDF page, the only use of the accent. The input at the
bottom of the notebook is an empty ruled line with "Ask about this document" faint on it. Footer text tiny under it.

## c-ledger
The panel on the right is set like a scholarly two-column apparatus. Upper part: the answer in the serif as a
compact paragraph with numbered markers. Lower part: a "SOURCES" index in small monospace, then each source as an
exact excerpt of the document line, set as a tiny reproduction of the page's own typography (as if cut from the
page), with "p. 2" beside it in monospace; the first one shown with a thin blue-black rule on its left as the
selected one, and on the PDF page that same line is outlined by a thin hand-drawn ink bracket in the left margin.
The question sits above the answer as a single line in small caps grotesk. Model "GPT-6.1 Sol · low" in tiny
monospace at the top right of the panel. The three follow-ups as a single line of three phrases separated by
middle dots. A bottom input line "Ask about this document" with "Ctrl Enter" in monospace, and the tiny footer.
