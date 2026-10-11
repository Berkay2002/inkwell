const PATHS = {
  sidebar: "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2ZM9 3v18",
  outline: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14ZM20 20l-3.5-3.5",
  minus: "M5 12h14",
  plus: "M12 5v14M5 12h14",
  rotate: "M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5",
  up: "m18 15-6-6-6 6",
  down: "m6 9 6 6 6-6",
  right: "m9 18 6-6-6-6",
  close: "M18 6 6 18M6 6l12 12",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4",
  moon: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z",
  system: "M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2ZM8 21h8M12 17v4",
  command: "M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3",
  file: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8ZM14 3v5h5",
  open: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z",
  pin: "M12 17v5M9 10.8a2 2 0 0 1-1.1 1.8l-1.8.9A2 2 0 0 0 5 15.2V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.8a2 2 0 0 0-1.1-1.8l-1.8-.9A2 2 0 0 1 15 10.8V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1Z",
  grid: "M3 3h7v7H3ZM14 3h7v7h-7ZM14 14h7v7h-7ZM3 14h7v7H3Z",
  fileX: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8ZM14 3v5h5M9.5 12.5l5 5M14.5 12.5l-5 5",
  lock: "M5 11h14v10H5ZM8 11V7a4 4 0 0 1 8 0v4",
  upload: "M12 16V4M7 9l5-5 5 5M4 16v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3",
  home: "M3 11 12 3l9 8M5 9.5V21h5v-6h4v6h5V9.5",
  clock: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM12 7v5l3 2",
  settings: "M20 7h-9M14 17H5M17 20a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM7 10a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z",
  more: "M5 10.4a1.6 1.6 0 1 0 0 3.2 1.6 1.6 0 0 0 0-3.2ZM12 10.4a1.6 1.6 0 1 0 0 3.2 1.6 1.6 0 0 0 0-3.2ZM19 10.4a1.6 1.6 0 1 0 0 3.2 1.6 1.6 0 0 0 0-3.2Z",
  check: "m5 12.5 4.5 4.5L19 7.5",
  chat: "M20 14a2 2 0 0 1-2 2H8l-4 4V6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2Z",
  maximize: "M7 5h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2Z",
  restore: "M9 9V7a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-2M7 9h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2Z",
  organize: "m12 3 9 5-9 5-9-5ZM3 13l9 5 9-5",
  edit: "M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16ZM13.5 6.5l4 4",
  annotate: "m9 11-5 5v4h4l5-5M9 11l6-6 4 4-6 6ZM14 20h6",
  fill: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8ZM14 3v5h5M9 13h6M9 17h4",
  redact: "M4 5h16M4 19h10M4 10.9h16v2.2H4Z",
  ocr: "M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3M9 16l3-8 3 8M10 13.5h4",
  compress: "M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5",
  merge: "M9 3h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2M7 7h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2Z",
  rotateLeft: "M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5",
  trash: "M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3",
  share: "M12 15V3M8 7l4-4 4 4M6 11H5a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-8a1 1 0 0 0-1-1h-1",
  filePlus: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8ZM14 3v5h5M12 11v6M9 14h6",
  scissors: "M6 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6ZM6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6ZM8.1 8.1 20 20M14.5 9.5 20 4M8.1 15.9l3.4-3.4",
  swap: "M4 8h15M15 4l4 4-4 4M20 16H5M9 12l-4 4 4 4",
  crop: "M7 2v15h15M2 7h15v15",
  flipH: "M12 3v18M8 7 3 12l5 5V7ZM16 7l5 5-5 5V7Z",
  flipV: "M3 12h18M7 8l5-5 5 5H7ZM7 16l5 5 5-5H7Z",
  link: "M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7",
  book: "M3 5.5C5.5 4 9 4 12 6c3-2 6.5-2 9-.5V19c-2.5-1.5-6-1.5-9 .5-3-2-6.5-2-9-.5ZM12 6v13.5",
  undo: "M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11",
  area: "M4 7V4h3M10 4h4M17 4h3v3M20 10v4M20 17v3h-3M14 20h-4M7 20H4v-3M4 14v-4",
  textT: "M5 5h14M12 5v14M9 19h6",
  redo: "m15 14 5-5-5-5M20 9H9.5a5.5 5.5 0 0 0 0 11H13",
  cursor: "m5 3 14 7-6 2-2 6Z",
  highlight: "m9 11-5 5v4h4l5-5M9 11l6-6 4 4-6 6ZM4 21h16",
  underline: "M7 4v7a5 5 0 0 0 10 0V4M5 20h14",
  strike: "M5 12h14M16 7a4 4 0 0 0-4-2c-2.5 0-4 1.2-4 3s1.5 2.5 4 4 4 2.2 4 4-1.5 3-4 3a4 4 0 0 1-4-2",
  squiggle: "M7 4v7a5 5 0 0 0 10 0V4M4 20q2-2 4 0t4 0 4 0 4 0",
  note: "M5 4h14a1 1 0 0 1 1 1v10l-5 5H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1ZM15 20v-4a1 1 0 0 1 1-1h4",
  pen: "M3 17c3-8 5-10 7-8s-2 8 1 8 4-5 6-5 2 3 4 3",
  rect: "M4 6h16v12H4Z",
  ellipse: "M12 5c4.5 0 8 3 8 7s-3.5 7-8 7-8-3-8-7 3.5-7 8-7Z",
  line: "M5 19 19 5",
  arrow: "M5 19 19 5M10 5h9v9",
  polygon: "m12 3 8 6-3 11H7L4 9Z",
  stamp: "M9 14V9a3 3 0 1 1 6 0v5M5 14h14v4H5ZM5 21h14",
  reply: "M10 8 4 13l6 5M4 13h9a7 7 0 0 1 7 7",
  print: "M7 9V3h10v6M7 17H5a1 1 0 0 1-1-1v-5a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v5a1 1 0 0 1-1 1h-2M7 14h10v7H7Z",
  textField: "M3 7h18v10H3ZM7 10v4",
  checkSquare: "M4 4h16v16H4ZM8 12l3 3 5-6",
  radio: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
  dropdown: "M3 6h18v12H3ZM15 11l2 2 2-2",
  listBox: "M4 4h16v16H4ZM8 9h8M8 13h8M8 17h5",
  pushButton: "M3 8h18v8H3ZM8 12h8",
  signature: "M3 16c3-6 4-9 6-9s0 8 2 8 3-4 5-4 2 2 4 2M3 21h18",
  wand: "M15 4v3M13.5 5.5h3M19 11v2M18 12h2M5 20 15 10l-2-2L3 18ZM9 5v2M8 6h2",
  code: "m8 7-5 5 5 5M16 7l5 5-5 5M14 4l-4 16",
  tools: "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76Z",
  // a page with a half-filled circle on it (not the app theme's moon)
  pageDark: "M6 3h12a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1ZM12 9a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 9v8",
  paperclip: "m21 11-9 9a5.5 5.5 0 0 1-7.8-7.8l9-9a3.7 3.7 0 0 1 5.2 5.2l-9 9a1.8 1.8 0 0 1-2.6-2.6l8.2-8.2",
  present: "M4 4h16v11H4ZM12 15v5M8 20h8M10.5 7.5l4 2-4 2Z",
  download: "M12 4v11M7 10.5l5 5 5-5M5 20h14",
  grip: "M9 5.5v.01M15 5.5v.01M9 12v.01M15 12v.01M9 18.5v.01M15 18.5v.01",
  hash: "M5 9h14M5 15h14M10 4 8 20M16 4l-2 16",
  headerFooter: "M5 3h14a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1ZM4 8h16M4 16h16",
  background: "M5 3h14a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1ZM8 8h8v8H8Z",
  shield: "M12 3 5 6v5c0 4.6 2.9 8 7 10 4.1-2 7-5.4 7-10V6Z",
  // three overlapping plates (the inks), a block of swatches, a box inside a page
  plates: "M9 4a5 5 0 1 0 0 10 5 5 0 0 0 0-10ZM15 4a5 5 0 1 0 0 10 5 5 0 0 0 0-10ZM12 10a5 5 0 1 0 0 10 5 5 0 0 0 0-10Z",
  swatches: "M4 4h7v7H4ZM13 4h7v7h-7ZM4 13h7v7H4ZM13 13h7v7h-7Z",
  pagebox: "M4 3h16v18H4ZM8 7h8v10H8Z",
  access: "M12 3.5a1.8 1.8 0 1 0 0 3.6 1.8 1.8 0 0 0 0-3.6ZM5 9.5l7 1.5 7-1.5M12 11v4.5M8.5 20.5 12 15.5l3.5 5",
};

export type IconName = keyof typeof PATHS;

/** Parts of an icon that are filled in as well as stroked. */
const FILLS: Partial<Record<IconName, string>> = {
  pageDark: "M12 9a4 4 0 0 1 0 8Z",
};

export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={PATHS[name]} />
      {FILLS[name] && <path d={FILLS[name]} fill="currentColor" />}
    </svg>
  );
}
