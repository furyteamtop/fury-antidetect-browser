import { useCallback, useRef, useState } from "react";
import { useI18n } from "../i18n";

interface Request {
  title: string;
  detail?: string;
  /** Present for a prompt; absent for a plain confirmation. */
  placeholder?: string;
  initial?: string;
  confirmLabel: string;
  danger?: boolean;
}

/** Confirmations and prompts, drawn by the application.
 *
 *  Not `window.confirm`. wry ships no JavaScript dialog delegate at all — no
 *  alert, no confirm, no prompt — and WKWebView answers `false` and `null`
 *  immediately when the host does not implement those panels. So every native
 *  dialog in this app was a silent no-op: pressing Delete did nothing, and
 *  creating a project did nothing, with no error anywhere to explain it.
 *
 *  Drawing them here is what the app should have done regardless. A system
 *  dialog cannot be translated with the rest of the interface, ignores the
 *  theme, and says "localhost wants to…" in a window that is not a browser. */
export function useAsk(): {
  ask: (req: Request) => Promise<string | null>;
  dialog: React.ReactNode;
} {
  const { t } = useI18n();
  const [request, setRequest] = useState<Request | null>(null);
  const [value, setValue] = useState("");
  const resolver = useRef<((v: string | null) => void) | null>(null);

  const ask = useCallback((req: Request) => {
    setRequest(req);
    setValue(req.initial ?? "");
    return new Promise<string | null>((resolve) => {
      resolver.current = resolve;
    });
  }, []);

  const settle = (v: string | null) => {
    setRequest(null);
    resolver.current?.(v);
    resolver.current = null;
  };

  const dialog = request ? (
    <div
      className="scrim"
      onMouseDown={(e) => e.target === e.currentTarget && settle(null)}
    >
      <form
        className="palette"
        style={{ maxHeight: "none" }}
        role="dialog"
        aria-modal="true"
        onSubmit={(e) => {
          e.preventDefault();
          // A prompt with nothing typed is a cancel, not an empty name.
          settle(request.placeholder !== undefined ? value.trim() || null : "");
        }}
      >
        <div style={{ padding: "var(--s-5) var(--s-5) var(--s-3)" }}>
          <div className="name" style={{ marginBottom: "var(--s-2)" }}>
            {request.title}
          </div>
          {request.detail && <p className="hint" style={{ margin: 0 }}>{request.detail}</p>}
          {request.placeholder !== undefined && (
            <input
              style={{ marginTop: "var(--s-3)" }}
              value={value}
              autoFocus
              placeholder={request.placeholder}
              onChange={(e) => setValue(e.target.value)}
            />
          )}
        </div>
        <div className="modalFoot">
          <div className="spacer" />
          <button type="button" className="ghost" onClick={() => settle(null)}>
            {t("ui.cancel")}
          </button>
          <button
            type="submit"
            className={request.danger ? "danger" : "primary"}
            // Autofocus goes to the input when there is one; otherwise the
            // action gets it, so Enter and Escape both work with no mouse.
            autoFocus={request.placeholder === undefined}
            disabled={request.placeholder !== undefined && !value.trim()}
          >
            {request.confirmLabel}
          </button>
        </div>
      </form>
    </div>
  ) : null;

  return { ask, dialog };
}
