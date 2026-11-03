// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { api } from "../api";
import { useI18n } from "../i18n";
import { Icon, IconButton } from "./Icon";

/** The proxy shop Fury is partnered with, agreed 03.11.2026.
 *
 *  Said to be a partner on the card itself: Fury is open source and handed to
 *  careful people, and an advert dressed as advice would cost more trust than
 *  it earns. The codes are the part that counts — the shop ties them to the
 *  referral account, so a purchase with a code counts even when the person
 *  never followed the link. Nothing is fetched or reported: the link opens in
 *  the system browser and the codes are plain text. */
const PROXYS_URL = "https://proxys.io/?refid=278406";

const PROXYS_CODES: { code: string; detail: "partner.fury10" | "partner.fury35" }[] = [
  { code: "FURY10", detail: "partner.fury10" },
  { code: "FURY35", detail: "partner.fury35" },
];

/** The card: the shop, its two codes with a copy button each, and the link.
 *  `onHide` puts a close button on it, for the copy the proxies toolbar opens. */
export function PartnerCard({ onHide }: { onHide?: () => void }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState<string | null>(null);
  const [copyFailed, setCopyFailed] = useState(false);

  const copy = async (code: string) => {
    setCopyFailed(false);
    try {
      await navigator.clipboard.writeText(code);
      setCopied(code);
      window.setTimeout(() => setCopied((c) => (c === code ? null : c)), 2000);
    } catch {
      // The code is on screen and selectable, so a refusing webview leaves
      // something to copy by hand; said rather than swallowed.
      setCopyFailed(true);
    }
  };

  return (
    <div className="partnerCard">
      <div className="productRow">
        <span className="productIcon">
          <Icon name="globe" size={18} />
        </span>
        <div className="productText">
          <div className="productName">
            proxys.io <span className="badge">{t("partner.badge")}</span>
          </div>
          <div className="muted small">{t("partner.proxysDetail")}</div>
        </div>
        <button className="primary" onClick={() => void api.openUrl(PROXYS_URL)}>
          {t("partner.open")} <Icon name="external" size={13} />
        </button>
        {onHide && <IconButton icon="close" label={t("partner.hide")} onClick={onHide} />}
      </div>
      {PROXYS_CODES.map((c) => (
        <div key={c.code} className="productRow partnerCode">
          <code className="mono">{c.code}</code>
          <span className="productText small">{t(c.detail)}</span>
          <button className="ghost" onClick={() => void copy(c.code)}>
            <Icon name="copy" size={13} /> {copied === c.code ? t("partner.copied") : t("partner.copy")}
          </button>
        </div>
      ))}
      <p className="muted small partnerNote">{copyFailed ? t("partner.copyFailed") : t("partner.note")}</p>
    </div>
  );
}
