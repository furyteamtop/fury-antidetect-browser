// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";

export const themes = ["system", "dark", "light"] as const;
export type Theme = (typeof themes)[number];

const KEY = "fury.theme";

/** Theme choice, applied to the document root.
 *
 *  "system" is not a third palette — it means "do not decide", and the stylesheet
 *  answers through prefers-color-scheme. Storing the literal choice rather than
 *  the resolved one matters: someone who picked "system" should follow the
 *  desktop when it changes at sunset, not stay wherever it happened to be when
 *  they chose. */
export function useTheme(): [Theme, (t: Theme) => void] {
  const [theme, setStored] = useState<Theme>(
    () => (localStorage.getItem(KEY) as Theme) || "system",
  );

  useEffect(() => {
    const root = document.documentElement;
    const media = window.matchMedia("(prefers-color-scheme: light)");

    const apply = () => {
      const resolved = theme === "system" ? (media.matches ? "light" : "dark") : theme;
      root.setAttribute("data-theme", resolved);
    };
    apply();

    if (theme !== "system") return;
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);

  return [
    theme,
    (t: Theme) => {
      localStorage.setItem(KEY, t);
      setStored(t);
    },
  ];
}
