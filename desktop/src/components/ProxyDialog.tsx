import { useState } from "react";
import { api, type LocalProxy } from "../api";

const KINDS = ["socks5", "http", "https"] as const;

interface CheckResult {
  ok: boolean;
  error?: string;
  ip?: string;
  country?: string;
  city?: string;
  timezone?: string;
  org?: string;
  ms?: number;
}

/** Adding or editing a proxy.
 *
 *  The check is the reason this is a dialog and not a row of inputs. A proxy
 *  that does not work turns into a profile that will not open, and finding that
 *  out at launch — after the persona is chosen and the account is warmed — is
 *  the wrong moment. Worse, the exit's country and time zone are what the
 *  profile has to agree with: a profile leaving in Germany while reporting
 *  Asia/Tbilisi is the cheapest detection in the industry, and the only way to
 *  know where it actually leaves is to ask. */
export function ProxyDialog({
  editing,
  onClose,
  onSaved,
}: {
  editing: LocalProxy | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [name, setName] = useState(editing?.name ?? "");
  const [kind, setKind] = useState(editing?.kind ?? "socks5");
  const [host, setHost] = useState(editing?.host ?? "");
  const [port, setPort] = useState(String(editing?.port ?? ""));
  const [username, setUsername] = useState(editing?.username ?? "");
  const [password, setPassword] = useState(editing?.password ?? "");
  const [check, setCheck] = useState<CheckResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const complete = host.trim() !== "" && Number(port) > 0;

  const url = () => {
    const auth = username ? `${encodeURIComponent(username)}:${encodeURIComponent(password)}@` : "";
    return `${kind}://${auth}${host.trim()}:${Number(port)}`;
  };

  const runCheck = async () => {
    setBusy(true);
    setCheck(null);
    setError(null);
    try {
      setCheck(await api.checkProxy(url()));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.saveProxy({
        id: editing?.id ?? "",
        name: name.trim() || `${host.trim()}:${port}`,
        kind,
        host: host.trim(),
        port: Number(port),
        username: username || null,
        password: password || null,
        last_country: check?.country ?? editing?.last_country ?? null,
        last_ip: check?.ip ?? editing?.last_ip ?? null,
      });
      onSaved();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh" }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{editing ? "Edit proxy" : "New proxy"}</h2>
        </div>

        <div className="form" style={{ paddingTop: "var(--s-5)" }}>
          <div className="field">
            <label>Type</label>
            <div className="segmented">
              {KINDS.map((k) => (
                <button key={k} aria-pressed={kind === k} onClick={() => setKind(k)}>
                  {k}
                </button>
              ))}
            </div>
          </div>

          <div className="field">
            <label htmlFor="x-host">Address</label>
            <div className="row">
              <input
                id="x-host"
                value={host}
                autoFocus
                placeholder="exit.provider.net"
                onChange={(e) => setHost(e.target.value)}
              />
              <input
                style={{ width: 92 }}
                value={port}
                placeholder="1080"
                inputMode="numeric"
                onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
              />
            </div>
          </div>

          <div className="field">
            <label htmlFor="x-user">Credentials</label>
            <div className="row">
              <input
                id="x-user"
                value={username}
                placeholder="user (optional)"
                autoComplete="off"
                onChange={(e) => setUsername(e.target.value)}
              />
              <input
                value={password}
                placeholder="password"
                type="password"
                autoComplete="off"
                onChange={(e) => setPassword(e.target.value)}
              />
            </div>
          </div>

          <div className="field">
            <label htmlFor="x-name">Label</label>
            <div>
              <input
                id="x-name"
                value={name}
                placeholder={host ? `${host}:${port || "…"}` : "EU residential"}
                onChange={(e) => setName(e.target.value)}
              />
              <p className="hint">
                What you will see in the profile list. Defaults to the address.
              </p>
            </div>
          </div>

          <div className="field">
            <label>Check</label>
            <div>
              <button disabled={busy || !complete} onClick={runCheck}>
                {busy ? "Checking…" : "Where does this come out?"}
              </button>
              {check && (
                <div
                  className={check.ok ? "verdict good" : "verdict bad"}
                  style={{ marginTop: "var(--s-2)" }}
                >
                  {check.ok ? (
                    <>
                      {check.ip}
                      {check.country ? ` · ${check.country}` : ""}
                      {check.city ? `, ${check.city}` : ""}
                      {check.timezone ? ` · ${check.timezone}` : ""}
                      {check.ms !== undefined ? ` · ${check.ms} ms` : ""}
                      {check.org ? (
                        <div className="small" style={{ opacity: 0.8 }}>{check.org}</div>
                      ) : null}
                    </>
                  ) : (
                    check.error
                  )}
                </div>
              )}
              <p className="hint">
                {/* Said plainly because it is the only outbound call the agent
                    makes on its own behalf, and an operator checking a proxy
                    should know they are telling someone it exists. */}
                Asks a geo service through this proxy — the exit IP is something
                only the far end can report. Point <code>FURY_IP_CHECK</code> at your
                own if you would rather not tell a third party.
              </p>
              {check?.ok && check.timezone && (
                <p className="hint">
                  Set the profile's time zone to <strong>{check.timezone}</strong> to
                  match where it actually leaves.
                </p>
              )}
            </div>
          </div>
        </div>

        <div className="modalFoot">
          {error && <span className="error">{error}</span>}
          <div className="spacer" />
          <button className="ghost" onClick={onClose}>
            Cancel
          </button>
          <button className="primary" disabled={busy || !complete} onClick={save}>
            {editing ? "Save" : "Add"}
          </button>
        </div>
      </div>
    </div>
  );
}
