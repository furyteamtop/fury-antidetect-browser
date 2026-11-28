// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov, devrahmanbd and the Fury authors

import { useState } from "react";
import {
  api,
  type AutomationInputDef,
  type AutomationManifest,
  type Profile,
  type RunAutomationRequest,
} from "../api";
import { useI18n } from "../i18n";

export const DEMO_URL_PRESETS: Record<string, string[]> = {
  news: [
    "https://www.bbc.com",
    "https://news.ycombinator.com",
    "https://techcrunch.com",
    "https://www.theverge.com",
    "https://www.reuters.com",
    "https://www.npr.org",
    "https://apnews.com",
    "https://www.nytimes.com",
  ],
  store: [
    "https://www.amazon.com",
    "https://www.ebay.com",
    "https://www.walmart.com",
    "https://www.target.com",
    "https://www.bestbuy.com",
    "https://www.etsy.com",
    "https://www.aliexpress.com",
    "https://www.shopify.com",
  ],
  tech: [
    "https://www.wikipedia.org",
    "https://github.com",
    "https://www.reddit.com",
    "https://stackoverflow.com",
    "https://medium.com",
    "https://www.imdb.com",
    "https://www.quora.com",
    "https://www.bloomberg.com",
    "https://archive.org",
  ],
};

export const ALL_DEMO_URLS: string[] = [
  ...DEMO_URL_PRESETS.news,
  ...DEMO_URL_PRESETS.store,
  ...DEMO_URL_PRESETS.tech,
];

export function AutomationRunDialog({
  manifest,
  profiles,
  initialProfileIds,
  onClose,
  onStarted,
}: {
  manifest: AutomationManifest;
  profiles: Profile[];
  initialProfileIds?: string[];
  onClose: () => void;
  onStarted: (jobId: string) => void;
}) {
  const { say } = useI18n();

  // Selected profiles
  const [selectedIds, setSelectedIds] = useState<string[]>(() => {
    if (initialProfileIds && initialProfileIds.length > 0) {
      return initialProfileIds;
    }
    return profiles.length > 0 ? [profiles[0].id] : [];
  });

  // Proxy settings
  const [proxyMode, setProxyMode] = useState<"profile" | "bulk" | "direct">("profile");
  const [bulkProxies, setBulkProxies] = useState("");

  // Standard script parameters
  const [concurrency, setConcurrency] = useState<number | "">(1);
  const [runCount, setRunCount] = useState<number | "">(3);
  const [minDwell, setMinDwell] = useState<number | "">(12);
  const [maxDwell, setMaxDwell] = useState<number | "">(25);
  const [customUrls, setCustomUrls] = useState("");
  const [background, setBackground] = useState(false);

  // Dynamic custom values for inputs declared in manifest.inputs
  const [customValues, setCustomValues] = useState<Record<string, any>>(() => {
    const initial: Record<string, any> = {};
    const extractDefaults = (fields?: AutomationInputDef[]) => {
      if (!fields) return;
      for (const f of fields) {
        if (f.type === "section" && f.fields) {
          extractDefaults(f.fields);
        } else if (f.key) {
          if (f.default !== undefined) {
            initial[f.key] = f.default;
          }
        }
      }
    };
    extractDefaults(manifest.inputs);
    return initial;
  });

  const setCustomField = (key: string, value: any) => {
    setCustomValues((prev) => ({ ...prev, [key]: value }));
  };

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggleProfile = (id: string) => {
    setSelectedIds((prev) =>
      prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id]
    );
  };

  const handleRun = async () => {
    if (selectedIds.length === 0) {
      setError("Please select at least one target profile.");
      return;
    }

    setBusy(true);
    setError(null);

    const urls = customUrls
      .split(/\r?\n/)
      .map((u) => u.trim())
      .filter((u) => u.startsWith("http"));

    const resolvedProxyMode = customValues["proxies"] || proxyMode;
    const resolvedBulkProxies = (resolvedProxyMode === "bulk" || proxyMode === "bulk") ? bulkProxies.trim() : undefined;

    const resolvedUrls = urls.length > 0 ? urls : (customValues["custom_urls"] ? String(customValues["custom_urls"]).split(/\r?\n/).map(u => u.trim()).filter(u => u.startsWith("http")) : undefined);
    if (manifest.id === "interactive-warmup" && (!resolvedUrls || resolvedUrls.length === 0)) {
      setError("Please provide at least one seed URL or choose a preset.");
      setBusy(false);
      return;
    }

    const resolvedConcurrency = customValues["concurrency"] !== undefined
      ? Number(customValues["concurrency"])
      : (typeof concurrency === "number" ? concurrency : (parseInt(String(concurrency), 10) || 1));

    const resolvedRunCount = customValues["sites"] !== undefined
      ? Number(customValues["sites"])
      : (customValues["run_count"] !== undefined
        ? Number(customValues["run_count"])
        : (typeof runCount === "number" ? runCount : (parseInt(String(runCount), 10) || 3)));

    const rawMinDwell = customValues["min_dwell"] !== undefined
      ? Number(customValues["min_dwell"])
      : (typeof minDwell === "number" ? minDwell : (parseInt(String(minDwell), 10) || 10));

    const rawMaxDwell = customValues["max_dwell"] !== undefined
      ? Number(customValues["max_dwell"])
      : (typeof maxDwell === "number" ? maxDwell : (parseInt(String(maxDwell), 10) || 25));

    const resolvedMinDwell = Math.max(1, rawMinDwell);
    const resolvedMaxDwell = Math.max(resolvedMinDwell, rawMaxDwell);

    const req: RunAutomationRequest = {
      automation_id: manifest.id,
      profile_ids: selectedIds,
      proxy_mode: resolvedProxyMode,
      bulk_proxies: resolvedBulkProxies,
      concurrency: resolvedConcurrency,
      run_count: resolvedRunCount,
      dwell_seconds: [resolvedMinDwell, resolvedMaxDwell],
      custom_urls: resolvedUrls,
      background: customValues["background"] !== undefined ? Boolean(customValues["background"]) : (customValues["headless"] !== undefined ? Boolean(customValues["headless"]) : background),
      headless: background,
      custom_inputs: customValues,
    };

    try {
      const job = await api.automationsRun(req);
      onStarted(job.id);
      onClose();
    } catch (e: any) {
      setError(say(e));
      setBusy(false);
    }
  };

  // Renders individual input according to its declared type
  const renderField = (field: AutomationInputDef) => {
    const key = field.key || "";
    const val = customValues[key] !== undefined ? customValues[key] : (field.default ?? "");

    switch (field.type) {
      case "slider":
        return (
          <div key={key} style={{ marginBottom: 12 }}>
            <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 4 }}>
              <label>{field.label || key}:</label>
              <span style={{ fontWeight: 600 }}>{val}</span>
            </div>
            <input
              type="range"
              min={field.min ?? 0}
              max={field.max ?? 100}
              step={field.step ?? 1}
              value={val}
              onChange={(e) => setCustomField(key, Number(e.target.value))}
              style={{ width: "100%" }}
            />
            {field.help_text && <p className="hint" style={{ margin: "2px 0 0" }}>{field.help_text}</p>}
          </div>
        );
      case "number":
        return (
          <div key={key} style={{ marginBottom: 12 }}>
            <label>{field.label || key}:</label>
            <input
              type="number"
              min={field.min}
              max={field.max}
              step={field.step ?? 1}
              value={val}
              onChange={(e) => setCustomField(key, e.target.value === "" ? "" : Number(e.target.value))}
              style={{ width: "100%" }}
            />
            {field.help_text && <p className="hint" style={{ margin: "2px 0 0" }}>{field.help_text}</p>}
          </div>
        );
      case "boolean":
      case "toggle":
        return (
          <label key={key} style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 12, cursor: "pointer" }}>
            <input
              type="checkbox"
              checked={Boolean(val)}
              onChange={(e) => setCustomField(key, e.target.checked)}
            />
            <span style={{ fontSize: "0.95em" }}>{field.label || key}</span>
          </label>
        );
      case "select":
        return (
          <div key={key} style={{ marginBottom: 12 }}>
            <label>{field.label || key}:</label>
            <select
              value={val}
              onChange={(e) => setCustomField(key, e.target.value)}
              style={{ width: "100%" }}
            >
              {(field.options || []).map((opt) => (
                <option key={opt.value} value={opt.value}>{opt.label}</option>
              ))}
            </select>
            {field.help_text && <p className="hint" style={{ margin: "2px 0 0" }}>{field.help_text}</p>}
          </div>
        );
      case "textarea": {
        const isUrlField = key === "custom_urls" || key.toLowerCase().includes("url");
        const rawText = typeof val === "string" ? val : "";
        const urlLines = rawText.split(/\r?\n/).map((s) => s.trim()).filter((s) => s.startsWith("http"));

        return (
          <div key={key} style={{ marginBottom: 12 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", marginBottom: 4 }}>
              <label style={{ margin: 0, fontWeight: 500 }}>{field.label || key}:</label>
              {isUrlField && urlLines.length > 0 && (
                <span className="hint" style={{ fontSize: "0.8em", color: "var(--accent)", fontWeight: 500 }}>
                  {urlLines.length} site{urlLines.length > 1 ? "s" : ""} selected
                </span>
              )}
            </div>

            {isUrlField && (
              <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap", marginBottom: 6 }}>
                <span style={{ fontSize: "0.78em", color: "var(--text-faint)", fontWeight: 500 }}>
                  Demo Presets:
                </span>
                <button
                  type="button"
                  className="ghost"
                  style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4 }}
                  onClick={() => setCustomField(key, DEMO_URL_PRESETS.news.join("\n"))}
                  title="Load 8 top news portals"
                >
                  News (8)
                </button>
                <button
                  type="button"
                  className="ghost"
                  style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4 }}
                  onClick={() => setCustomField(key, DEMO_URL_PRESETS.store.join("\n"))}
                  title="Load 8 top e-commerce store portals"
                >
                  Store (8)
                </button>
                <button
                  type="button"
                  className="ghost"
                  style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4 }}
                  onClick={() => setCustomField(key, DEMO_URL_PRESETS.tech.join("\n"))}
                  title="Load 9 tech and reference platforms"
                >
                  Tech (9)
                </button>
                <button
                  type="button"
                  className="ghost"
                  style={{
                    fontSize: "0.76em",
                    padding: "2px 8px",
                    borderRadius: 4,
                    borderColor: "var(--accent)",
                    color: "var(--accent)",
                    fontWeight: 500,
                  }}
                  onClick={() => setCustomField(key, ALL_DEMO_URLS.join("\n"))}
                  title="Load all 25 top sites across News, Store, and Tech"
                >
                  All (25 Sites)
                </button>
                {rawText.trim() && (
                  <button
                    type="button"
                    className="ghost"
                    style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4, marginLeft: "auto", color: "var(--text-faint)" }}
                    onClick={() => setCustomField(key, "")}
                    title="Clear URLs"
                  >
                    Clear
                  </button>
                )}
              </div>
            )}

            <textarea
              rows={isUrlField ? 4 : 3}
              placeholder={field.placeholder || (isUrlField ? "Select a demo preset above or enter URLs (1 per line)" : "")}
              value={val}
              onChange={(e) => setCustomField(key, e.target.value)}
              style={{ width: "100%", fontFamily: "monospace", fontSize: "0.85em" }}
            />
            {field.help_text && <p className="hint" style={{ margin: "2px 0 0" }}>{field.help_text}</p>}
          </div>
        );
      }
      case "text":
      case "password":
      default:
        return (
          <div key={key} style={{ marginBottom: 12 }}>
            <label>{field.label || key}:</label>
            <input
              type={field.type === "password" ? "password" : "text"}
              placeholder={field.placeholder}
              value={val}
              onChange={(e) => setCustomField(key, e.target.value)}
              style={{ width: "100%" }}
            />
            {field.help_text && <p className="hint" style={{ margin: "2px 0 0" }}>{field.help_text}</p>}
          </div>
        );
    }
  };

  // Pre-built Target Profiles Group
  const renderProfilesSection = () => (
    <div className="settingsGroup" key="profiles_section" style={{ marginBottom: "var(--s-4)" }}>
      <h3 style={{ margin: "0 0 4px", fontSize: 14 }}>Target Profiles</h3>
      <p className="hint" style={{ margin: "0 0 8px" }}>Choose which profiles this automation will drive.</p>
      <div
        style={{
          maxHeight: 140,
          overflowY: "auto",
          border: "1px solid var(--border)",
          borderRadius: "var(--r-sm)",
          padding: "6px 10px",
          background: "var(--bg)",
        }}
      >
        {profiles.length === 0 ? (
          <p className="hint" style={{ margin: "4px 0" }}>No profiles found. Create or import a profile first.</p>
        ) : (
          profiles.map((p) => {
            const checked = selectedIds.includes(p.id);
            return (
              <label
                key={p.id}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "4px 0",
                  cursor: "pointer",
                }}
              >
                <input
                  type="checkbox"
                  style={{ width: 16, height: 16, minWidth: 16, flexShrink: 0, margin: 0, accentColor: "var(--accent)" }}
                  checked={checked}
                  onChange={() => toggleProfile(p.id)}
                />
                <span style={{ fontWeight: checked ? 600 : 400 }}>{p.name}</span>
                <span className="hint" style={{ fontSize: "0.8em" }}>({p.id.slice(0, 8)})</span>
              </label>
            );
          })
        )}
      </div>
      {profiles.length > 0 && (
        <div style={{ display: "flex", gap: 8, marginTop: 6, alignItems: "center" }}>
          <button
            type="button"
            className="ghost"
            style={{ fontSize: "0.8em", padding: "2px 8px" }}
            onClick={() => setSelectedIds(profiles.map((p) => p.id))}
          >
            Select All
          </button>
          <button
            type="button"
            className="ghost"
            style={{ fontSize: "0.8em", padding: "2px 8px" }}
            onClick={() => setSelectedIds([])}
          >
            Clear
          </button>
          <span className="hint" style={{ marginLeft: "auto", fontSize: "0.8em" }}>
            {selectedIds.length} of {profiles.length} selected
          </span>
        </div>
      )}
    </div>
  );

  // Pre-built Proxy Strategy Group
  const renderProxiesSection = () => (
    <div className="settingsGroup" key="proxies_section" style={{ marginBottom: "var(--s-4)" }}>
      <h3 style={{ margin: "0 0 6px", fontSize: 14 }}>Proxy Strategy</h3>
      <div style={{ display: "flex", flexDirection: "column", gap: 8, marginBottom: 8 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
          <input
            type="radio"
            name="proxyMode"
            value="profile"
            style={{ width: 16, height: 16, minWidth: 16, flexShrink: 0, margin: 0, accentColor: "var(--accent)" }}
            checked={proxyMode === "profile"}
            onChange={() => setProxyMode("profile")}
          />
          <span style={{ fontSize: "0.92em" }}>Use each profile's assigned proxy</span>
        </label>
        <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
          <input
            type="radio"
            name="proxyMode"
            value="bulk"
            style={{ width: 16, height: 16, minWidth: 16, flexShrink: 0, margin: 0, accentColor: "var(--accent)" }}
            checked={proxyMode === "bulk"}
            onChange={() => setProxyMode("bulk")}
          />
          <span style={{ fontSize: "0.92em" }}>Bulk proxy pool (rotate across profiles)</span>
        </label>
        <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
          <input
            type="radio"
            name="proxyMode"
            value="direct"
            style={{ width: 16, height: 16, minWidth: 16, flexShrink: 0, margin: 0, accentColor: "var(--accent)" }}
            checked={proxyMode === "direct"}
            onChange={() => setProxyMode("direct")}
          />
          <span style={{ fontSize: "0.92em" }}>Direct connection (no proxy)</span>
        </label>
      </div>

      {proxyMode === "direct" && (
        <div style={{ color: "var(--danger, #ef4444)", fontSize: "0.82em", marginTop: 6, display: "flex", alignItems: "center", gap: 6, padding: "6px 10px", background: "rgba(239, 68, 68, 0.1)", borderRadius: 6 }}>
          <span>⚠️ <strong>Host Network Leak Warning:</strong> Outbound requests will exit from this machine's real public IP address. The Fury agent will refuse launching unless the profile explicitly has direct network permission enabled.</span>
        </div>
      )}

      {proxyMode === "bulk" && (
        <textarea
          rows={3}
          placeholder="127.0.0.1:8080:username:password&#10;192.168.1.1:9050"
          value={bulkProxies}
          onChange={(e) => setBulkProxies(e.target.value)}
          style={{ width: "100%", fontFamily: "monospace", fontSize: "0.85em", marginTop: 4 }}
        />
      )}
    </div>
  );

  const hasDeclaredInputs = manifest.inputs && manifest.inputs.length > 0;

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        className="modal"
        style={{ width: "min(640px, 94vw)", height: "auto", maxHeight: "88vh" }}
        role="dialog"
        aria-modal="true"
      >
        <div
          className="modalHead"
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            padding: "var(--s-4) var(--s-5) var(--s-3)",
            borderBottom: "1px solid var(--line)",
          }}
        >
          <h2 style={{ margin: 0, fontSize: 17, fontWeight: 620 }}>Run: {manifest.name}</h2>
          <button className="iconic" onClick={onClose}>✕</button>
        </div>

        <div style={{ padding: "var(--s-4) var(--s-5)", overflowY: "auto", flex: 1, minHeight: 0 }}>
          {/* Prominent Automation Description Card */}
          <div
            style={{
              background: "var(--surface-sunken, rgba(255, 255, 255, 0.04))",
              border: "1px solid var(--border)",
              borderRadius: "var(--r-sm)",
              padding: "10px 12px",
              marginBottom: "var(--s-4)",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 4 }}>
              <span style={{ fontSize: "0.78em", textTransform: "uppercase", letterSpacing: "0.06em", color: "var(--accent)", fontWeight: 620 }}>
                What this automation is for
              </span>
              <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                {manifest.runtime && (
                  <span className="badge" style={{ fontSize: "0.72em", padding: "1px 6px" }}>
                    {manifest.runtime}
                  </span>
                )}
                {manifest.version && (
                  <span className="badge" style={{ fontSize: "0.72em", padding: "1px 6px" }}>
                    v{manifest.version}
                  </span>
                )}
              </div>
            </div>
            <div style={{ fontSize: "0.88em", lineHeight: 1.45, color: "var(--text)" }}>
              {manifest.description || manifest.summary || "No description provided."}
            </div>
            {manifest.requires && manifest.requires.length > 0 && (
              <div style={{ marginTop: 6, fontSize: "0.78em", color: "var(--text-faint)" }}>
                <strong>Prerequisites:</strong> {manifest.requires.join(", ")}
              </div>
            )}
          </div>

          {error && <div className="notice warnBar" style={{ marginBottom: "var(--s-3)" }}>{error}</div>}

          {hasDeclaredInputs ? (
            // Dynamically rendered per-automation schema
            manifest.inputs!.map((item) => {
              if (item.type === "section") {
                return (
                  <div className="settingsGroup" key={item.title} style={{ marginBottom: "var(--s-4)" }}>
                    <h3 style={{ margin: "0 0 6px", fontSize: 14 }}>{item.title}</h3>
                    {item.description && <p className="hint" style={{ margin: "0 0 8px" }}>{item.description}</p>}
                    {(item.fields || []).map((f) => renderField(f))}
                  </div>
                );
              } else if (item.type === "profile_selector") {
                return renderProfilesSection();
              } else if (item.type === "proxy_selector") {
                return renderProxiesSection();
              } else {
                return renderField(item);
              }
            })
          ) : (
            // Legacy default layout for automations without explicit inputs schema
            <>
              {renderProfilesSection()}
              {renderProxiesSection()}

              <div className="settingsGroup" style={{ marginBottom: "var(--s-4)" }}>
                <h3 style={{ margin: "0 0 6px", fontSize: 14 }}>Parameters</h3>
                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "var(--s-3)", marginBottom: 10 }}>
                  <div>
                    <label style={{ display: "block", marginBottom: 4, fontWeight: 500 }}>Run count / Sites:</label>
                    <input
                      type="number"
                      min={1}
                      max={1000}
                      value={runCount}
                      placeholder="3"
                      onChange={(e) => {
                        const val = e.target.value;
                        setRunCount(val === "" ? "" : Math.max(1, parseInt(val, 10) || 1));
                      }}
                      style={{ width: "100%" }}
                    />
                    <p className="hint" style={{ margin: "2px 0 0", fontSize: "0.78em" }}>
                      Target pages per profile
                    </p>
                  </div>
                  <div>
                    <label style={{ display: "block", marginBottom: 4, fontWeight: 500 }}>Parallel concurrency:</label>
                    <input
                      type="number"
                      min={1}
                      max={64}
                      value={concurrency}
                      placeholder="1"
                      onChange={(e) => {
                        const val = e.target.value;
                        setConcurrency(val === "" ? "" : Math.max(1, parseInt(val, 10) || 1));
                      }}
                      style={{ width: "100%" }}
                    />
                    <p className="hint" style={{ margin: "2px 0 0", fontSize: "0.78em" }}>
                      Parallel profiles active simultaneously
                    </p>
                  </div>
                </div>

                <div style={{ marginBottom: 10 }}>
                  <label style={{ display: "block", marginBottom: 4, fontWeight: 500 }}>Dwell time (sec per page):</label>
                  <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
                    <div style={{ width: 110 }}>
                      <input
                        type="number"
                        min={1}
                        max={600}
                        style={{ width: "100%" }}
                        placeholder="Min (12)"
                        value={minDwell}
                        onChange={(e) => {
                          const val = e.target.value;
                          setMinDwell(val === "" ? "" : Math.max(1, parseInt(val, 10) || 1));
                        }}
                      />
                    </div>
                    <span style={{ color: "var(--text-faint)", fontWeight: 500, flexShrink: 0 }}>to</span>
                    <div style={{ width: 110 }}>
                      <input
                        type="number"
                        min={1}
                        max={1200}
                        style={{ width: "100%" }}
                        placeholder="Max (25)"
                        value={maxDwell}
                        onChange={(e) => {
                          const val = e.target.value;
                          setMaxDwell(val === "" ? "" : Math.max(1, parseInt(val, 10) || 1));
                        }}
                      />
                    </div>
                    <span style={{ color: "var(--text-dim)", fontSize: "0.85em", flexShrink: 0 }}>seconds per page</span>
                  </div>
                  <p className="hint" style={{ margin: "2px 0 0", fontSize: "0.78em" }}>
                    Randomized human dwell interval spent on each target
                  </p>
                </div>

                <div style={{ marginBottom: 8 }}>
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", marginBottom: 4 }}>
                    <label style={{ margin: 0, fontWeight: 500 }}>Custom seed URLs (optional, one per line):</label>
                    {customUrls.trim() && (
                      <span className="hint" style={{ fontSize: "0.8em", color: "var(--accent)", fontWeight: 500 }}>
                        {customUrls.split(/\r?\n/).filter((u) => u.trim().startsWith("http")).length} sites loaded
                      </span>
                    )}
                  </div>
                  <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap", marginBottom: 6 }}>
                    <span style={{ fontSize: "0.78em", color: "var(--text-faint)", fontWeight: 500 }}>
                      Demo Presets:
                    </span>
                    <button
                      type="button"
                      className="ghost"
                      style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4 }}
                      onClick={() => setCustomUrls(DEMO_URL_PRESETS.news.join("\n"))}
                      title="Load 8 top news portals"
                    >
                      News (8)
                    </button>
                    <button
                      type="button"
                      className="ghost"
                      style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4 }}
                      onClick={() => setCustomUrls(DEMO_URL_PRESETS.store.join("\n"))}
                      title="Load 8 top e-commerce store portals"
                    >
                      Store (8)
                    </button>
                    <button
                      type="button"
                      className="ghost"
                      style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4 }}
                      onClick={() => setCustomUrls(DEMO_URL_PRESETS.tech.join("\n"))}
                      title="Load 9 tech and reference platforms"
                    >
                      Tech (9)
                    </button>
                    <button
                      type="button"
                      className="ghost"
                      style={{
                        fontSize: "0.76em",
                        padding: "2px 8px",
                        borderRadius: 4,
                        borderColor: "var(--accent)",
                        color: "var(--accent)",
                        fontWeight: 500,
                      }}
                      onClick={() => setCustomUrls(ALL_DEMO_URLS.join("\n"))}
                      title="Load all 25 top sites across News, Store, and Tech"
                    >
                      All (25 Sites)
                    </button>
                    {customUrls.trim() && (
                      <button
                        type="button"
                        className="ghost"
                        style={{ fontSize: "0.76em", padding: "2px 8px", borderRadius: 4, marginLeft: "auto", color: "var(--text-faint)" }}
                        onClick={() => setCustomUrls("")}
                        title="Clear URLs"
                      >
                        Clear
                      </button>
                    )}
                  </div>
                  <textarea
                    rows={4}
                    placeholder="Select a demo preset above or enter URLs (1 per line)"
                    value={customUrls}
                    onChange={(e) => setCustomUrls(e.target.value)}
                    style={{ width: "100%", fontFamily: "monospace", fontSize: "0.85em" }}
                  />
                </div>

                <label style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 6, cursor: "pointer" }}>
                  <input
                    type="checkbox"
                    style={{ width: 16, height: 16, minWidth: 16, flexShrink: 0, margin: 0, accentColor: "var(--accent)" }}
                    checked={background}
                    onChange={(e) => setBackground(e.target.checked)}
                  />
                  <span style={{ fontSize: "0.9em" }}>Run in background (window opened unfocused/hidden, non-headless)</span>
                </label>
              </div>
            </>
          )}
        </div>

        <div className="modalFoot" style={{ justifyContent: "flex-end" }}>
          <button className="ghost" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="primary" onClick={handleRun} disabled={busy}>
            {busy ? "Starting…" : "Run Automation"}
          </button>
        </div>
      </div>
    </div>
  );
}
