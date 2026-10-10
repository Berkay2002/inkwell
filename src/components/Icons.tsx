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
  redact: "M12 3 5 6v5c0 4.5 3 8 7 10 4-2 7-5.5 7-10V6Z",
  ocr: "M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3M9 16l3-8 3 8M10 13.5h4",
  compress: "M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5",
  merge: "M9 3h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2M7 7h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2Z",
};

export type IconName = keyof typeof PATHS;

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
    </svg>
  );
}
