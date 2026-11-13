// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useRef } from "react";
import type { ExtensionPackage, KeySource } from "../api";
import { useI18n } from "../i18n";

/** Choosing an extension to install: a .crx or .zip, or an unpacked folder
 *  (issue 28). Shared by the profile's Extensions tab, the Extensions section
 *  and the per-profile Extensions dialog.
 *
 *  The shell has no native file dialog, so what a chosen file or folder is to
 *  this side is bytes, and bytes are what the agent is handed; it tells a .crx
 *  from a .zip by their first bytes and pins the id (agent/src/ext.rs). */

type T = ReturnType<typeof useI18n>["t"];

/** A folder bigger than this is almost certainly not one extension: a parent
 *  folder, or a project with node_modules in it. The pipe would carry it, the
 *  profile would keep it, and Chromium would load it slowly. */
const MAX_FOLDER_MB = 100;

class TooBig extends Error {
  constructor() {
    super("too_big");
  }
}

function toBase64(f: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onerror = () => reject(r.error);
    // A data: URL is "data:<type>;base64,<payload>"; the payload is what the
    // agent wants.
    r.onload = () => resolve(String(r.result).split(",", 2)[1] ?? "");
    r.readAsDataURL(f);
  });
}

async function readFolder(list: File[]): Promise<ExtensionPackage> {
  // Hidden entries (.git, .DS_Store, an editor's settings) are not part of an
  // extension and can be most of a developer's folder.
  const files = list.filter((f) => !(f.webkitRelativePath || f.name).split("/").some((p) => p.startsWith(".")));
  if (files.reduce((sum, f) => sum + f.size, 0) > MAX_FOLDER_MB * 1024 * 1024) throw new TooBig();
  return {
    files: await Promise.all(files.map(async (f) => ({ path: f.webkitRelativePath || f.name, b64: await toBase64(f) }))),
  };
}

/** The sentence for an install that failed, from the agent's stable reason
 *  names where there is one. */
export function packageError(t: T, say: (e: unknown) => string, e: unknown): string {
  if (e instanceof TooBig) return t("ext.err.tooBig", { mb: MAX_FOLDER_MB });
  const text = say(e);
  if (text.includes("not_a_package")) return t("ext.err.notPackage");
  if (text.includes("no_manifest")) return t("ext.err.noManifest");
  if (text.includes("several_manifests")) return t("ext.err.severalManifests");
  return text;
}

/** Why one profile was skipped, from the agent's (or the shell's) stable
 *  reason names. "open" is left to the caller: what to say about it depends on
 *  how many profiles were asked for. */
export function skipReason(t: T, r: string): string {
  if (r === "proxy_missing") return t("exv.reason.proxy_missing");
  if (r === "not_here") return t("exv.reason.not_here");
  if (r.startsWith("id_mismatch:")) return t("exv.reason.id_mismatch");
  if (r.startsWith("fetch:")) return t("exv.reason.fetch", { detail: r.slice(6) });
  if (r.startsWith("in_use:")) {
    const [who, machine] = r.slice(7).split("|");
    return t("exv.reason.in_use", { who: who || "?", machine: machine || "?" });
  }
  return r;
}

/** What the operator should know about the id: nothing for a .crx or a
 *  manifest with its own key; that Fury made one, or reused the one it made. */
export function keyNote(t: T, source: KeySource | undefined, name: string, id: string | undefined): string {
  if (source === "generated") return t("ext.keyGenerated", { name, id: id ?? "?" });
  if (source === "saved") return t("ext.keySaved", { name, id: id ?? "?" });
  return "";
}

/** Two buttons, a file and a folder, and the hidden inputs behind them. */
export function PackageButtons({
  disabled,
  fileLabel,
  folderLabel,
  primary,
  onPicked,
  onError,
}: {
  disabled: boolean;
  fileLabel: string;
  folderLabel: string;
  primary?: boolean;
  /** `label` is the file's or folder's name, for a message when the agent
   *  could not name the extension. */
  onPicked: (pkg: ExtensionPackage, label: string) => void;
  onError: (e: unknown) => void;
}) {
  const file = useRef<HTMLInputElement>(null);
  const folder = useRef<HTMLInputElement>(null);

  // Not a React prop: TypeScript's DOM types do not know it, and React passes
  // an unknown boolean attribute through only as a string anyway.
  useEffect(() => {
    folder.current?.setAttribute("webkitdirectory", "");
  }, []);

  const reset = () => {
    if (file.current) file.current.value = "";
    if (folder.current) folder.current.value = "";
  };

  return (
    <>
      <input
        ref={file}
        type="file"
        accept=".crx,.zip"
        style={{ display: "none" }}
        onChange={async (e) => {
          const f = e.target.files?.[0];
          if (!f) return;
          try {
            onPicked({ packageB64: await toBase64(f) }, f.name);
          } catch (err) {
            onError(err);
          } finally {
            reset();
          }
        }}
      />
      <input
        ref={folder}
        type="file"
        style={{ display: "none" }}
        onChange={async (e) => {
          const list = [...(e.target.files ?? [])];
          if (list.length === 0) return;
          try {
            const name = (list[0].webkitRelativePath || list[0].name).split("/")[0];
            onPicked(await readFolder(list), name);
          } catch (err) {
            onError(err);
          } finally {
            reset();
          }
        }}
      />
      <button className={primary ? "primary" : undefined} disabled={disabled} onClick={() => file.current?.click()}>
        {fileLabel}
      </button>
      <button className={primary ? "primary" : undefined} disabled={disabled} onClick={() => folder.current?.click()}>
        {folderLabel}
      </button>
    </>
  );
}
