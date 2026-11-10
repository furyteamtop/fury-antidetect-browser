// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
// Bookmark sets: a tree of folders and links made once and given to many
// profiles (agent/src/bookmarks.rs).
//
// The agent writes a set onto a profile's bookmarks bar when the profile is
// next opened, so nothing here touches a running browser and nothing here is
// instant. The screen says so, because "I saved it and the bookmark is not
// there" is otherwise the first thing anybody reports.

import { useEffect, useState } from "react";

import { api, type BookmarkNode, type BookmarkSet, type Profile } from "../api";
import { useI18n } from "../i18n";
import { useAsk } from "./Ask";
import { IconButton } from "./Icon";

const blank = (): BookmarkSet => ({ id: "", name: "", nodes: [], profiles: [], all_profiles: false });

const isFolder = (n: BookmarkNode) => n.url === undefined || n.url === null;

function countLinks(nodes: BookmarkNode[]): number {
  return nodes.reduce((sum, n) => sum + (isFolder(n) ? countLinks(n.children ?? []) : 1), 0);
}

/** The list at `path`, replaced by what `change` makes of it. A path is the
 *  indexes of the folders on the way down; the empty path is the top level. */
function editAt(nodes: BookmarkNode[], path: number[], change: (list: BookmarkNode[]) => BookmarkNode[]): BookmarkNode[] {
  if (path.length === 0) return change(nodes);
  const [head, ...rest] = path;
  return nodes.map((n, i) => (i === head ? { ...n, children: editAt(n.children ?? [], rest, change) } : n));
}

export function BookmarksView({ profiles }: { profiles: Profile[] }) {
  const { t, say } = useI18n();
  const [sets, setSets] = useState<BookmarkSet[]>([]);
  const [editing, setEditing] = useState<BookmarkSet | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const { ask, dialog } = useAsk();

  const load = async () => {
    try {
      setSets(await api.bookmarkSets());
      setError(null);
    } catch (e) {
      setError(say(e));
    }
  };
  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = async () => {
    if (!editing) return;
    setBusy(true);
    try {
      await api.saveBookmarkSet(editing);
      setEditing(null);
      setSaved(true);
      await load();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (s: BookmarkSet) => {
    const go = await ask({
      title: t("row.delete"),
      detail: t("bm.deleteConfirm", { name: s.name }),
      confirmLabel: t("ui.delete"),
      danger: true,
    });
    if (go === null) return;
    try {
      await api.deleteBookmarkSet(s.id);
      setSaved(true);
      await load();
    } catch (e) {
      setError(say(e));
    }
  };

  return (
    <section className="bookmarks">
      {dialog}
      <p className="muted">{t("bm.intro")}</p>

      {error && (
        <div className="notice" role="status">
          {error}
          <button className="ghost" onClick={() => setError(null)}>
            {t("app.dismiss")}
          </button>
        </div>
      )}
      {saved && !editing && (
        <div className="notice" role="status">
          {t("bm.saved")}
          <button className="ghost" onClick={() => setSaved(false)}>
            {t("app.dismiss")}
          </button>
        </div>
      )}

      {!editing && (
        <>
          {sets.length === 0 && <p className="muted">{t("bm.empty")}</p>}
          {sets.map((s) => (
            <div key={s.id} className="bmSet">
              <div>
                <strong>{s.name}</strong>
                <div className="muted small">
                  {s.all_profiles
                    ? t("bm.countAll", { links: countLinks(s.nodes) })
                    : t("bm.count", {
                        links: countLinks(s.nodes),
                        // Only the ones still here: a deleted profile stays
                        // in the list until the set is saved again.
                        profiles: s.profiles.filter((id) => profiles.some((p) => p.id === id)).length,
                      })}
                </div>
              </div>
              <div className="bmActions">
                <button className="ghost" onClick={() => { setSaved(false); setEditing(structuredClone(s)); }}>
                  {t("row.edit")}
                </button>
                <button className="ghost danger" onClick={() => void remove(s)}>
                  {t("row.delete")}
                </button>
              </div>
            </div>
          ))}
          <button className="primary" onClick={() => { setSaved(false); setEditing(blank()); }}>
            {t("bm.new")}
          </button>
        </>
      )}

      {editing && (
        <div className="bmEditor">
          <div className="field">
            <label htmlFor="bm-name">{t("bm.name")}</label>
            <input
              id="bm-name"
              value={editing.name}
              autoFocus
              onChange={(e) => setEditing({ ...editing, name: e.target.value })}
            />
          </div>

          <NodeList
            nodes={editing.nodes}
            path={[]}
            onChange={(path, change) => setEditing({ ...editing, nodes: editAt(editing.nodes, path, change) })}
          />

          <h3>{t("bm.profiles")}</h3>
          <label className="bmCheck">
            <input
              type="checkbox"
              checked={editing.all_profiles}
              onChange={(e) => setEditing({ ...editing, all_profiles: e.target.checked })}
            />
            {t("bm.allProfiles")}
          </label>
          {!editing.all_profiles && (
            <ProfilePicker
              profiles={profiles}
              chosen={editing.profiles}
              onChange={(ids) => setEditing({ ...editing, profiles: ids })}
            />
          )}

          <div className="bmFoot">
            <button className="ghost" onClick={() => setEditing(null)} disabled={busy}>
              {t("ui.cancel")}
            </button>
            <button className="primary" onClick={() => void save()} disabled={busy || editing.name.trim() === ""}>
              {t("ui.save")}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

function NodeList({
  nodes,
  path,
  onChange,
}: {
  nodes: BookmarkNode[];
  path: number[];
  onChange: (path: number[], change: (list: BookmarkNode[]) => BookmarkNode[]) => void;
}) {
  const { t } = useI18n();
  const set = (i: number, patch: Partial<BookmarkNode>) =>
    onChange(path, (list) => list.map((n, j) => (j === i ? { ...n, ...patch } : n)));
  const move = (i: number, by: number) =>
    onChange(path, (list) => {
      const to = i + by;
      if (to < 0 || to >= list.length) return list;
      const next = [...list];
      [next[i], next[to]] = [next[to], next[i]];
      return next;
    });

  return (
    <div className={path.length === 0 ? "bmTree" : "bmTree nested"}>
      {nodes.map((n, i) => (
        <div key={i} className="bmNode">
          <div className="bmRow">
            {isFolder(n) ? (
              <input
                className="bmFolder"
                value={n.title}
                placeholder={t("bm.folderName")}
                aria-label={t("bm.folderName")}
                onChange={(e) => set(i, { title: e.target.value })}
              />
            ) : (
              <>
                <input
                  value={n.title}
                  placeholder={t("bm.title")}
                  aria-label={t("bm.title")}
                  onChange={(e) => set(i, { title: e.target.value })}
                />
                <input
                  className="mono"
                  value={n.url ?? ""}
                  placeholder="https://"
                  aria-label={t("bm.url")}
                  onChange={(e) => set(i, { url: e.target.value })}
                />
              </>
            )}
            <button className="ghost bmMove" title={t("bm.up")} aria-label={t("bm.up")} disabled={i === 0} onClick={() => move(i, -1)}>
              ↑
            </button>
            <button
              className="ghost bmMove"
              title={t("bm.down")}
              aria-label={t("bm.down")}
              disabled={i === nodes.length - 1}
              onClick={() => move(i, 1)}
            >
              ↓
            </button>
            <IconButton icon="trash" label={t("bm.remove")} danger onClick={() => onChange(path, (list) => list.filter((_, j) => j !== i))} />
          </div>
          {isFolder(n) && <NodeList nodes={n.children ?? []} path={[...path, i]} onChange={onChange} />}
        </div>
      ))}
      <div className="bmAdd">
        <button className="ghost" onClick={() => onChange(path, (list) => [...list, { title: "", url: "" }])}>
          + {t("bm.addLink")}
        </button>
        {/* The agent refuses folders nested more than eight deep; nobody gets
            there by accident, so the button simply stops at seven. */}
        {path.length < 7 && (
          <button className="ghost" onClick={() => onChange(path, (list) => [...list, { title: "", children: [] }])}>
            + {t("bm.addFolder")}
          </button>
        )}
      </div>
    </div>
  );
}

function ProfilePicker({
  profiles,
  chosen,
  onChange,
}: {
  profiles: Profile[];
  chosen: string[];
  onChange: (ids: string[]) => void;
}) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const shown = profiles.filter((p) => p.name.toLowerCase().includes(query.trim().toLowerCase()));
  const has = (id: string) => chosen.includes(id);
  const allShown = shown.length > 0 && shown.every((p) => has(p.id));

  if (profiles.length === 0) return <p className="muted">{t("bm.noProfiles")}</p>;
  return (
    <div className="bmPicker">
      <div className="bmPickerHead">
        <input value={query} placeholder={t("bm.search")} aria-label={t("bm.search")} onChange={(e) => setQuery(e.target.value)} />
        <label className="bmCheck">
          <input
            type="checkbox"
            checked={allShown}
            onChange={(e) => {
              const ids = shown.map((p) => p.id);
              onChange(e.target.checked ? [...new Set([...chosen, ...ids])] : chosen.filter((id) => !ids.includes(id)));
            }}
          />
          {t("bm.selectShown", { n: shown.length })}
        </label>
      </div>
      <div className="bmPickerList">
        {shown.map((p) => (
          <label key={p.id} className="bmCheck">
            <input
              type="checkbox"
              checked={has(p.id)}
              onChange={(e) => onChange(e.target.checked ? [...chosen, p.id] : chosen.filter((id) => id !== p.id))}
            />
            {p.name}
          </label>
        ))}
      </div>
    </div>
  );
}
