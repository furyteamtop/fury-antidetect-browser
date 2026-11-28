// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov, devrahmanbd and the Fury authors

import { useEffect, useRef, useState } from "react";
import {
  api,
  type ActiveJobInfo,
  type AutomationManifest,
  type Profile,
} from "../api";
import { useI18n } from "../i18n";
import { AutomationRunDialog } from "./AutomationRunDialog";

export function AutomationsView({
  profiles,
  initialProfileIds,
}: {
  profiles: Profile[];
  initialProfileIds?: string[];
}) {
  const { say } = useI18n();

  const [automations, setAutomations] = useState<AutomationManifest[]>([]);
  const [activeJobs, setActiveJobs] = useState<ActiveJobInfo[]>([]);
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<"all" | "builtin" | "custom">("all");
  const [runningTarget, setRunningTarget] = useState<AutomationManifest | null>(null);
  const [viewingDocs, setViewingDocs] = useState<AutomationManifest | null>(null);
  const [viewingLogs, setViewingLogs] = useState<AutomationManifest | null>(null);
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [copySuccess, setCopySuccess] = useState(false);
  const logContainerRef = useRef<HTMLDivElement>(null);

  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const [importSuccess, setImportSuccess] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const fileInputRef = useRef<HTMLInputElement>(null);

  const loadData = async () => {
    try {
      const [list, jobs] = await Promise.all([
        api.automationsList(),
        api.automationsStatus(),
      ]);
      setAutomations(list);
      setActiveJobs(jobs);
    } catch (e) {
      setError(say(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadData();
    const interval = setInterval(async () => {
      try {
        const jobs = await api.automationsStatus();
        setActiveJobs(jobs);
      } catch {}
    }, 1000);
    return () => clearInterval(interval);
  }, []);

  useEffect(() => {
    if (logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [activeJobs, viewingLogs, selectedJobId]);

  const handleStop = async (jobId: string) => {
    try {
      await api.automationsStop(jobId);
      const jobs = await api.automationsStatus();
      setActiveJobs(jobs);
    } catch (e) {
      setError(say(e));
    }
  };

  const handleImport = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    try {
      setImporting(true);
      setError(null);

      const reader = new FileReader();
      reader.onload = async () => {
        try {
          const res = reader.result as string;
          const base64 = res.split(",")[1];
          const imported = await api.automationsImportB64(file.name, base64);
          setAutomations((prev) => [imported, ...prev.filter((a) => a.id !== imported.id)]);
          setImportSuccess(`Imported "${imported.name}" successfully!`);
          setTimeout(() => setImportSuccess(null), 3000);
        } catch (err: any) {
          setError(say(err));
        } finally {
          setImporting(false);
          if (fileInputRef.current) fileInputRef.current.value = "";
        }
      };
      reader.readAsDataURL(file);
    } catch (err: any) {
      setError(say(err));
      setImporting(false);
    }
  };

  const handleExport = async (auto: AutomationManifest) => {
    try {
      setError(null);
      const b64 = await api.automationsExportB64(auto.id);
      const byteCharacters = atob(b64);
      const byteNumbers = new Array(byteCharacters.length);
      for (let i = 0; i < byteCharacters.length; i++) {
        byteNumbers[i] = byteCharacters.charCodeAt(i);
      }
      const byteArray = new Uint8Array(byteNumbers);
      const blob = new Blob([byteArray], { type: "application/octet-stream" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${auto.id}.fury`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
    } catch (err: any) {
      setError(say(err));
    }
  };

  const filtered = automations.filter((a) => {
    const matchesSearch =
      a.name.toLowerCase().includes(search.toLowerCase()) ||
      a.summary.toLowerCase().includes(search.toLowerCase()) ||
      (a.description || "").toLowerCase().includes(search.toLowerCase()) ||
      a.author.toLowerCase().includes(search.toLowerCase());
    if (!matchesSearch) return false;
    if (filter === "builtin") {
      return a.is_builtin;
    }
    if (filter === "custom") {
      return !a.is_builtin;
    }
    return true;
  });

  return (
    <div className="settings" style={{ overflowY: "auto", paddingBottom: "var(--s-6)" }}>
      {/* Hidden file input for .fury import */}
      <input
        ref={fileInputRef}
        type="file"
        accept=".fury,.zip"
        style={{ display: "none" }}
        onChange={handleImport}
      />

      {/* Top Header bar */}
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "var(--s-3)", flexWrap: "wrap", gap: 8 }}>
        <div>
          <p style={{ margin: "0 0 4px" }}>
            Community & built-in automations driven over Fury's Local API and CDP.
          </p>
          <span className="muted" style={{ fontSize: "0.85em" }}>
            {automations.length} automations available
            {initialProfileIds && initialProfileIds.length > 0 && ` · ${initialProfileIds.length} profiles pre-selected`}
          </span>
        </div>

        <div style={{ display: "flex", gap: 8 }}>
          <button
            className="secondary"
            disabled={importing}
            onClick={() => fileInputRef.current?.click()}
          >
            {importing ? "Importing…" : "⬇ Import .fury"}
          </button>
        </div>
      </div>

      {importSuccess && (
        <div className="notice" style={{ marginBottom: "var(--s-3)", background: "var(--green-bg, #052e16)", color: "var(--green, #4ade80)" }}>
          {importSuccess}
        </div>
      )}

      {error && <div className="notice warnBar" style={{ marginBottom: "var(--s-3)" }}>{error}</div>}


      {/* Active Runs Drawer */}
      {activeJobs.length > 0 && (
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: "var(--r-2)",
            padding: "var(--s-3)",
            marginBottom: "var(--s-4)",
            background: "var(--surface)",
          }}
        >
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
            <h3 style={{ margin: 0, display: "flex", alignItems: "center", gap: 8 }}>
              <span>▶ Active Runs ({activeJobs.filter((j) => j.status === "running").length})</span>
            </h3>
          </div>

          <div style={{ display: "flex", flexDirection: "column", gap: "var(--s-2)" }}>
            {activeJobs.map((job) => (
              <div
                key={job.id}
                style={{
                  border: "1px solid var(--border)",
                  borderRadius: 4,
                  padding: "8px 12px",
                  background: "var(--surface-sunken)",
                }}
              >
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", flexWrap: "wrap", gap: 8 }}>
                  <div style={{ display: "flex", alignItems: "center", flexWrap: "wrap", gap: 8 }}>
                    <strong
                      style={{ cursor: "pointer", textDecoration: "underline", textUnderlineOffset: 3 }}
                      title="Click to view action logs"
                      onClick={() => {
                        const found = automations.find((a) => a.id === job.automation_id || a.name.toLowerCase() === job.automation_name.toLowerCase());
                        if (found) {
                          setSelectedJobId(job.id);
                          setViewingLogs(found);
                        }
                      }}
                    >
                      {job.automation_name}
                    </strong>
                    <span
                      style={{
                        fontSize: "0.8em",
                        padding: "2px 6px",
                        borderRadius: 3,
                        background:
                          job.status === "running"
                            ? "var(--green, #22c55e)"
                            : job.status === "finished"
                            ? "var(--accent)"
                            : job.status === "failed"
                            ? "var(--danger, #ef4444)"
                            : "var(--muted)",
                        color: "#fff",
                        fontWeight: 650,
                      }}
                    >
                      {job.status.toUpperCase()}
                    </span>
                    <span className="muted" style={{ fontSize: "0.8em" }}>
                      Profiles: {job.profile_ids.length > 0 ? job.profile_ids.join(", ") : "none"}
                    </span>
                  </div>

                  <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                    <button
                      className="ghost"
                      style={{ fontSize: "0.8em", padding: "2px 8px" }}
                      onClick={() => {
                        const found = automations.find((a) => a.id === job.automation_id || a.name.toLowerCase() === job.automation_name.toLowerCase());
                        if (found) {
                          setSelectedJobId(job.id);
                          setViewingLogs(found);
                        }
                      }}
                    >
                      ⤢ Full View
                    </button>
                    {job.status === "running" && (
                      <button
                        className="danger"
                        style={{ fontSize: "0.8em", padding: "2px 8px" }}
                        onClick={() => handleStop(job.id)}
                      >
                        Stop
                      </button>
                    )}
                  </div>
                </div>

                {/* Log snippet */}
                <div
                  style={{
                    marginTop: 8,
                    background: "#18181b",
                    color: "#a1a1aa",
                    padding: "6px 10px",
                    borderRadius: 4,
                    fontSize: "0.8em",
                    fontFamily: "monospace",
                    maxHeight: 120,
                    overflowY: "auto",
                  }}
                >
                  {job.logs.slice(-6).map((line, idx) => (
                    <div key={idx}>{line}</div>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Filter and Search Bar */}
      <div style={{ display: "flex", gap: "var(--s-2)", marginBottom: "var(--s-3)", flexWrap: "wrap", alignItems: "center" }}>
        <input
          type="text"
          placeholder="Search automations by name, tag, or author..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ flex: 1, minWidth: 200 }}
        />

        <div style={{ display: "flex", gap: 4 }}>
          {(["all", "builtin", "custom"] as const).map((f) => (
            <button
              key={f}
              className={filter === f ? "pill active" : "pill"}
              onClick={() => setFilter(f)}
              style={{
                padding: "4px 10px",
                borderRadius: "var(--r-pill)",
                border: "1px solid var(--border)",
                background: filter === f ? "var(--accent)" : "transparent",
                color: filter === f ? "#fff" : "inherit",
                cursor: "pointer",
                fontSize: "0.85em",
              }}
            >
              {f === "all" ? "All" : f === "builtin" ? "Built-in" : "Custom"}
            </button>
          ))}
        </div>
      </div>

      {/* Automations Cards Grid */}
      {loading ? (
        <p className="muted">Loading automations…</p>
      ) : filtered.length === 0 ? (
        <p className="muted">No matching automations found.</p>
      ) : (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(320px, 1fr))", gap: "var(--s-3)" }}>
          {filtered.map((auto) => {
            const matchingJobs = activeJobs.filter(
              (j) => j.automation_id === auto.id || j.automation_name.toLowerCase() === auto.name.toLowerCase()
            );
            const latestJob =
              matchingJobs.find((j) => j.status === "running") ||
              (matchingJobs.length > 0 ? matchingJobs[matchingJobs.length - 1] : null);

            return (
              <div
                key={auto.id}
                style={{
                  border: latestJob?.status === "running" ? "1px solid var(--green, #22c55e)" : "1px solid var(--border)",
                  borderRadius: "var(--r-2)",
                  padding: "var(--s-3)",
                  display: "flex",
                  flexDirection: "column",
                  justifyContent: "space-between",
                  background: "var(--surface)",
                  boxShadow: latestJob?.status === "running" ? "0 0 12px rgba(34, 197, 94, 0.15)" : undefined,
                  transition: "box-shadow 0.2s ease, border-color 0.2s ease",
                }}
              >
                <div>
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 6 }}>
                    <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                      <h3
                        role="button"
                        tabIndex={0}
                        title="Click to view action logs"
                        onClick={() => {
                          setSelectedJobId(latestJob?.id || null);
                          setViewingLogs(auto);
                        }}
                        style={{
                          margin: 0,
                          fontSize: "1.1em",
                          cursor: "pointer",
                          display: "inline-flex",
                          alignItems: "center",
                          gap: 6,
                          color: "var(--text)",
                          transition: "color 0.15s ease",
                        }}
                        onMouseEnter={(e) => (e.currentTarget.style.color = "var(--accent)")}
                        onMouseLeave={(e) => (e.currentTarget.style.color = "var(--text)")}
                      >
                        <span style={{ textDecoration: "underline", textDecorationStyle: "dotted", textUnderlineOffset: 3 }}>
                          {auto.name}
                        </span>
                        {auto.version && (
                          <span style={{ fontSize: "0.8em", color: "var(--muted)", fontWeight: 400 }}>
                            v{auto.version}
                          </span>
                        )}
                      </h3>

                      {latestJob && (
                        <span
                          role="button"
                          tabIndex={0}
                          title="Click to view action logs"
                          onClick={() => {
                            setSelectedJobId(latestJob.id);
                            setViewingLogs(auto);
                          }}
                          style={{
                            cursor: "pointer",
                            fontSize: "0.72em",
                            padding: "2px 6px",
                            borderRadius: 3,
                            fontWeight: 650,
                            letterSpacing: "0.03em",
                            background:
                              latestJob.status === "running"
                                ? "var(--green, #22c55e)"
                                : latestJob.status === "finished"
                                ? "var(--accent, #8b5cf6)"
                                : latestJob.status === "failed"
                                ? "var(--danger, #ef4444)"
                                : "var(--muted)",
                            color: "#fff",
                          }}
                        >
                          {latestJob.status.toUpperCase()}
                        </span>
                      )}
                    </div>

                    <span className="muted" style={{ fontSize: "0.75em", background: "var(--surface-sunken)", padding: "2px 6px", borderRadius: 4 }}>
                      by {auto.author}
                    </span>
                  </div>

                  <p style={{ fontSize: "0.9em", color: "var(--muted)", margin: "0 0 12px", minHeight: 40 }}>
                    {auto.summary || auto.description}
                  </p>

                  {/* Live Mini Console on Card when Running */}
                  {latestJob && latestJob.status === "running" && (
                    <div
                      role="button"
                      tabIndex={0}
                      onClick={() => {
                        setSelectedJobId(latestJob.id);
                        setViewingLogs(auto);
                      }}
                      title="Click to open live action logs"
                      style={{
                        cursor: "pointer",
                        background: "#18181b",
                        color: "#a1a1aa",
                        padding: "8px 10px",
                        borderRadius: 4,
                        fontSize: "0.78em",
                        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
                        marginBottom: 12,
                        border: "1px solid rgba(34, 197, 94, 0.4)",
                        boxShadow: "0 0 10px rgba(34, 197, 94, 0.1)",
                      }}
                    >
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
                        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                          <span style={{ display: "inline-block", width: 7, height: 7, borderRadius: "50%", background: "var(--green, #22c55e)", boxShadow: "0 0 6px var(--green, #22c55e)" }} />
                          <span style={{ color: "var(--green, #22c55e)", fontWeight: 650, fontSize: "0.9em" }}>RUNNING</span>
                          <span className="muted" style={{ fontSize: "0.85em" }}>Profiles: {latestJob.profile_ids.join(", ") || "none"}</span>
                        </div>
                        <span style={{ fontSize: "0.8em", color: "var(--accent)" }}>View Logs ⤢</span>
                      </div>
                      <div style={{ maxHeight: 60, overflow: "hidden", color: "#e4e4e7", lineHeight: 1.4 }}>
                        {latestJob.logs.slice(-3).map((line, idx) => (
                          <div key={idx} style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                            {line}
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                  <div style={{ display: "flex", gap: 4, flexWrap: "wrap", marginBottom: 12 }}>
                    {(auto.drives || []).map((d) => (
                      <span
                        key={d}
                        style={{
                          fontSize: "0.75em",
                          padding: "2px 6px",
                          borderRadius: 3,
                          background: "var(--surface-sunken)",
                          border: "1px solid var(--border)",
                        }}
                      >
                        {d}
                      </span>
                    ))}
                    {auto.is_builtin ? (
                      <span style={{ fontSize: "0.75em", padding: "2px 6px", borderRadius: 3, background: "var(--surface-sunken)", color: "var(--green)" }}>
                        verified
                      </span>
                    ) : (
                      <span style={{ fontSize: "0.75em", padding: "2px 6px", borderRadius: 3, background: "var(--surface-sunken)", color: "var(--accent)" }}>
                        custom
                      </span>
                    )}
                  </div>
                </div>

                <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
                  <button
                    className="ghost"
                    style={{ fontSize: "0.85em" }}
                    title="Export as .fury archive"
                    onClick={() => handleExport(auto)}
                  >
                    Export
                  </button>
                  <button className="ghost" style={{ fontSize: "0.85em" }} onClick={() => setViewingDocs(auto)}>
                    Docs
                  </button>
                  <button
                    className={latestJob?.status === "running" ? "secondary" : "ghost"}
                    style={{
                      fontSize: "0.85em",
                      color: latestJob?.status === "running" ? "var(--green, #22c55e)" : undefined,
                      fontWeight: latestJob?.status === "running" ? 650 : undefined,
                    }}
                    onClick={() => {
                      setSelectedJobId(latestJob?.id || null);
                      setViewingLogs(auto);
                    }}
                  >
                    {latestJob?.status === "running" ? "● Logs" : "Logs"}
                  </button>
                  <button className="primary" style={{ fontSize: "0.85em" }} onClick={() => setRunningTarget(auto)}>
                    Run…
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {/* Run Automation Dialog Modal */}
      {runningTarget && (
        <AutomationRunDialog
          manifest={runningTarget}
          profiles={profiles}
          initialProfileIds={initialProfileIds}
          onClose={() => setRunningTarget(null)}
          onStarted={async (jobId?: string) => {
            const target = runningTarget;
            const jobs = await api.automationsStatus();
            setActiveJobs(jobs);
            if (target) {
              setSelectedJobId(jobId || null);
              setViewingLogs(target);
            }
          }}
        />
      )}

      {/* Docs Modal */}
      {viewingDocs && (
        <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && setViewingDocs(null)}>
          <div
            className="modal"
            style={{ width: "min(640px, 94vw)", height: "auto", maxHeight: "80vh" }}
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
              <h2 style={{ margin: 0, fontSize: 17, fontWeight: 620 }}>{viewingDocs.name}</h2>
              <button className="iconic" onClick={() => setViewingDocs(null)}>✕</button>
            </div>
            <div style={{ padding: "var(--s-4) var(--s-5)", overflowY: "auto", flex: 1, minHeight: 0 }}>
              <p><strong>Author:</strong> {viewingDocs.author}</p>
              <p><strong>License:</strong> {viewingDocs.licence}</p>
              <p><strong>Path:</strong> <code>{viewingDocs.path}</code></p>
              <p><strong>Entry Script:</strong> <code>{viewingDocs.entry}</code></p>
              <p><strong>Requirements:</strong> {(viewingDocs.requires || []).join(", ") || "None"}</p>
              <div style={{ marginTop: 16 }}>
                <h4 style={{ margin: "0 0 6px" }}>Description & Notes</h4>
                <p style={{ whiteSpace: "pre-wrap", color: "var(--text-dim)" }}>
                  {viewingDocs.description || viewingDocs.summary}
                </p>
              </div>
            </div>
            <div className="modalFoot" style={{ justifyContent: "flex-end" }}>
              <button className="primary" onClick={() => setViewingDocs(null)}>Close</button>
            </div>
          </div>
        </div>
      )}

      {/* Action Logs Modal */}
      {viewingLogs && (() => {
        const auto = viewingLogs;
        const matchingJobs = activeJobs.filter(
          (j) => j.automation_id === auto.id || j.automation_name.toLowerCase() === auto.name.toLowerCase()
        );
        const latestJob =
          (selectedJobId && matchingJobs.find((j) => j.id === selectedJobId)) ||
          matchingJobs.find((j) => j.status === "running") ||
          (matchingJobs.length > 0 ? matchingJobs[matchingJobs.length - 1] : null);

        return (
          <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && setViewingLogs(null)}>
            <div
              className="modal"
              style={{
                width: "min(800px, 95vw)",
                height: "auto",
                maxHeight: "88vh",
                display: "flex",
                flexDirection: "column",
              }}
              role="dialog"
              aria-modal="true"
            >
              {/* Modal Head */}
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
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <h2 style={{ margin: 0, fontSize: 17, fontWeight: 620 }}>
                    {auto.name} {auto.version && <span style={{ fontSize: "0.8em", color: "var(--muted)", fontWeight: 400 }}>v{auto.version}</span>}
                  </h2>
                  <span style={{ fontSize: "0.82em", color: "var(--muted)" }}>Action Logs</span>
                </div>
                <button className="iconic" onClick={() => setViewingLogs(null)}>✕</button>
              </div>

              {/* Modal Body */}
              <div style={{ padding: "var(--s-4) var(--s-5)", overflowY: "auto", flex: 1, minHeight: 0, display: "flex", flexDirection: "column", gap: "var(--s-3)" }}>
                {latestJob ? (
                  <>
                    {/* Run Selector if multiple runs exist */}
                    {matchingJobs.length > 1 && (
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 2 }}>
                        <span className="muted" style={{ fontSize: "0.82em" }}>Run History ({matchingJobs.length} sessions):</span>
                        <select
                          value={latestJob.id}
                          onChange={(e) => setSelectedJobId(e.target.value)}
                          style={{ fontSize: "0.8em", padding: "3px 8px", background: "var(--surface)", border: "1px solid var(--border)", borderRadius: 4 }}
                        >
                          {matchingJobs.slice().reverse().map((j) => (
                            <option key={j.id} value={j.id}>
                              {j.id.slice(0, 8)} — {j.status.toUpperCase()} ({new Date(j.started_at_ms).toLocaleTimeString()})
                            </option>
                          ))}
                        </select>
                      </div>
                    )}

                    {/* Job Status Banner */}
                    <div
                      style={{
                        display: "flex",
                        justifyContent: "space-between",
                        alignItems: "center",
                        flexWrap: "wrap",
                        gap: 10,
                        padding: "10px 14px",
                        borderRadius: "var(--r-1)",
                        background: "var(--surface-sunken)",
                        border: "1px solid var(--border)",
                      }}
                    >
                      <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                        <strong style={{ fontSize: "1.05em" }}>{latestJob.automation_name}</strong>
                        <span
                          style={{
                            fontSize: "0.8em",
                            padding: "2px 8px",
                            borderRadius: 4,
                            fontWeight: 650,
                            letterSpacing: "0.04em",
                            background:
                              latestJob.status === "running"
                                ? "var(--green, #22c55e)"
                                : latestJob.status === "finished"
                                ? "var(--accent, #8b5cf6)"
                                : latestJob.status === "failed"
                                ? "var(--danger, #ef4444)"
                                : "var(--muted)",
                            color: "#fff",
                          }}
                        >
                          {latestJob.status.toUpperCase()}
                        </span>
                        <span className="muted" style={{ fontSize: "0.85em" }}>
                          Profiles: {latestJob.profile_ids.length > 0 ? latestJob.profile_ids.join(", ") : "none"}
                        </span>
                        {latestJob.id && (
                          <span className="muted" style={{ fontSize: "0.78em" }}>
                            (job: {latestJob.id.slice(0, 8)})
                          </span>
                        )}
                      </div>

                      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                        {latestJob.status === "running" && (
                          <button
                            className="danger"
                            style={{ fontSize: "0.85em", padding: "4px 12px" }}
                            onClick={() => handleStop(latestJob.id)}
                          >
                            Stop
                          </button>
                        )}
                        <button
                          className="secondary"
                          style={{ fontSize: "0.85em", padding: "4px 10px" }}
                          onClick={() => {
                            navigator.clipboard.writeText(latestJob.logs.join("\n"));
                            setCopySuccess(true);
                            setTimeout(() => setCopySuccess(false), 2000);
                          }}
                        >
                          {copySuccess ? "✓ Copied" : "Copy Logs"}
                        </button>
                      </div>
                    </div>

                    {/* Terminal Log Console */}
                    <div
                      ref={logContainerRef}
                      style={{
                        flex: 1,
                        minHeight: 280,
                        maxHeight: 450,
                        background: "#09090b",
                        border: "1px solid rgba(255, 255, 255, 0.1)",
                        borderRadius: "var(--r-1)",
                        padding: "12px 14px",
                        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
                        fontSize: "0.82em",
                        lineHeight: 1.5,
                        color: "#d4d4d8",
                        overflowY: "auto",
                        whiteSpace: "pre-wrap",
                        wordBreak: "break-all",
                      }}
                    >
                      {latestJob.logs.length === 0 ? (
                        <div style={{ color: "#71717a" }}>Job started. Waiting for output...</div>
                      ) : (
                        latestJob.logs.map((line, idx) => {
                          const isErr = line.startsWith("[err]") || line.includes("FAILED") || line.includes("error:");
                          const isSuccess = line.includes("Warm-up complete") || line.includes("cleanly") || line.includes("Connected to");
                          const isAction = line.includes("Navigating to") || line.includes("Exploring") || line.includes("Reading");
                          const color = isErr ? "#f87171" : isSuccess ? "#4ade80" : isAction ? "#60a5fa" : "#e4e4e7";
                          return (
                            <div key={idx} style={{ color }}>
                              {line}
                            </div>
                          );
                        })
                      )}
                    </div>
                  </>
                ) : (
                  <div
                    style={{
                      textAlign: "center",
                      padding: "var(--s-6) var(--s-4)",
                      background: "var(--surface-sunken)",
                      borderRadius: "var(--r-2)",
                      border: "1px solid var(--border)",
                    }}
                  >
                    <p style={{ margin: "0 0 12px", fontSize: "1em", color: "var(--muted)" }}>
                      No action logs recorded yet for <strong>{auto.name}</strong>.
                    </p>
                    <p style={{ margin: "0 0 16px", fontSize: "0.85em", color: "var(--text-faint)" }}>
                      Run this automation on any profile to stream real-time logs here.
                    </p>
                    <button
                      className="primary"
                      onClick={() => {
                        setViewingLogs(null);
                        setRunningTarget(auto);
                      }}
                    >
                      Run {auto.name}…
                    </button>
                  </div>
                )}
              </div>

              {/* Modal Foot */}
              <div
                className="modalFoot"
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  padding: "var(--s-3) var(--s-5)",
                  borderTop: "1px solid var(--line)",
                }}
              >
                <span className="muted" style={{ fontSize: "0.8em" }}>
                  {latestJob?.status === "running" ? "● Streaming live output..." : "Session ended"}
                </span>
                <div style={{ display: "flex", gap: 8 }}>
                  {latestJob && latestJob.status !== "running" && (
                    <button
                      className="primary"
                      style={{ fontSize: "0.85em" }}
                      onClick={() => {
                        setViewingLogs(null);
                        setRunningTarget(auto);
                      }}
                    >
                      Run Again…
                    </button>
                  )}
                  <button className="secondary" style={{ fontSize: "0.85em" }} onClick={() => setViewingLogs(null)}>
                    Close
                  </button>
                </div>
              </div>
            </div>
          </div>
        );
      })()}
    </div>
  );
}
