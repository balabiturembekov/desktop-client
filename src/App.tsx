import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import LoginPage from "./pages/LoginPage";
import TrackerPage from "./pages/TrackerPage";
import { User } from "./types";

interface ClosePayload {
  unsynced_count: number;
  timer_running: boolean;
}

interface UpdateProgress {
  downloaded: number;
  total: number;
}

export default function App() {
  const [user, setUser] = useState<User | null>(null);
  const [checking, setChecking] = useState(true);
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(null);
  // BUG-F10: surface update errors to the user
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [closeModal, setCloseModal] = useState<ClosePayload | null>(null);
  const [closing, setClosing] = useState(false);
  const [permissionsRequired, setPermissionsRequired] = useState(false);
  const [accessibilityNeedsRestart, setAccessibilityNeedsRestart] = useState(false);

  useEffect(() => {
    invoke<User | null>("cmd_get_current_user")
      .then((u) => setUser(u))
      .catch(() => setUser(null))
      .finally(() => setChecking(false));
  }, []);

  // BUG-F01: cancelled flag prevents calling unlisten on an already-cleaned-up listener
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<string>("update-available", (e) => {
      setUpdateVersion(e.payload);
      setUpdateDismissed(false);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<UpdateProgress>("update-progress", (e) => {
      setUpdateProgress(e.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<ClosePayload>("close-requested-with-unsynced", (e) => {
      setCloseModal(e.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlistenRequired: (() => void) | undefined;
    let unlistenGranted: (() => void) | undefined;
    let unlistenNeedsRestart: (() => void) | undefined;
    listen("permissions-required", () => {
      setPermissionsRequired(true);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenRequired = fn;
    });
    listen("accessibility-granted", () => {
      setPermissionsRequired(false);
      setAccessibilityNeedsRestart(false);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenGranted = fn;
    });
    listen("accessibility-needs-restart", () => {
      setAccessibilityNeedsRestart(true);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenNeedsRestart = fn;
    });
    return () => {
      cancelled = true;
      unlistenRequired?.();
      unlistenGranted?.();
      unlistenNeedsRestart?.();
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen("screenshot-taken", async () => {
      let permissionGranted = await isPermissionGranted();
      if (!permissionGranted) {
        const permission = await requestPermission();
        permissionGranted = permission === "granted";
      }
      if (permissionGranted) {
        sendNotification({ title: "Hubnity", body: "Screenshot taken" });
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  // BUG-F10: catch and display update errors
  const handleUpdate = async () => {
    setUpdating(true);
    setUpdateProgress(null);
    setUpdateError(null);
    try {
      await invoke("cmd_download_and_install");
    } catch (e) {
      setUpdateError(String(e));
    } finally {
      setUpdating(false);
    }
  };

  // BUG-F06: renamed from "Wait for sync" to "Go back"
  const handleGoBack = () => setCloseModal(null);

  const handleStopAndClose = async () => {
    setClosing(true);
    await invoke("cmd_stop_and_quit").catch(console.error);
  };

  const handleForceClose = async () => {
    setClosing(true);
    await invoke("cmd_force_quit").catch(console.error);
  };

  if (checking) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-[#0f0f0f]">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-[#6ee7b7] border-t-transparent" />
      </div>
    );
  }

  const progressPercent =
    updateProgress && updateProgress.total > 0
      ? Math.round((updateProgress.downloaded / updateProgress.total) * 100)
      : null;

  return (
    <>
      {/* Accessibility needs-restart banner */}
      {accessibilityNeedsRestart && (
        <div className="fixed top-0 left-0 right-0 z-50 bg-[#1e1208] border-b border-[#4a2e0a] px-4 py-2.5 flex items-center gap-3">
          <svg className="shrink-0" width="14" height="14" viewBox="0 0 16 16" fill="none">
            <path d="M8 2L14 13H2L8 2Z" stroke="#f59e0b" strokeWidth="1.5" strokeLinejoin="round"/>
            <path d="M8 7v3M8 11.5v.5" stroke="#f59e0b" strokeWidth="1.5" strokeLinecap="round"/>
          </svg>
          <p className="flex-1 text-xs text-[#f59e0b]">
            Permission granted — quit and reopen Hubnity to apply
          </p>
          <button
            onClick={() => invoke("cmd_force_quit").catch(() => {})}
            className="shrink-0 rounded-md bg-[#f59e0b] px-3 py-1 text-[11px] font-semibold text-[#0f0f0f] transition hover:bg-[#fbbf24]"
          >
            Quit & Reopen
          </button>
        </div>
      )}

      {/* Update banner — bottom, non-intrusive */}
      {updateVersion && !updateDismissed && (
        <div className="fixed bottom-0 left-0 right-0 z-40 bg-[#141414] border-t border-[#242424] px-4 py-2.5 flex items-center gap-3">
          {updating ? (
            <>
              <div className="h-3 w-3 shrink-0 animate-spin rounded-full border-2 border-[#6ee7b7] border-t-transparent" />
              <div className="flex-1 min-w-0">
                <div className="h-1 rounded-full bg-[#242424] overflow-hidden">
                  <div
                    className="h-full rounded-full bg-[#6ee7b7] transition-all duration-300"
                    style={{ width: progressPercent != null ? `${progressPercent}%` : "100%" }}
                  />
                </div>
                <p className="text-[10px] text-[#555] mt-1">
                  {progressPercent != null
                    ? `Downloading… ${progressPercent}%`
                    : "Downloading…"}
                </p>
              </div>
            </>
          ) : updateError ? (
            <>
              <div className="flex-1 min-w-0">
                <p className="text-xs text-red-400 truncate">Update failed — {updateError}</p>
              </div>
              <button
                onClick={handleUpdate}
                className="shrink-0 rounded-md bg-[#6ee7b7] px-3 py-1 text-[11px] font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0]"
              >
                Retry
              </button>
              <button
                onClick={() => setUpdateDismissed(true)}
                className="shrink-0 text-[#555] hover:text-white transition text-sm leading-none"
                aria-label="Dismiss"
              >
                ×
              </button>
            </>
          ) : (
            <>
              <div className="flex-1 min-w-0">
                <p className="text-xs text-white font-semibold truncate">
                  New version v{updateVersion} available
                </p>
              </div>
              <button
                onClick={handleUpdate}
                className="shrink-0 rounded-md bg-[#6ee7b7] px-3 py-1 text-[11px] font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0]"
              >
                Update now
              </button>
              <button
                onClick={() => setUpdateDismissed(true)}
                className="shrink-0 text-[#555] hover:text-white transition text-sm leading-none"
                aria-label="Dismiss"
              >
                ×
              </button>
            </>
          )}
        </div>
      )}

      {user ? (
        <TrackerPage user={user} onLogout={() => setUser(null)} />
      ) : (
        <LoginPage onLogin={setUser} />
      )}

      {/* Permissions onboarding screen */}
      {permissionsRequired && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-[#0f0f0f]">
          <div className="w-[340px] rounded-xl bg-[#141414] border border-[#242424] shadow-2xl p-6 flex flex-col gap-5">
            {/* Header */}
            <div className="flex flex-col gap-1">
              <div className="flex items-center gap-2">
                <div className="h-5 w-5 rounded bg-[#f59e0b] flex items-center justify-center shrink-0">
                  <svg width="10" height="10" viewBox="0 0 16 16" fill="none">
                    <path d="M8 2L14 13H2L8 2Z" stroke="#0f0f0f" strokeWidth="1.5" strokeLinejoin="round"/>
                    <path d="M8 7v3M8 11.5v.5" stroke="#0f0f0f" strokeWidth="1.5" strokeLinecap="round"/>
                  </svg>
                </div>
                <span className="text-sm font-semibold text-white">Permissions Required</span>
              </div>
              <p className="text-xs text-[#666] ml-7">
                Grant the following permissions, then restart the app.
              </p>
            </div>

            {/* Permissions list */}
            <div className="flex flex-col gap-2.5">
              <div className="rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-3 py-2.5 flex gap-3">
                <div className="text-[#6ee7b7] mt-0.5 shrink-0">
                  <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                    <circle cx="8" cy="5" r="3" stroke="currentColor" strokeWidth="1.5"/>
                    <path d="M3 15a5 5 0 0110 0" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
                  </svg>
                </div>
                <div>
                  <p className="text-xs font-semibold text-white">Accessibility</p>
                  <p className="text-xs text-[#555]">Required to track keyboard and mouse activity</p>
                </div>
              </div>
              <div className="rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-3 py-2.5 flex gap-3">
                <div className="text-[#6ee7b7] mt-0.5 shrink-0">
                  <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                    <rect x="1" y="2" width="14" height="10" rx="1.5" stroke="currentColor" strokeWidth="1.5"/>
                    <path d="M5 14h6M8 12v2" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
                  </svg>
                </div>
                <div>
                  <p className="text-xs font-semibold text-white">Screen Recording</p>
                  <p className="text-xs text-[#555]">Required to capture periodic screenshots</p>
                </div>
              </div>
            </div>

            {/* Restart notice */}
            <p className="text-[11px] text-[#444] text-center leading-relaxed">
              macOS requires an app restart after granting permissions.
            </p>

            {/* Buttons — BUG-F16: autoFocus on primary button for focus management */}
            <div className="flex flex-col gap-2">
              <button
                autoFocus
                onClick={() => invoke("cmd_open_accessibility_settings").catch(console.error)}
                className="w-full rounded-lg bg-[#f59e0b] py-2 text-xs font-semibold text-[#0f0f0f] transition hover:bg-[#fbbf24]"
              >
                Open Settings
              </button>
              <button
                onClick={() => invoke("cmd_force_quit").catch(console.error)}
                className="w-full rounded-lg bg-[#6ee7b7] py-2 text-xs font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0]"
              >
                Quit &amp; Reopen
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Close warning modal */}
      {closeModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70">
          <div className="w-[300px] rounded-xl bg-[#141414] border border-[#242424] shadow-2xl p-5 flex flex-col gap-4">
            {/* Header */}
            <div className="flex items-center gap-2">
              <div className="h-5 w-5 rounded bg-[#6ee7b7] flex items-center justify-center shrink-0">
                <svg width="10" height="10" viewBox="0 0 16 16" fill="none">
                  <circle cx="8" cy="8" r="3" fill="#0f0f0f" />
                  <path d="M8 2v2M8 12v2M2 8h2M12 8h2" stroke="#0f0f0f" strokeWidth="1.5" strokeLinecap="round" />
                </svg>
              </div>
              <span className="text-sm font-semibold text-white">Before you close</span>
            </div>

            {/* Warnings */}
            <div className="flex flex-col gap-2.5">
              {closeModal.timer_running && (
                <div className="rounded-lg bg-[#1e1a0f] border border-[#3d3010] px-3 py-2.5">
                  <p className="text-xs font-semibold text-[#f59e0b] mb-0.5">Timer is still running</p>
                  <p className="text-xs text-[#8a7a50]">
                    The current session will be saved when you stop.
                  </p>
                </div>
              )}
              {closeModal.unsynced_count > 0 && (
                <div className="rounded-lg bg-[#0f1a1e] border border-[#103040] px-3 py-2.5">
                  <p className="text-xs font-semibold text-[#6ee7b7] mb-0.5">
                    {closeModal.unsynced_count} unsynced{" "}
                    {closeModal.unsynced_count === 1 ? "entry" : "entries"}
                  </p>
                  <p className="text-xs text-[#4a7a70]">
                    They will sync automatically next time you open the app.
                  </p>
                </div>
              )}
            </div>

            {/* Buttons — BUG-F16: autoFocus on primary action */}
            <div className="flex flex-col gap-2">
              {closeModal.timer_running && (
                <button
                  autoFocus
                  onClick={handleStopAndClose}
                  disabled={closing}
                  className="w-full rounded-lg bg-[#6ee7b7] py-2 text-xs font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0] disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {closing ? (
                    <span className="flex items-center justify-center gap-1.5">
                      <span className="h-3 w-3 animate-spin rounded-full border-2 border-[#0f0f0f] border-t-transparent" />
                      Stopping...
                    </span>
                  ) : (
                    "Stop timer & close"
                  )}
                </button>
              )}
              {/* BUG-F06: renamed from "Wait for sync" to "Go back" */}
              {!closeModal.timer_running && closeModal.unsynced_count > 0 && (
                <button
                  autoFocus
                  onClick={handleGoBack}
                  disabled={closing}
                  className="w-full rounded-lg bg-[#6ee7b7] py-2 text-xs font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0] disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  Go back
                </button>
              )}
              <button
                onClick={handleForceClose}
                disabled={closing}
                className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] py-2 text-xs text-[#888] transition hover:border-red-800 hover:text-red-400 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Close anyway
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
