// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import {
  Children,
  Fragment,
  isValidElement,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

/** A dropdown drawn by the application, in place of `<select>`.
 *
 *  A native select opens the platform's own menu: on macOS a translucent
 *  system popover with a blue check, nothing like the window it opens over
 *  (the owner, 14.10.2026, on the invitation form). The control could be
 *  restyled; the menu cannot.
 *
 *  So the menu is ours, and the API is the select's: `<option>` children,
 *  `value`, and an `onChange` whose argument has `target.value`. Every call
 *  site moved over by changing the tag, which kept the change mechanical
 *  across thirty dropdowns and their handlers untouched.
 *
 *  The list is portalled to the body with fixed coordinates, because the
 *  dropdowns live in scrolling panes and in dialogs that clip. It opens
 *  downwards, or upwards when there is not room. Keyboard: arrows, Home/End,
 *  Enter or Space to choose, Escape to close, letters to jump. */

/** `heading` is an `<optgroup>`'s label: shown in the list, never chosen. */
type Opt = { value: string; label: string; disabled: boolean; heading?: boolean };

export interface SelectChange {
  target: { value: string };
  currentTarget: { value: string };
}

function text(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(text).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return text(node.props.children);
  return "";
}

/** The `<option>`s, wherever they are: in arrays from a map, in fragments,
 *  behind a `cond && (...)`. */
function collect(children: ReactNode, out: Opt[] = []): Opt[] {
  Children.forEach(children, (child) => {
    if (!isValidElement<{ value?: unknown; disabled?: boolean; children?: ReactNode }>(child)) return;
    if (child.type === Fragment) {
      collect(child.props.children, out);
      return;
    }
    // A group: its label as a heading, then its options. The value is one no
    // option can have, so a heading is never the selected row.
    if (child.type === "optgroup") {
      const label = String((child.props as { label?: unknown }).label ?? "");
      out.push({ value: `\u0000group:${label}`, label, disabled: true, heading: true });
      collect(child.props.children, out);
      return;
    }
    if (child.type === "option") {
      const label = text(child.props.children);
      out.push({
        value: child.props.value === undefined ? label : String(child.props.value),
        label,
        disabled: !!child.props.disabled,
      });
    }
  });
  return out;
}

export function Select({
  value,
  onChange,
  children,
  disabled,
  id,
  className,
  style,
  title,
  autoFocus,
  searchable = false,
  searchPlaceholder,
  emptySearchText,
  "aria-label": ariaLabel,
}: {
  value?: string | number;
  onChange?: (e: SelectChange) => void;
  children?: ReactNode;
  disabled?: boolean;
  id?: string;
  className?: string;
  style?: CSSProperties;
  title?: string;
  autoFocus?: boolean;
  /** A filter for long, saved-item lists. The native type-ahead remains for
   * ordinary selects, whose short menus do not need a field of their own. */
  searchable?: boolean;
  searchPlaceholder?: string;
  emptySearchText?: string;
  "aria-label"?: string;
}) {
  const options = collect(children);
  const current = value === undefined ? (options[0]?.value ?? "") : String(value);
  const selected = options.findIndex((o) => o.value === current);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [search, setSearch] = useState("");
  const [place, setPlace] = useState<{ left: number; top: number; width: number; maxHeight: number; up: boolean } | null>(null);
  const button = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const typed = useRef({ text: "", at: 0 });
  const listId = useId();

  const shown = (() => {
    const query = searchable ? search.trim().toLowerCase() : "";
    if (!query) return options;
    const filtered: Opt[] = [];
    let heading: Opt | null = null;
    for (const option of options) {
      if (option.heading) {
        heading = option;
      } else if (option.label.toLowerCase().includes(query)) {
        if (heading) filtered.push(heading);
        heading = null;
        filtered.push(option);
      }
    }
    return filtered;
  })();

  const choose = (i: number) => {
    const o = shown[i];
    if (!o || o.disabled) return;
    setOpen(false);
    setSearch("");
    button.current?.focus();
    if (o.value !== current) onChange?.({ target: { value: o.value }, currentTarget: { value: o.value } });
  };

  const step = (from: number, dir: 1 | -1) => {
    for (let i = from + dir; i >= 0 && i < shown.length; i += dir) {
      if (!shown[i].disabled) return i;
    }
    return from;
  };

  const show = () => {
    if (disabled || options.length === 0) return;
    setSearch("");
    setActive(selected >= 0 ? selected : step(-1, 1));
    setOpen(true);
  };

  // A selected proxy remains selected while filtering. The active menu row
  // follows it when it matches, or the first match when it does not.
  useEffect(() => {
    if (!open) return;
    const matching = shown.findIndex((o) => o.value === current);
    setActive(matching >= 0 ? matching : step(-1, 1));
  // `shown` is rebuilt from the option children and search text.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, search, current, options.length]);

  useEffect(() => {
    if (open && searchable) searchInput.current?.focus();
  }, [open, searchable, place]);

  // Placed before paint, so the list never flashes at the wrong spot.
  useLayoutEffect(() => {
    if (!open || !button.current) return;
    const r = button.current.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - 8;
    const above = r.top - 8;
    const want = Math.min(320, shown.length * 30 + (searchable ? 48 : 10));
    const up = below < want && above > below;
    setPlace({
      left: r.left,
      top: up ? r.top - 4 : r.bottom + 4,
      width: r.width,
      maxHeight: Math.max(120, Math.min(320, up ? above : below)),
      up,
    });
  }, [open, shown.length, searchable]);

  // Closed by anything that would leave it floating over the wrong place.
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (popup.current?.contains(t) || button.current?.contains(t)) return;
      setOpen(false);
    };
    const scrolled = (e: Event) => {
      if (popup.current && e.target instanceof Node && popup.current.contains(e.target)) return;
      setOpen(false);
    };
    const resized = () => setOpen(false);
    document.addEventListener("mousedown", away, true);
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("resize", resized);
    window.addEventListener("blur", resized);
    return () => {
      document.removeEventListener("mousedown", away, true);
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("resize", resized);
      window.removeEventListener("blur", resized);
    };
  }, [open]);

  // The active row stays in view as the arrows move it.
  useEffect(() => {
    if (!open || active < 0) return;
    list.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [open, active, place]);

  const onKey = (e: React.KeyboardEvent<HTMLElement>) => {
    if (disabled) return;
    const typing = e.currentTarget === searchInput.current;
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show();
      }
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        setActive((a) => step(a, 1));
        return;
      case "ArrowUp":
        e.preventDefault();
        setActive((a) => step(a, -1));
        return;
      // In the search field Home and End move the caret, as in any text box.
      case "Home":
        if (typing) return;
        e.preventDefault();
        setActive(step(-1, 1));
        return;
      case "End":
        if (typing) return;
        e.preventDefault();
        setActive(step(shown.length, -1));
        return;
      case "Enter":
        e.preventDefault();
        choose(active);
        return;
      case " ":
        if (typing) return;
        e.preventDefault();
        choose(active);
        return;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
        setSearch("");
        if (typing) button.current?.focus();
        return;
      case "Tab":
        // The search field is portaled to the end of <body>; Tab from there
        // would leave the dialog. Back to the button first, so Tab moves on
        // from where the list was opened.
        if (typing) button.current?.focus();
        setOpen(false);
        setSearch("");
        return;
    }
    // Type to jump, as a native menu does.
    if (!typing && e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
      const now = Date.now();
      const t = typed.current;
      t.text = (now - t.at < 700 ? t.text : "") + e.key.toLowerCase();
      t.at = now;
      const i = shown.findIndex((o) => !o.disabled && o.label.toLowerCase().startsWith(t.text));
      if (i >= 0) setActive(i);
    }
  };

  const label = selected >= 0 ? options[selected].label : "";

  return (
    <>
      <button
        ref={button}
        type="button"
        id={id}
        className={"selectBtn" + (className ? " " + className : "")}
        style={style}
        title={title}
        disabled={disabled}
        autoFocus={autoFocus}
        role="combobox"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open && active >= 0 ? `${listId}-${active}` : undefined}
        onMouseDown={(e) => {
          // Toggled here rather than on click, so the press that opens the
          // list is not also read as a click away from it.
          if (e.button !== 0 || disabled) return;
          e.preventDefault();
          button.current?.focus();
          if (open) setOpen(false);
          else show();
        }}
        onKeyDown={onKey}
      >
        {/* Every label in one grid cell, all but the current one hidden: the
            control is as wide as its longest option, as a native select is,
            and does not change width as the choice changes. */}
        <span className="selectSizer">
          {options.map((o, i) => (
            <span key={i} aria-hidden={i !== selected} className={i === selected ? undefined : "selectGhost"}>
              {o.label || " "}
            </span>
          ))}
          {selected < 0 && <span>{label || " "}</span>}
        </span>
      </button>
      {open &&
        place &&
        createPortal(
          <div
            ref={popup}
            className="selectPop"
            style={{
              left: place.left,
              minWidth: place.width,
              maxHeight: place.maxHeight,
              ...(place.up ? { bottom: window.innerHeight - place.top } : { top: place.top }),
            }}
          >
            {searchable && (
              <input
                ref={searchInput}
                className="selectSearch"
                value={search}
                placeholder={searchPlaceholder}
                aria-label={searchPlaceholder}
                role="combobox"
                aria-controls={listId}
                aria-expanded="true"
                aria-activedescendant={active >= 0 ? `${listId}-${active}` : undefined}
                onChange={(e) => setSearch(e.target.value)}
                onKeyDown={onKey}
                onBlur={() => {
                  requestAnimationFrame(() => {
                    if (!popup.current?.contains(document.activeElement)) setOpen(false);
                  });
                }}
              />
            )}
            <div ref={list} id={listId} role="listbox" className="selectList">
              {shown.length === 0 && <div className="selectEmpty">{emptySearchText}</div>}
              {shown.map((o, i) =>
                o.heading ? (
                  <div key={i} role="presentation" className="selectHeading">
                    {o.label}
                  </div>
                ) : (
                <div
                  key={i}
                  id={`${listId}-${i}`}
                  data-index={i}
                  role="option"
                  aria-selected={o.value === current}
                  aria-disabled={o.disabled || undefined}
                  className={"selectOption" + (i === active ? " active" : "") + (o.disabled ? " disabled" : "")}
                  onMouseMove={() => !o.disabled && active !== i && setActive(i)}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => choose(i)}
                >
                  <span className="selectCheck">{o.value === current ? "✓" : ""}</span>
                  <span className="selectLabel">{o.label || " "}</span>
                </div>
              ),
              )}
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
