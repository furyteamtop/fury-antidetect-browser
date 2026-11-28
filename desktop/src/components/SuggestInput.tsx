// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

/** A text field with suggestions, drawn by the application, in place of
 *  `<input list>` with a `<datalist>`.
 *
 *  Same reason as Select.tsx: the datalist's menu is the platform's, a system
 *  popover over a dark window. The field stays free text -- a stage can be a
 *  word of one's own, a time zone can be typed -- and the list under it is
 *  ours, filtered by what has been typed so far. A suggestion carries the
 *  value it puts in the field and, optionally, the words it is shown with. */
export function SuggestInput({
  id,
  value,
  onChange,
  suggestions,
  placeholder,
}: {
  id?: string;
  value: string;
  onChange: (value: string) => void;
  suggestions: { value: string; label?: string }[];
  placeholder?: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [place, setPlace] = useState<{ left: number; top: number; width: number; maxHeight: number; up: boolean } | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const listId = useId();

  // A suggestion with words of its own is shown by them: a stage stored as
  // "warming" reads "в прогреве" in the field, as it does in the list (the
  // field used to show the stored word, 14.10.2026). Typing those words
  // stores the value, so a stage typed by hand is the same stage.
  const key = value.trim().toLowerCase();
  const own = suggestions.find((s) => s.label && s.value.toLowerCase() === key);
  const shownText = own?.label ?? value;
  const emit = (text: string) => {
    const t = text.trim().toLowerCase();
    const match = t ? suggestions.find((s) => (s.label ?? s.value).toLowerCase() === t) : undefined;
    onChange(match ? match.value : text);
  };

  // Everything while the field shows exactly one of them or nothing, so the
  // list is a menu to pick from; narrowed by what is typed otherwise.
  const q = shownText.trim().toLowerCase();
  const exact = suggestions.some((s) => s.value.toLowerCase() === key);
  const shown = (q && !exact
    ? suggestions.filter((s) => s.value.toLowerCase().includes(q) || (s.label ?? "").toLowerCase().includes(q))
    : suggestions
  ).slice(0, 200);

  const choose = (i: number) => {
    const s = shown[i];
    if (!s) return;
    onChange(s.value);
    setOpen(false);
  };

  useLayoutEffect(() => {
    if (!open || !input.current) return;
    const r = input.current.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - 8;
    const above = r.top - 8;
    const want = Math.min(280, shown.length * 30 + 10);
    const up = below < want && above > below;
    setPlace({ left: r.left, top: up ? r.top - 4 : r.bottom + 4, width: r.width, maxHeight: Math.max(120, Math.min(280, up ? above : below)), up });
  }, [open, shown.length]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (list.current?.contains(t) || input.current?.contains(t)) return;
      setOpen(false);
    };
    const scrolled = (e: Event) => {
      if (list.current && e.target instanceof Node && list.current.contains(e.target)) return;
      setOpen(false);
    };
    const resized = () => setOpen(false);
    document.addEventListener("mousedown", away, true);
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("resize", resized);
    return () => {
      document.removeEventListener("mousedown", away, true);
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("resize", resized);
    };
  }, [open]);

  useEffect(() => {
    if (!open || active < 0) return;
    list.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [open, active]);

  return (
    <>
      <input
        ref={input}
        id={id}
        value={shownText}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        role="combobox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-autocomplete="list"
        aria-activedescendant={open && active >= 0 ? `${listId}-${active}` : undefined}
        onFocus={() => {
          setActive(-1);
          setOpen(true);
        }}
        onMouseDown={() => !open && setOpen(true)}
        onChange={(e) => {
          emit(e.target.value);
          setActive(-1);
          setOpen(true);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            if (!open) setOpen(true);
            setActive((a) => Math.min(shown.length - 1, a + 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setActive((a) => Math.max(0, a - 1));
          } else if (e.key === "Enter" && open && active >= 0) {
            e.preventDefault();
            choose(active);
          } else if (e.key === "Escape" && open) {
            e.preventDefault();
            e.stopPropagation();
            setOpen(false);
          } else if (e.key === "Tab") {
            setOpen(false);
          }
        }}
      />
      {open &&
        place &&
        shown.length > 0 &&
        createPortal(
          <div
            ref={list}
            id={listId}
            role="listbox"
            className="selectPop selectList"
            style={{
              left: place.left,
              minWidth: place.width,
              maxHeight: place.maxHeight,
              ...(place.up ? { bottom: window.innerHeight - place.top } : { top: place.top }),
            }}
          >
            {shown.map((s, i) => (
              <div
                key={s.value}
                id={`${listId}-${i}`}
                data-index={i}
                role="option"
                aria-selected={s.value.toLowerCase() === key}
                className={"selectOption" + (i === active ? " active" : "")}
                onMouseMove={() => active !== i && setActive(i)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => choose(i)}
              >
                <span className="selectCheck">{s.value.toLowerCase() === key ? "✓" : ""}</span>
                <span className="selectLabel">{s.label ?? s.value}</span>
              </div>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
