// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type CatalogueEntry, type Extension, type ExtensionEverywhere, type ExtensionPackage, type KeySource, type Profile } from "../api";
import { useI18n } from "../i18n";
import { keyNote, PackageButtons, packageError, skipReason } from "./ExtensionPackage";
import { Select } from "./Select";

const ID_IN_TEXT = /[a-p]{32}/;

/** The Extensions tab of the profile editor (issue 29): what this profile has,
 *  and adding from the four places an extension comes from, without leaving
 *  the editor -- the catalogue, the Web Store by id, another profile on this
 *  machine, a file or folder.
 *
 *  Everything goes through the same agent calls the Extensions section uses,
 *  with this one profile as the target, so the rules are the same: a store
 *  package comes through this profile's proxy (a team profile's through its
 *  lock), and an open profile is skipped. From another profile copies the
 *  extension WITHOUT its storage: a new persona signed into the source's wallet
 *  is two personas tied together. Copying with the login stays in the
 *  Extensions section, where it says so. */
export function ProfileExtensions({ profile }: { profile: Profile }) {
  const { t, say } = useI18n();
  const [list, setList] = useState<Extension[] | null>(null);
  const [catalogue, setCatalogue] = useState<CatalogueEntry[]>([]);
  const [elsewhere, setElsewhere] = useState<ExtensionEverywhere[]>([]);
  const [fromCatalogue, setFromCatalogue] = useState("");
  const [fromProfile, setFromProfile] = useState("");
  const [byId, setById] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const open = profile.running;

  const reload = async () => {
    try {
      const [mine, all] = await Promise.all([api.extensions(profile.id), api.allExtensions()]);
      setList(mine);
      setElsewhere(all.filter((x) => !mine.some((m) => m.id === x.id) && x.profiles.some((p) => p.id !== profile.id)));
    } catch (e) {
      setError(say(e));
    }
  };

  useEffect(() => {
    void reload();
    api.extensionCatalogue().then(setCatalogue, (e) => setError(say(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile.id]);

  /** One profile asked for, so one answer: added, or why not. */
  const run = async (
    work: () => Promise<{ extension: { id: string; name: string } | null; installed: string[]; skipped: { id: string; reason: string }[]; key_source?: KeySource }>,
    fallback: string,
  ) => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const r = await work();
      const name = r.extension?.name ?? fallback;
      if (r.installed.includes(profile.id)) {
        setNote([t("pe.added", { name }), keyNote(t, r.key_source, name, r.extension?.id)].filter(Boolean).join(" "));
      } else {
        const why = r.skipped.find((s) => s.id === profile.id)?.reason ?? "";
        setError(why === "open" ? t("pe.open") : skipReason(t, why));
      }
      await reload();
    } catch (e) {
      setError(packageError(t, say, e));
    } finally {
      setBusy(false);
    }
  };

  const fromStore = (id: string, name: string) =>
    run(
      () =>
        profile.origin === "team"
          ? api.installExtensionFromStoreTeam([profile.id], id)
          : api.installExtensionFromStore([profile.id], id),
      name,
    );

  const fromPackage = (pkg: ExtensionPackage, label: string) =>
    run(() => api.installExtensionMany([profile.id], pkg), label);

  const copyFrom = (extId: string) => {
    const x = elsewhere.find((e) => e.id === extId);
    const source = x?.profiles.find((p) => p.id !== profile.id);
    if (!x || !source) return;
    return run(() => api.copyExtensionWithState(source.id, extId, [profile.id], false), x.name);
  };

  const pastedId = byId.match(ID_IN_TEXT)?.[0] ?? null;
  const notHere = catalogue.filter((c) => !list?.some((x) => x.id === c.id));
  const disabled = busy || open;

  return (
    <>
      <div className="field">
        <label>{t("pe.installed")}</label>
        <div>
          {list === null ? (
            <p className="hint">{t("ext.loading")}</p>
          ) : list.length === 0 ? (
            <p className="hint">{t("ext.none")}</p>
          ) : (
            <table className="plain">
              <tbody>
                {list.map((x) => (
                  <tr key={x.id}>
                    <td>
                      <strong>{x.name}</strong> <span className="muted">{x.version}</span>
                      <div className="muted mono" style={{ fontSize: 11 }}>
                        {x.id}
                      </div>
                    </td>
                    <td style={{ textAlign: "right" }}>
                      <button
                        className="ghost danger"
                        disabled={disabled}
                        onClick={async () => {
                          setBusy(true);
                          setError(null);
                          setNote(null);
                          try {
                            await api.removeExtension(profile.id, x.id);
                            await reload();
                          } catch (e) {
                            setError(say(e));
                          } finally {
                            setBusy(false);
                          }
                        }}
                      >
                        {t("ext.remove")}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {open && <p className="hint warn">{t("pe.open")}</p>}
        </div>
      </div>

      <div className="field">
        <label>{t("exv.catalogue")}</label>
        <div className="row">
          <Select value={fromCatalogue} onChange={(e) => setFromCatalogue(e.target.value)} disabled={disabled || notHere.length === 0}>
            <option value="">{t("pe.choose")}</option>
            {notHere.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </Select>
          <button
            disabled={disabled || !fromCatalogue}
            onClick={() => {
              const c = catalogue.find((x) => x.id === fromCatalogue);
              if (c) void fromStore(c.id, c.name).then(() => setFromCatalogue(""));
            }}
          >
            {t("pe.add")}
          </button>
        </div>
      </div>

      <div className="field">
        <label>{t("pe.byId")}</label>
        <div className="row">
          <input
            value={byId}
            spellCheck={false}
            disabled={disabled}
            placeholder="https://chromewebstore.google.com/detail/…/hlkenndednhfkekhgcdicdfddnkalmdm"
            style={{ fontFamily: "var(--mono)", fontSize: 12 }}
            onChange={(e) => setById(e.target.value)}
          />
          <button
            disabled={disabled || !pastedId}
            onClick={() => pastedId && void fromStore(pastedId, pastedId.slice(0, 8) + "…").then(() => setById(""))}
          >
            {t("pe.add")}
          </button>
        </div>
      </div>

      {elsewhere.length > 0 && (
        <div className="field">
          <label>{t("pe.fromProfile")}</label>
          <div>
            <div className="row">
              <Select value={fromProfile} onChange={(e) => setFromProfile(e.target.value)} disabled={disabled}>
                <option value="">{t("pe.choose")}</option>
                {elsewhere.map((x) => (
                  <option key={x.id} value={x.id}>
                    {x.name} {x.version}
                  </option>
                ))}
              </Select>
              <button
                disabled={disabled || !fromProfile}
                onClick={() => void copyFrom(fromProfile)?.then(() => setFromProfile(""))}
              >
                {t("pe.add")}
              </button>
            </div>
            <p className="hint">{t("pe.fromProfileHint")}</p>
          </div>
        </div>
      )}

      <div className="field">
        <label>{t("pe.fromFile")}</label>
        <div>
          <div className="row">
            <PackageButtons
              disabled={disabled}
              fileLabel={busy ? t("ck.working") : t("ext.addFile")}
              folderLabel={t("ext.addFolder")}
              onPicked={(pkg, label) => void fromPackage(pkg, label)}
              onError={(e) => setError(packageError(t, say, e))}
            />
          </div>
          <p className="hint">{t("ext.hintAny")}</p>
        </div>
      </div>

      {note && (
        <div className="field">
          <div />
          <p>{note}</p>
        </div>
      )}
      {error && (
        <div className="field">
          <div />
          <p className="error">{error}</p>
        </div>
      )}
    </>
  );
}
