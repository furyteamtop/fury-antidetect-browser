import { useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "../i18n";

export interface Command {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
}

/** ⌘K.
 *
 *  Not a second search box — the toolbar already filters the table. This is for
 *  the other half of the job: reaching a profile without touching the mouse.
 *  Someone running fifty accounts types three letters and presses Enter, and
 *  the browser opens; the table is for looking, this is for going.
 *
 *  It ranks by where the match falls rather than by score. A profile whose name
 *  *starts* with what was typed is almost always the one meant, and burying it
 *  under six proxies that happen to contain the same substring is how a palette
 *  becomes something people stop using. */
export function CommandPalette({
  actions,
  onClose,
}: {
  /** Everything that can be done, already carrying what to do. Building the
   *  list here from profiles *and* taking actions separately produced every
   *  profile twice — once as a row that did nothing. One source. */
  actions: Command[];
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const items = useMemo<Command[]>(() => {
    const q = query.trim().toLowerCase();
    if (!q) return actions.slice(0, 12);

    return actions
      .map((c) => {
        const haystack = `${c.label} ${c.hint ?? ""}`.toLowerCase();
        const at = haystack.indexOf(q);
        if (at < 0) return null;
        // 0 = the label starts with it, 1 = the label contains it, 2 = only the
        // hint does.
        const rank = c.label.toLowerCase().startsWith(q) ? 0 : c.label.toLowerCase().includes(q) ? 1 : 2;
        return { c, rank, at };
      })
      .filter((x): x is { c: Command; rank: number; at: number } => x !== null)
      .sort((a, b) => a.rank - b.rank || a.at - b.at)
      .slice(0, 12)
      .map((x) => x.c);
  }, [query, actions]);

  useEffect(() => setCursor(0), [query]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") return onClose();
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setCursor((c) => Math.min(c + 1, items.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setCursor((c) => Math.max(c - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const chosen = items[cursor];
        if (chosen) {
          chosen.run();
          onClose();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [items, cursor, onClose]);

  // Keep the highlighted row in view when arrowing past the fold.
  useEffect(() => {
    listRef.current?.children[cursor]?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  return (
    <div className="scrim paletteScrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-modal="true">
        <input
          className="paletteInput"
          value={query}
          autoFocus
          placeholder={t("cmd.placeholder")}
          onChange={(e) => setQuery(e.target.value)}
        />
        <div className="paletteList" ref={listRef}>
          {items.length === 0 && <div className="empty pad">{t("cmd.nothing")}</div>}
          {items.map((c, i) => (
            <button
              key={c.id}
              className={i === cursor ? "paletteRow on" : "paletteRow"}
              // Pointer selection follows the cursor rather than fighting it:
              // moving the mouse over a row should not re-order what Enter does.
              onMouseMove={() => setCursor(i)}
              onClick={() => {
                c.run();
                onClose();
              }}
            >
              <span className="ellipsis">{c.label}</span>
              {c.hint && <span className="muted small ellipsis">{c.hint}</span>}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
