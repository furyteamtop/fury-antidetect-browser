// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { Icon, IconButton } from "./Icon";
import { useCallback, useEffect, useState } from "react";
import { api, type LocalProxy, type Profile } from "../api";
import { flag } from "../flag";
import { useI18n } from "../i18n";
import { useAsk } from "./Ask";
import { ProxyForm, addedByMe, proxyLock } from "./ProxyForm";
import { ProxyPaste } from "./ProxyPaste";
import { PartnerCard } from "./Partner";
import { sortRows, useSort } from "../sort";
import { SortTh } from "./SortTh";
import { proxyExit, type ProxyAction } from "../proxyAction";

const SORT_KEYS = ["name", "address", "lastSeen", "usedBy"] as const;
type SortKey = (typeof SORT_KEYS)[number];

/** The proxies this machine knows about.
 *
 *  A section rather than a list buried in the profile editor, because an exit
 *  outlives the profile that first needed it: the same residential IP carries
 *  three accounts, and the place to see that is here — the "used by" column is
 *  the one number that matters, since several accounts behind one address is
 *  the oldest farm signal there is. */
export function Proxies({
  profiles,
  local,
  myEmail = null,
  canCheck,
  canRotate,
  onCheckProxy,
  onRotateProxy,
  proxyActionFor,
  onProxySaved,
}: {
  profiles: Profile[];
  local: boolean;
  myEmail?: string | null;
  canCheck: boolean;
  canRotate: boolean;
  onCheckProxy: (proxy: LocalProxy) => Promise<boolean>;
  onRotateProxy: (proxy: LocalProxy) => Promise<boolean>;
  proxyActionFor: (proxy: LocalProxy) => ProxyAction | undefined;
  onProxySaved: (id: string) => void;
}) {
  const { t, say } = useI18n();
  const { ask, dialog } = useAsk();
  const [rows, setRows] = useState<LocalProxy[]>([]);
  const [editing, setEditing] = useState<LocalProxy | null | undefined>(undefined);
  const [pasting, setPasting] = useState(false);
  const [shopping, setShopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState<{ done: number; total: number } | null>(null);

  const checkAll = async () => {
    const list = rows.filter((proxy) => proxyActionFor(proxy)?.status !== "pending");
    if (!canCheck || list.length === 0) return;
    setChecking({ done: 0, total: list.length });
    let done = 0;
    // Three at a time: the checker is a third party, and forty simultaneous
    // requests from one address is how a checker starts refusing.
    const queue = [...list];
    const worker = async () => {
      for (;;) {
        const p = queue.shift();
        if (!p) return;
        await onCheckProxy(p);
        done++;
        setChecking({ done, total: list.length });
      }
    };
    await Promise.all([worker(), worker(), worker()]);
    setChecking(null);
    await load();
  };

  /** Local proxies whose password the keychain will not open now. They read
   *  back blank, and nothing used to say so: a tester re-typed every one after
   *  an update (15.10.2026). */
  const [unreadable, setUnreadable] = useState<string[]>([]);
  const [stillLocked, setStillLocked] = useState(false);

  const load = useCallback(async () => {
    try {
      setRows(await api.proxies());
      setError(null);
    } catch (e) {
      setError(say(e));
    }
    if (local) {
      void api.vaultStatus().then((v) => setUnreadable(v.unreadable), () => {});
    }
  }, [local]);

  useEffect(() => {
    void load();
  }, [load]);

  // Counted once, not once per row per render: forty proxies over two
  // thousand profiles is eighty thousand comparisons on every sort.
  const used = new Map<string, number>();
  for (const p of profiles) if (p.proxy) used.set(p.proxy.id, (used.get(p.proxy.id) ?? 0) + 1);
  const usedBy = (id: string) => used.get(id) ?? 0;

  const sort = useSort<SortKey>("fury.sort.proxies", SORT_KEYS);
  const shown = sortRows(rows, sort, (p, key) => {
    switch (key) {
      case "name":
        return p.name;
      case "address":
        return [p.host, p.port];
      case "lastSeen": {
        // Where the exit is: country, then address. After "check all" the
        // result breaks ties within one exit, faster first, failed last.
        const action = proxyActionFor(p);
        const exit = proxyExit(action, { country: p.last_country, ip: p.last_ip });
        const check = action?.action === "check" ? action : undefined;
        return [exit?.country, exit?.ip, check?.status === "error" ? Number.MAX_SAFE_INTEGER : check?.ms ?? null];
      }
      case "usedBy":
        return usedBy(p.id);
    }
  });

  return (
    <>
      {dialog}
      <div className="toolbar">
        <button className="primary" onClick={() => setEditing(null)}>
          {t("px.newOne")}
        </button>
        <button onClick={() => setPasting(true)}>{t("pp.paste")}</button>
        {canCheck && rows.length > 0 && (
          <button className="ghost" disabled={checking !== null} onClick={() => void checkAll()}>
            {checking ? t("px.checkingN", { done: checking.done, total: checking.total }) : t("px.checkAll", { n: rows.length })}
          </button>
        )}
        {!checking && rows.some((p) => proxyActionFor(p)?.action === "check") && (
          <span className="muted small">
            {t("px.checkSummary", {
              ok: rows.filter((p) => proxyActionFor(p)?.action === "check" && proxyActionFor(p)?.status === "ok").length,
              bad: rows.filter((p) => proxyActionFor(p)?.action === "check" && proxyActionFor(p)?.status === "error").length,
            })}
          </span>
        )}
        <div className="spacer" />
        {rows.length > 0 && (
          <button className="partnerBuy" aria-expanded={shopping} onClick={() => setShopping((v) => !v)}>
            <Icon name="tag" size={14} /> {t("partner.buy")} <span className="partnerPill">{t("partner.upTo")}</span>
          </button>
        )}
        <IconButton icon="refresh" label={t("bar.refresh")} onClick={() => void load()} />
      </div>

      {error && <div className="notice warnBar">{error}</div>}
      {unreadable.length > 0 && (
        <div className="notice warnBar">
          {t("px.vaultUnreadable", { n: unreadable.length, list: unreadable.join(", ") })}
          {stillLocked && <> {t("px.vaultStillLocked")}</>}{" "}
          <button
            className="linky"
            onClick={async () => {
              const v = await api.vaultStatus(true).catch(() => null);
              setStillLocked(v !== null && v.unreadable.length > 0);
              await load();
            }}
          >
            {t("px.vaultRetry")}
          </button>
        </div>
      )}

      {shopping && rows.length > 0 && <PartnerCard onHide={() => setShopping(false)} />}

      <div className="tableWrap">
        {rows.length === 0 ? (
          <div className="pad">
            <p className="empty">{t("px.none")}</p>
            <PartnerCard />
          </div>
        ) : (
          <table className="grid">
            <thead>
              <tr>
                <SortTh k="name" sort={sort}>{t("px.label")}</SortTh>
                <SortTh k="address" sort={sort}>{t("px.address")}</SortTh>
                <SortTh k="lastSeen" sort={sort}>{t("px.lastSeen")}</SortTh>
                <SortTh k="usedBy" sort={sort} first="desc">{t("px.usedBy")}</SortTh>
                <th />
              </tr>
            </thead>
            <tbody>
              {shown.map((p) => {
                const action = proxyActionFor(p);
                const check = action?.action === "check" ? action : undefined;
                const rotation = action?.action === "rotate" ? action : undefined;
                const exit = proxyExit(action, { ip: p.last_ip, country: p.last_country });
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
                const locked = proxyLock(p, myEmail, t);
                return (
                <tr key={p.id}>
                  <td>
                    <div className="name">{p.name}</div>
                    <div className="muted small">
                      {p.kind}
                      {p.added_by && <> · {addedByMe(p, myEmail) ? t("px.addedByYou") : t("px.addedBy", { who: p.added_by })}</>}
                    </div>
                  </td>
                  <td className="mono">
                    {p.host}:{p.port}
                    {p.username && <div className="muted small">{p.username}</div>}
                  </td>
                  <td className="muted small">
                    {exit?.ip ?? "—"}
                    {exit?.country && (
                      <>
                        {" · "}
                        <span className="flag">{flag(exit.country)}</span> {exit.country}
                      </>
                    )}
                    {check?.status === "pending" && check.afterRotation && <div>{t("px.checking")}</div>}
                    {check?.status === "ok" && <div className="ok">{t("px.checkOk", { ms: check.ms ?? 0 })}</div>}
                    {check?.checkerCountry && <div title={t("px.serverInWhy")}>{t("px.serverIn", { country: check.checkerCountry })}</div>}
                    {check?.status === "error" && (
                      <div className="warn" title={check.error}>
                        {t(check.errorCode === "noIp" ? "px.checkNoIp" : "px.checkBad")}
                      </div>
                    )}
                    {rotation?.status === "error" && <div className="warn" title={rotation.error}>{rotation.error ?? t("px.rotateFailed")}</div>}
                  </td>
                  <td className="muted">
                    {usedBy(p.id) === 0 ? "—" : t("px.usedByN", { n: usedBy(p.id) })}
                  </td>
                  <td className="actions">
                    <div>
                      {/* The same two icons a profile row has, in the same
                          places: one table's pencil is every table's pencil. */}
                      {canCheck && (
                        <IconButton
                          icon="probe"
                          label={checkLabel}
                          title={checkTitle}
                          disabled={checking !== null || action?.status === "pending"}
                          onClick={() => void onCheckProxy(p)}
                        />
                      )}
                      {canRotate && !!p.rotate_url && locked === null && (
                        <IconButton
                          icon="refresh"
                          label={rotationLabel}
                          title={rotationTitle}
                          disabled={checking !== null || action?.status === "pending"}
                          onClick={() => void onRotateProxy(p)}
                        />
                      )}
                      <IconButton icon="pencil" label={t("row.edit")} onClick={() => setEditing(p)} />
                      <IconButton
                        icon="trash"
                        danger
                        label={t("row.delete")}
                        title={locked ?? undefined}
                        disabled={locked !== null}
                        onClick={async () => {
                          const go = await ask({
                            title: t("row.delete"),
                            detail: t("px.confirmDelete", { name: p.name }),
                            confirmLabel: t("ui.delete"),
                            danger: true,
                          });
                          if (go === null) return;
                          try {
                            await api.deleteProxy(p.id);
                          } catch (e) {
                            setError(say(e));
                          }
                          await load();
                        }}
                      />
                    </div>
                  </td>
                </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      {editing !== undefined && (
        <ProxyForm
          editing={editing}
          myEmail={myEmail}
          onClose={() => setEditing(undefined)}
          onSaved={async (id) => {
            onProxySaved(id);
            setEditing(undefined);
            await load();
          }}
        />
      )}

      {pasting && (
        <ProxyPaste onClose={() => setPasting(false)} onSaved={() => void load()} />
      )}
    </>
  );
}
