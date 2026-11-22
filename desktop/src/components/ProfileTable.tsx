// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { flag } from "../flag";
import { useI18n } from "../i18n";
import { Icon, IconButton } from "./Icon";
import type { Me, Profile } from "../api";
import { platformOf } from "../platform";
import { assess } from "../consistency";
import { isSuggested, statusHue } from "../status";
import { sortRows, useSort } from "../sort";
import { SortTh } from "./SortTh";
import { proxyActionKey, proxyExit, type ProxyAction } from "../proxyAction";

const SORT_KEYS = ["name", "project", "proxy", "status", "lastOpened"] as const;
type SortKey = (typeof SORT_KEYS)[number];

/** Every row's controls follow the permissions the SERVER resolved. Hiding a
 *  button is presentation, not protection — the server refuses regardless — but
 *  showing a control that will always fail is its own kind of lie.
 *
 *  In local mode there are no permissions to resolve, and the state that
 *  matters is different: not "who holds this" but "is it open right now". */
/** Is this profile open on THIS machine, right now?
 *
 *  Two different questions behind one answer, and which one applies depends on
 *  where the profile lives. A local profile is open when its browser process is
 *  running here, which the agent reports. A team profile is open when the
 *  server's lock is held by this user ON THIS MACHINE — matching on the user
 *  alone was wrong, because the same person signed in on a laptop and a desktop
 *  would see the laptop's live lock labelled "open here", with a Close button
 *  that worked and released a lock whose browser was still running elsewhere.
 *
 *  Exported because the window outside this table needs the same answer — a
 *  notice about a launch has to stop being shown when that launch has ended —
 *  and two spellings of "is it open" would drift the first time one changed. */
export function isOpenHere(
  p: Profile,
  where: { local: boolean; userId?: string; machine: string },
): boolean {
  // By where the profile lives, as the comment says, and not by the shell's
  // mode: connected to a server, a profile of this machine's was asked about
  // its (absent) lock, so one that was running showed Free and Open
  // (14.10.2026).
  if (where.local || p.origin === "local") return p.running;
  return (
    p.lock !== null &&
    p.lock.user_id === where.userId &&
    p.lock.machine_name === where.machine
  );
}

/** 5.4 MB, 820 KB: one decimal under ten, none above. */
function formatSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  const mb = bytes / (1024 * 1024);
  return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

export function ProfileTable({
  profiles: given,
  me,
  thisMachine,
  local,
  busy,
  launching,
  onLaunch,
  onStop,
  onEdit,
  onEditProxy,
  onCheckProxy,
  onRotateProxy,
  proxyActions,
  onDelete,
  onExtensions,
  onShare,
  onNetwork,
  onClone,
  personas,
  sharing,
  selected,
  onToggle,
  onToggleAll,
  showProject,
}: {
  profiles: Profile[];
  me: Me | null;
  thisMachine: string;
  local: boolean;
  busy: boolean;
  /** Profiles whose launch is in flight: only their own buttons wait. */
  launching?: Set<string>;
  onLaunch: (p: Profile, force?: boolean) => void;
  onStop: (p: Profile) => void;
  /** Local mode only: with a server, editing belongs where the permissions
   *  live, and that screen does not exist yet. */
  onEdit?: (p: Profile) => void;
  /** Opens the profile editor on its saved-proxy tab. */
  onEditProxy?: (p: Profile) => void;
  /** Checks the proxy assigned to a visible profile. */
  onCheckProxy?: (p: Profile) => void;
  onRotateProxy?: (p: Profile) => void;
  proxyActions?: Map<string, ProxyAction>;
  onDelete?: (p: Profile) => void;
  /** Extensions and disk usage — the two things beside the browser data. */
  onExtensions?: (p: Profile) => void;
  /** Give someone access to this one profile. On the row, in words, because
   *  as an unlabelled icon that appeared only once a row was ticked nobody
   *  found it -- the owner asked where it was (08.10.2026). Absent when there
   *  is no server to share through. */
  onShare?: (p: Profile) => void;
  /** What network this profile is on, step by step. Opens from the proxy cell. */
  onNetwork?: (p: Profile) => void;
  /** Copies of this one — the same dialog the selection bar opens (5.24). */
  onClone?: (p: Profile) => void;
  /** The catalogue's persona ids, so a row on a persona that no longer exists
   *  says so before Open is pressed. Null while unknown. */
  personas?: Set<string> | null;
  /** Profile id → how many profiles come out of the same exit (consistency.ts). */
  sharing?: Map<string, number>;
  selected: Set<string>;
  onToggle: (id: string) => void;
  onToggleAll: () => void;
  /** Shown when the list spans projects — inside one, the column would repeat
   *  the same value on every row. */
  showProject: boolean;
}) {
  const { t } = useI18n();
  // Marked whenever there IS a server and the row is not on it.
  //
  // The first rule was "only when the list holds both kinds", and it was wrong
  // the moment somebody signed up: a brand new organisation has no profiles, so
  // the list was entirely local, nothing was marked, and the one row on screen
  // gave no hint that it was invisible to the team. The question the badge
  // answers is "is this on the server", and that question exists as soon as
  // there is a server.
  const hasServer = !local;

  // Sorted here, before windowing, so the window is a slice of the sorted
  // list. Selection is by id, so a re-sort keeps what was ticked.
  const sort = useSort<SortKey>("fury.sort.profiles", SORT_KEYS);
  const profiles = sortRows(given, sort, (p, key) => {
    switch (key) {
      case "name":
        return p.name;
      case "project":
        return [p.project_name, p.name];
      case "proxy": {
        // Exit country first, then the saved label that identifies it.
        const action = p.proxy && (p.origin === "local" || p.origin === "team")
          ? proxyActions?.get(proxyActionKey(p.origin, p.proxy.id))
          : undefined;
        const exit = proxyExit(action, { country: p.proxy?.country, ip: p.proxy?.last_ip });
        return [exit?.country, p.proxy?.name || p.proxy?.display, p.name];
      }
      case "status": {
        // The account's stage first: that is what a farm sorts by. Then
        // open here, in use elsewhere, idle.
        const open = isOpenHere(p, { local, userId: me?.user_id, machine: thisMachine });
        return [(p.status ?? "").trim().toLowerCase(), open ? 0 : p.lock ? 1 : 2, p.name];
      }
      case "lastOpened":
        return p.last_opened_at ? Date.parse(p.last_opened_at) : null;
    }
  });

  // Windowing (docs/16 5.8). Measured before it was written: a full render
  // of 2 000 rows cost ~120 ms and 4 MB of DOM, and the list re-renders on
  // every 5-second poll. Below WINDOW_FROM rows nothing changes — a short
  // list is rendered whole, so it cannot jitter. Above it, only the rows in
  // view plus a buffer are rendered, with two spacer rows holding the scroll
  // height. Row height is an estimate corrected from what is on screen;
  // rows differ by a tags line or a stage pill, so the estimate is an
  // average, and the buffer absorbs the difference.
  const tableRef = useRef<HTMLTableElement>(null);
  const [rowHeight, setRowHeight] = useState(58);
  const [range, setRange] = useState({ start: 0, end: WINDOW_FROM });
  const windowed = profiles.length > WINDOW_FROM;
  useEffect(() => {
    if (!windowed) return;
    const scroller = tableRef.current?.parentElement;
    if (!scroller) return;
    const update = () => {
      const top = scroller.scrollTop;
      const view = scroller.clientHeight || 600;
      const start = Math.max(0, Math.floor(top / rowHeight) - BUFFER);
      const end = Math.min(profiles.length, Math.ceil((top + view) / rowHeight) + BUFFER);
      setRange((r) => (r.start === start && r.end === end ? r : { start, end }));
    };
    update();
    scroller.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(scroller);
    return () => {
      scroller.removeEventListener("scroll", update);
      ro.disconnect();
    };
  }, [windowed, rowHeight, profiles.length]);
  useLayoutEffect(() => {
    if (!windowed) return;
    // Correct the estimate from the rows actually on screen.
    const rows = tableRef.current?.querySelectorAll<HTMLTableRowElement>("tbody tr[data-row]");
    if (!rows || rows.length === 0) return;
    let sum = 0;
    rows.forEach((r) => (sum += r.getBoundingClientRect().height));
    const avg = sum / rows.length;
    if (avg > 20 && Math.abs(avg - rowHeight) > 2) setRowHeight(avg);
  });

  if (profiles.length === 0) {
    return <p className="empty pad">{t("row.emptyProject")}</p>;
  }

  const visible = windowed ? profiles.slice(range.start, range.end) : profiles;
  const columns = 6 + (showProject ? 1 : 0);

  return (
    <table className="grid" ref={tableRef}>
      <thead>
        <tr>
          <th className="checkCol">
            <input
              type="checkbox"
              aria-label="select all"
              checked={selected.size > 0 && selected.size === profiles.length}
              // Neither on nor off: some rows are picked. Without this the box
              // reads as "nothing selected" while a bulk action is armed.
              ref={(el) => {
                if (el) el.indeterminate = selected.size > 0 && selected.size < profiles.length;
              }}
              onChange={onToggleAll}
            />
          </th>
          <SortTh k="name" sort={sort}>{t("col.name")}</SortTh>
          {/* Only when the list spans projects. Inside one project the column
              would repeat the same value on every row. */}
          {showProject && <SortTh k="project" sort={sort}>{t("col.project")}</SortTh>}
          <SortTh k="proxy" sort={sort}>{t("col.proxy")}</SortTh>
          <SortTh k="status" sort={sort}>{t("col.status")}</SortTh>
          <SortTh k="lastOpened" sort={sort} first="desc">{t("col.lastOpened")}</SortTh>
          <th />
        </tr>
      </thead>
      <tbody>
        {windowed && range.start > 0 && (
          <tr aria-hidden="true" style={{ height: range.start * rowHeight }}>
            <td colSpan={columns} style={{ padding: 0, border: 0 }} />
          </tr>
        )}
        {visible.map((p) => {
          const canLaunch = p.permissions.includes("launch");
          const canForce = p.permissions.includes("manage_access");
          const canReveal = p.permissions.includes("reveal_secrets");
          const canDelete = p.permissions.includes("delete_profile");
          const canEdit = p.permissions.includes("edit_profile");
          const proxyAction = p.proxy && (p.origin === "local" || p.origin === "team")
            ? proxyActions?.get(proxyActionKey(p.origin, p.proxy.id))
            : undefined;
          const check = proxyAction?.action === "check" ? proxyAction : undefined;
          const rotation = proxyAction?.action === "rotate" ? proxyAction : undefined;
          const canCheckProxy = (p.origin === "local" || p.origin === "team")
            && (p.origin === "local" || me?.role === "owner" || me?.role === "admin");
          const canRotateProxy = p.origin === "local" && !!p.proxy?.has_rotation && canEdit;
          const checkLabel = check?.status === "pending" ? t("px.checking") : t("px.check");
          const checkTitle = check?.status === "ok"
            ? t("px.checkOk", { ms: check.ms ?? 0 })
            : check?.status === "error"
              ? check.error ?? t("px.checkBad")
              : checkLabel;
          const rotationLabel = rotation?.status === "pending" ? t("px.rotating") : t("px.rotateNow");
          const rotationTitle = rotation?.status === "error"
              ? rotation.error ?? t("px.rotateFailed")
              : rotationLabel;
          const exit = proxyExit(proxyAction, { country: p.proxy?.country, ip: p.proxy?.last_ip });
          const locked = p.lock !== null;
          const open = isOpenHere(p, { local, userId: me?.user_id, machine: thisMachine });
          const starting = launching?.has(p.id) ?? false;
          // What the row says about itself (docs/12 B). A red verdict disables
          // Open with the reason: the contradiction is the ban, not the
          // fingerprint, and a warning that scrolls past is not a guard.
          const shared = sharing?.get(p.id) ?? 0;
          const verdict = assess(p, personas ?? null);
          const verdictText = verdict.notes.map((n) => t(n.key, n.vars)).join(" · ");

          return (
            <tr key={p.id} data-row className={selected.has(p.id) ? "picked" : undefined}>
              <td className="checkCol">
                <input
                  type="checkbox"
                  aria-label={p.name}
                  checked={selected.has(p.id)}
                  onChange={() => onToggle(p.id)}
                />
              </td>
              <td>
                <div className="name">
                  {/* Where the profile opens, as a two-letter badge in the
                      platform's colour, so a list of forty scans by platform.
                      Derived from the start URLs, never fetched: see
                      platform.ts for why a favicon request would be a leak. */}
                  {(() => {
                    const pl = platformOf(p.start_urls);
                    return pl ? (
                      <span
                        className="platform"
                        title={pl.name}
                        style={{ background: `hsl(${pl.hue} 45% 30%)`, color: `hsl(${pl.hue} 80% 88%)` }}
                      >
                        {pl.mark}
                      </span>
                    ) : null;
                  })()}
                  {p.name}
                  {/* Only the local ones are marked, and only when the list is
                      mixed. Connected to a server, every row without this badge
                      is the team's; on a machine with no server every row would
                      carry it, which is noise rather than information. */}
                  {/* Marks rather than words. A row is read at a glance and a
                      list of them is read as a column, so two glyphs in a fixed
                      place carry more than two labels of different lengths
                      pushing the name around. Both keep their sentence on
                      hover. */}
                  {hasServer && p.origin === "local" && (
                    <span className="mark" title={t("col.onlyHereWhy")}>
                      <Icon name="laptop" size={13} />
                    </span>
                  )}
                  {p.shared_with > 0 && (
                    <span
                      className="mark"
                      title={t("col.sharedWith", { n: String(p.shared_with) })}
                    >
                      <Icon name="people" size={13} />
                      <span className="markN">{p.shared_with}</span>
                    </span>
                  )}
                  {/* The consistency verdict, only when there is one to give.
                      A tick on every consistent row would be a column of
                      ticks; silence is the tick. */}
                  {verdict.level !== "ok" && (
                    <span className={`mark verdict ${verdict.level}`} title={verdictText}>
                      <Icon name="alert" size={13} />
                    </span>
                  )}
                </div>
                {p.tags.length > 0 && (
                  <div className="tags">{p.tags.map((t) => <span key={t}>{t}</span>)}</div>
                )}
              </td>
              {showProject && (
                <td className="muted nowrap">
                  {p.project_name ?? <span className="dim">{t("col.noProject")}</span>}
                </td>
              )}
              <td
                className={p.proxy && onNetwork ? "clickable" : undefined}
                title={p.proxy && onNetwork ? t("net.current") : undefined}
                onClick={() => p.proxy && onNetwork?.(p)}
              >
                {p.proxy ? (
                  <div className="proxyCell">
                    <div className="proxyLabelRow">
                      {onCheckProxy && canCheckProxy && (
                        <span className={`proxyCheck proxyCheck-${check?.status ?? "idle"}`} onClick={(e) => e.stopPropagation()}>
                          <IconButton
                            icon="probe"
                            label={checkLabel}
                            title={checkTitle}
                            disabled={busy || proxyAction?.status === "pending"}
                            onClick={() => onCheckProxy(p)}
                          />
                        </span>
                      )}
                      {onRotateProxy && canRotateProxy && (
                        <span className={`proxyRotate proxyRotate-${rotation?.status ?? "idle"}`} onClick={(e) => e.stopPropagation()}>
                          <IconButton
                            icon="refresh"
                            label={rotationLabel}
                            title={rotationTitle}
                            disabled={busy || proxyAction?.status === "pending"}
                            onClick={() => onRotateProxy(p)}
                          />
                        </span>
                      )}
                      <div className="proxyLabel" title={p.proxy.name}>
                        {p.proxy.name || p.proxy.display}
                      </div>
                      {onEditProxy && canEdit && (
                        <span className="proxyEdit" onClick={(e) => e.stopPropagation()}>
                          <IconButton icon="pencil" label={t("px.edit")} disabled={busy} onClick={() => onEditProxy(p)} />
                        </span>
                      )}
                    </div>
                    {/* Until the proxy has been checked the line says so: a
                        label alone reads as a broken value rather than a fact
                        not yet known. */}
                    {!exit?.country && !exit?.ip && canReveal && shared < 2 && !check && !rotation && (
                      <div className="muted small">{t("row.proxyUnchecked")}</div>
                    )}
                    {(exit?.country || exit?.ip || !canReveal || shared >= 2) && (
                      <div className="muted small">
                        {exit?.country && <><span className="flag">{flag(exit.country)}</span> {exit.country}</>}
                        {exit?.country && exit?.ip && " · "}
                        {exit?.ip && <span className="mono">{exit.ip}</span>}
                        {!canReveal && <>{(exit?.country || exit?.ip) && " · "}{t("row.masked")}</>}
                        {shared >= 2 && <span>{(exit?.country || exit?.ip || !canReveal) && " · "}{t("row.sharedExit", { n: shared })}</span>}
                      </div>
                    )}
                    {check?.status === "pending" && check.afterRotation && <div className="muted small">{t("px.checking")}</div>}
                    {check?.status === "error" && (
                      <div className="warn small" title={check.error}>
                        {t(check.errorCode === "noIp" ? "px.checkNoIp" : "px.checkBad")}
                      </div>
                    )}
                    {rotation?.status === "error" && (
                      <div className="warn small" title={rotation.error}>{rotation.error ?? t("px.rotateFailed")}</div>
                    )}
                  </div>
                ) : (
                  // Not cosmetic: the agent refuses to launch without one,
                  // unless this profile has been given permission — and then
                  // the row says so, because a profile that goes out from this
                  // machine is the one thing in the list somebody must not
                  // discover by accident.
                  <span className="warn">
                    {p.allow_no_proxy ? t("row.noProxyAllowed") : t("row.noProxy")}
                  </span>
                )}
              </td>
              <td>
                {/* A state, not an instruction.
                    This cell rendered `row.open` — the label on the button two
                    columns to the right — so a profile that was open said
                    "Open" in the column headed Status, next to a Close button.
                    Read as an offer to do something, which is what an
                    imperative is, and it was reported as exactly that: "if it
                    means it is open somewhere, then it is not Open". */}
                {open && <span className="state mineLock">{t("row.openHere")}</span>}
                {!open && locked && (
                  <span className="state lock">
                    {t("row.inUse", { who: p.lock!.user_email, machine: p.lock!.machine_name })}
                  </span>
                )}
                {!open && !locked && <span className="state free">{t("row.idle")}</span>}
                {/* The account's own stage beside the lock's state (5.5).
                    Known stages are translated and coloured; anything else is
                    shown as typed. */}
                {p.status && (() => {
                  const hue = statusHue(p.status);
                  return (
                    <div
                      className="stage"
                      style={hue === null ? undefined : { background: `hsl(${hue} 45% 22%)`, color: `hsl(${hue} 70% 80%)` }}
                    >
                      {isSuggested(p.status) ? t(`status.${p.status.trim().toLowerCase()}` as never) : p.status}
                    </div>
                  );
                })()}
              </td>
              <td className="muted small">
                {/* docs/12: the metric that matters to someone running accounts
                    is which profiles have gone stale. A profile untouched for
                    two months behaves differently from a live one. */}
                {p.last_opened_at ? new Date(p.last_opened_at).toLocaleString() : t("row.never")}
                {/* What it weighs where the team keeps it, and on hover what
                    that is. "How big will a profile be, and what goes to the
                    server -- cookies, history?" was a tester's question
                    (08.10.2026); the answer belongs where the profile is. */}
                {p.origin === "team" && p.bundle_bytes != null && (
                  <div title={t("row.onServerWhat")}>
                    {t("row.onServer", { size: formatSize(p.bundle_bytes) })}
                  </div>
                )}
              </td>
              <td className="actions">
                <div className="rowActions">
                {!open && !locked && canLaunch && (
                  <button
                    disabled={busy || starting || verdict.level === "block"}
                    title={verdict.level === "block" ? `${t("row.fixFirst")}: ${verdictText}` : undefined}
                    onClick={() => onLaunch(p)}
                  >
                    {starting ? t("row.opening") : t("row.open")}
                  </button>
                )}
                {open && (
                  <button className="ghost" disabled={busy} onClick={() => onStop(p)}>
                    {t("row.close")}
                  </button>
                )}
                {!open && locked && canForce && (
                  <button className="danger" disabled={busy || starting} onClick={() => onLaunch(p, true)}>
                    {t("row.takeOver")}
                  </button>
                )}
                {!open && locked && !canForce && (
                  <span className="muted small">{t("row.askThem")}</span>
                )}
                {/* Edit and delete are icons; open keeps its word.
                    Three text buttons on every row read as three equal
                    choices, and they are not: opening a profile is what
                    somebody came to do, editing is occasional and deleting is
                    rare and irreversible. The icons carry title and aria-label,
                    so the name is a hover away and a screen reader still gets
                    a word rather than a glyph. */}
                {onShare && (p.origin === "local" || p.permissions.includes("manage_access")) && (
                  <IconButton
                    icon="lock"
                    label={t("row.shareShort")}
                    title={t("row.shareHint")}
                    disabled={busy}
                    onClick={() => onShare(p)}
                  />
                )}
                {onExtensions && canEdit && (
                  <IconButton icon="puzzle" label={t("ext.extensions")} disabled={busy} onClick={() => onExtensions(p)} />
                )}
                {onEdit && canEdit && (
                  <IconButton icon="pencil" label={t("row.edit")} disabled={busy} onClick={() => onEdit(p)} />
                )}
                {onClone && canEdit && (
                  <IconButton icon="copy" label={t("bp.clone")} disabled={busy} onClick={() => onClone(p)} />
                )}
                {onDelete && canDelete && (
                  <IconButton
                    icon="trash"
                    danger
                    label={t("row.delete")}
                    title={open ? t("row.closeFirst") : t("row.delete")}
                    disabled={busy || open}
                    onClick={() => onDelete(p)}
                  />
                )}
                </div>
              </td>
            </tr>
          );
        })}
        {windowed && range.end < profiles.length && (
          <tr aria-hidden="true" style={{ height: (profiles.length - range.end) * rowHeight }}>
            <td colSpan={columns} style={{ padding: 0, border: 0 }} />
          </tr>
        )}
      </tbody>
    </table>
  );
}

/** Lists longer than this are windowed; shorter ones are rendered whole. */
const WINDOW_FROM = 150;
/** Rows rendered beyond each edge of the viewport, so a wheel tick never
 *  reaches a blank row before the next range is set. */
const BUFFER = 12;
