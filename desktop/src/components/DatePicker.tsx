// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { languageNames, useI18n } from "../i18n";

/** A date field with a calendar drawn by the application, in place of
 *  `<input type="date">`, whose calendar is the platform's: the last system
 *  popover left in the launcher after Select and SuggestInput (14.10.2026).
 *
 *  The value is what the date input gave, "YYYY-MM-DD" or "", so the filters
 *  it feeds did not change. Shown in the interface's language; the week
 *  starts on Monday in Russian and on Sunday in English, as the calendars of
 *  those locales do. Keyboard: arrows move a day or a week, PageUp/PageDown a
 *  month, Enter picks, Escape closes. */

const pad = (n: number) => String(n).padStart(2, "0");
const iso = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const parse = (v: string): Date | null => {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v);
  return m ? new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])) : null;
};
const addDays = (d: Date, n: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
const addMonths = (d: Date, n: number) => {
  const target = new Date(d.getFullYear(), d.getMonth() + n, 1);
  const last = new Date(target.getFullYear(), target.getMonth() + 1, 0).getDate();
  return new Date(target.getFullYear(), target.getMonth(), Math.min(d.getDate(), last));
};

export function DatePicker({
  value,
  onChange,
  placeholder,
  style,
  "aria-label": ariaLabel,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  style?: CSSProperties;
  "aria-label"?: string;
}) {
  const { t, resolved } = useI18n();
  const locale = languageNames[resolved].tag;
  const weekStart = resolved === "en" ? 0 : 1;
  const chosen = parse(value);
  const [open, setOpen] = useState(false);
  // The day the keyboard is on; the month shown is the one it is in.
  const [cursor, setCursor] = useState<Date>(chosen ?? new Date());
  const [place, setPlace] = useState<{ left: number; top: number; up: boolean } | null>(null);
  const button = useRef<HTMLButtonElement>(null);
  const pop = useRef<HTMLDivElement>(null);

  const show = () => {
    setCursor(parse(value) ?? new Date());
    setOpen(true);
  };
  const pick = (d: Date | null) => {
    onChange(d ? iso(d) : "");
    setOpen(false);
    button.current?.focus();
  };

  useLayoutEffect(() => {
    if (!open || !button.current) return;
    const r = button.current.getBoundingClientRect();
    const height = 330;
    const up = window.innerHeight - r.bottom < height && r.top > height;
    const left = Math.min(r.left, window.innerWidth - 290);
    setPlace({ left: Math.max(8, left), top: up ? r.top - 4 : r.bottom + 4, up });
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const target = e.target as Node;
      if (pop.current?.contains(target) || button.current?.contains(target)) return;
      setOpen(false);
    };
    const scrolled = (e: Event) => {
      if (pop.current && e.target instanceof Node && pop.current.contains(e.target)) return;
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

  const onKey = (e: React.KeyboardEvent) => {
    if (!open) {
      if (["ArrowDown", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show();
      }
      return;
    }
    const move: Record<string, () => Date> = {
      ArrowLeft: () => addDays(cursor, -1),
      ArrowRight: () => addDays(cursor, 1),
      ArrowUp: () => addDays(cursor, -7),
      ArrowDown: () => addDays(cursor, 7),
      PageUp: () => addMonths(cursor, -1),
      PageDown: () => addMonths(cursor, 1),
    };
    if (move[e.key]) {
      e.preventDefault();
      setCursor(move[e.key]());
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(cursor);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    } else if (e.key === "Tab") {
      setOpen(false);
    }
  };

  // Six weeks from the first day of the week the month starts in.
  const first = new Date(cursor.getFullYear(), cursor.getMonth(), 1);
  const lead = (first.getDay() - weekStart + 7) % 7;
  const start = addDays(first, -lead);
  const days = Array.from({ length: 42 }, (_, i) => addDays(start, i));
  const today = iso(new Date());
  const weekdays = Array.from({ length: 7 }, (_, i) =>
    new Intl.DateTimeFormat(locale, { weekday: "short" }).format(new Date(2026, 0, 4 + weekStart + i)),
  );
  // "октябрь 2026 г." with a capital first letter only: CSS capitalize also
  // raised the "г." to "Г.".
  const monthRaw = new Intl.DateTimeFormat(locale, { month: "long", year: "numeric" }).format(first);
  const monthTitle = monthRaw.charAt(0).toUpperCase() + monthRaw.slice(1);
  const label = chosen ? new Intl.DateTimeFormat(locale, { day: "numeric", month: "short", year: "numeric" }).format(chosen) : "";

  return (
    <>
      <button
        ref={button}
        type="button"
        className={"selectBtn dateBtn" + (chosen ? "" : " empty")}
        style={style}
        aria-label={ariaLabel ?? placeholder}
        aria-haspopup="dialog"
        aria-expanded={open}
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          button.current?.focus();
          if (open) setOpen(false);
          else show();
        }}
        onKeyDown={onKey}
      >
        <span className="selectSizer">
          <span>{label || placeholder || " "}</span>
        </span>
      </button>
      {open &&
        place &&
        createPortal(
          <div
            ref={pop}
            className="selectList datePop"
            role="dialog"
            aria-label={ariaLabel ?? placeholder}
            style={{ left: place.left, ...(place.up ? { bottom: window.innerHeight - place.top } : { top: place.top }) }}
            onMouseDown={(e) => e.preventDefault()}
          >
            <div className="dateHead">
              <button type="button" className="ghost small" aria-label="‹" onClick={() => setCursor(addMonths(cursor, -1))}>
                ‹
              </button>
              <span className="dateTitle">{monthTitle}</span>
              <button type="button" className="ghost small" aria-label="›" onClick={() => setCursor(addMonths(cursor, 1))}>
                ›
              </button>
            </div>
            <div className="dateGrid">
              {weekdays.map((w) => (
                <span key={w} className="dateWeekday">
                  {w}
                </span>
              ))}
              {days.map((d) => {
                const k = iso(d);
                const cls = [
                  "dateDay",
                  d.getMonth() !== cursor.getMonth() ? "outside" : "",
                  k === today ? "today" : "",
                  k === value ? "chosen" : "",
                  k === iso(cursor) ? "active" : "",
                ]
                  .filter(Boolean)
                  .join(" ");
                return (
                  <button
                    key={k}
                    type="button"
                    className={cls}
                    aria-pressed={k === value}
                    onMouseEnter={() => d.getMonth() === cursor.getMonth() && setCursor(d)}
                    onClick={() => pick(d)}
                  >
                    {d.getDate()}
                  </button>
                );
              })}
            </div>
            <div className="dateFoot">
              <button type="button" className="ghost small" onClick={() => pick(new Date())}>
                {t("date.today")}
              </button>
              {value && (
                <button type="button" className="ghost small" onClick={() => pick(null)}>
                  {t("date.clear")}
                </button>
              )}
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
