// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

/** The line icons the toolbars use.
 *
 *  Drawn here rather than pulled from a set, for one reason worth the fifty
 *  lines: an icon library is a dependency that ships hundreds of glyphs to draw
 *  eight, and this application is handed to people who are careful about what
 *  their browser talks to. Everything here is inline and nothing is fetched.
 *
 *  All of them are 24-unit strokes on `currentColor`, so a button decides the
 *  size and the colour and an icon never fights it.
 *
 *  THE RULE FOR USING THEM, because a row of unlabelled glyphs is worse than a
 *  row of long buttons: an icon carries an action somebody already understands
 *  — copy, delete, refresh — and never the one they came to do. The primary
 *  action keeps its words. Every icon button carries `title` and `aria-label`,
 *  so the name is a hover away and a screen reader is given a word rather than
 *  a shape. */
const P = {
  share:
    "M4 12v7a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-7M12 3v13M12 3 8 7M12 3l4 4",
  upload: "M12 20V8M12 4 7 9M12 4l5 5M4 20h16",
  copy: "M9 9h10v10a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V9ZM6 15H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v1",
  cookie:
    "M21 12a9 9 0 1 1-9-9 3 3 0 0 0 3 3 3 3 0 0 0 3 3 3 3 0 0 0 3 3ZM9 10h.01M8 15h.01M13 15h.01M15 11h.01",
  trash: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6",
  close: "M18 6 6 18M6 6l12 12",
  refresh: "M20 11a8 8 0 1 0-.6 4M20 5v6h-6",
  pencil: "M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z",
  /** This machine. A laptop, because "on this machine" is what it means and a
   *  house would mean home. */
  laptop: "M4 6a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v9H4V6ZM2 18h20M9 18h6",
  /** Two people: somebody else holds this too. */
  people:
    "M9 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM3 20a6 6 0 0 1 12 0M16 5.5a3 3 0 0 1 0 5.8M18 20a5.5 5.5 0 0 0-2-4.2",
} as const;

export type IconName = keyof typeof P;

export function Icon({ name, size = 15 }: { name: IconName; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.9"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={P[name]} />
    </svg>
  );
}

/** A square button holding one icon, named for people who cannot see it. */
export function IconButton({
  icon,
  label,
  onClick,
  disabled,
  danger,
  title,
}: {
  icon: IconName;
  /** What it does, in words. Shown on hover and read aloud. */
  label: string;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
  /** Overrides the hover text when there is more to say than the name —
   *  usually WHY it is disabled, which is the one thing a greyed button has to
   *  be able to explain. */
  title?: string;
}) {
  return (
    <button
      type="button"
      className={danger ? "icon danger" : "icon"}
      onClick={onClick}
      disabled={disabled}
      title={title ?? label}
      aria-label={label}
    >
      <Icon name={icon} />
    </button>
  );
}
