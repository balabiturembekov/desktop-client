import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { User, Project, TimerPayload } from "../types";

interface Props {
  user: User;
  onLogout: () => void;
  showUpdateBanner?: boolean; // UX-10: padding when bottom banner is visible
}

function formatTime(secs: number): string {
  const h = Math.floor(secs / 3600).toString().padStart(2, "0");
  const m = Math.floor((secs % 3600) / 60).toString().padStart(2, "0");
  const s = (secs % 60).toString().padStart(2, "0");
  return `${h}:${m}:${s}`;
}

/** Formats an ISO-8601 timestamp as "DD.MM.YYYY HH:mm" (Hubstaff style). */
function formatSyncTime(iso: string): string {
  const d = new Date(iso);
  const dd = d.getDate().toString().padStart(2, "0");
  const mm = (d.getMonth() + 1).toString().padStart(2, "0");
  const yyyy = d.getFullYear();
  const hh = d.getHours().toString().padStart(2, "0");
  const min = d.getMinutes().toString().padStart(2, "0");
  return `${dd}.${mm}.${yyyy} ${hh}:${min}`;
}

export default function TrackerPage({ user, onLogout, showUpdateBanner }: Props) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedProject, setSelectedProject] = useState<Project | null>(null);
  const [totalSecs, setTotalSecs] = useState(0);
  const [isRunning, setIsRunning] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [projectsError, setProjectsError] = useState(false);
  const [starting, setStarting] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [lastSyncAt, setLastSyncAt] = useState<string | null>(null);
  // null = unknown (waiting for first connectivity-changed from sync_actor)
  // sync_actor emits the initial state within ~5 s of startup (M-01 audit #3).
  const [isOnline, setIsOnline] = useState<boolean | null>(null);
  // UX-08: sign-out confirmation dialog
  const [showLogoutConfirm, setShowLogoutConfirm] = useState(false);

  // Refs for tray throttle — not state, so they don't trigger re-renders
  const lastTrayUpdate = useRef<number>(0);
  const lastTrayIsRunning = useRef<boolean | null>(null);
  const settingsRef = useRef<HTMLDivElement>(null);
  // BUG-F07: stable ref so the timer-tick closure can read up-to-date projects
  const projectsRef = useRef<Project[]>([]);

  // BUG-F15: wrap in useCallback so the useEffect dependency is stable
  // BUG-F03: return a cancel function to prevent setState after unmount
  const loadProjects = useCallback((): (() => void) => {
    let cancelled = false;
    setProjectsError(false);
    Promise.all([
      invoke<Project[]>("cmd_get_projects"),
      invoke<number>("cmd_get_today_secs"),
    ])
      .then(([p, secs]) => {
        if (cancelled) return;
        projectsRef.current = p;
        setProjects(p);
        // BUG-F05: clear selection when project list is empty
        if (p.length > 0) setSelectedProject(p[0]);
        else setSelectedProject(null);
        setTotalSecs(secs);
      })
      .catch(() => { if (!cancelled) setProjectsError(true); })
      .finally(() => { if (!cancelled) setInitialized(true); });
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    return loadProjects();
  }, [loadProjects]);

  // BUG-F01: cancelled flag prevents calling unlisten on an already-cleaned-up listener
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let unlistenDayRollover: (() => void) | undefined;

    listen<TimerPayload>("timer-tick", (e) => {
      setTotalSecs(e.payload.total_secs);
      setIsRunning(e.payload.is_running);

      // BUG-F07: sync selected project when timer carries a project_id
      // (e.g. started from tray menu while app was closed)
      if (e.payload.project_id) {
        const pid = e.payload.project_id;
        const match = projectsRef.current.find((p) => p.remote_id === pid);
        if (match) setSelectedProject((cur) => cur?.remote_id === pid ? cur : match);
      }

      // Throttle tray updates: call on is_running change OR every 10s.
      const now = Date.now();
      const isRunningChanged = e.payload.is_running !== lastTrayIsRunning.current;
      if (isRunningChanged || now - lastTrayUpdate.current >= 10_000) {
        invoke("cmd_update_tray_status", {
          isRunning: e.payload.is_running,
          timeStr: formatTime(e.payload.total_secs),
        }).catch(() => {});
        lastTrayUpdate.current = now;
        lastTrayIsRunning.current = e.payload.is_running;
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    listen<void>("day-rollover", () => {
      setTotalSecs(0);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenDayRollover = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
      unlistenDayRollover?.();
    };
  }, []);

  // BUG-F01: cancelled flag for sync/connectivity listeners
  useEffect(() => {
    let cancelled = false;
    let unlistenSync: (() => void) | undefined;
    let unlistenConn: (() => void) | undefined;

    listen<string>("sync-completed", (e) => {
      setLastSyncAt(e.payload);
      // BUG-F17: do NOT setIsOnline(true) here — connectivity-changed is the source of truth
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenSync = fn;
    });

    listen<boolean>("connectivity-changed", (e) => {
      setIsOnline(e.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenConn = fn;
    });

    return () => {
      cancelled = true;
      unlistenSync?.();
      unlistenConn?.();
    };
  }, []);

  // Read autostart status once on mount
  useEffect(() => {
    invoke<boolean>("cmd_autostart_is_enabled")
      .then(setLaunchAtLogin)
      .catch(() => {});
  }, []);

  // Close settings dropdown on click-outside
  useEffect(() => {
    if (!showSettings) return;
    const handler = (e: MouseEvent) => {
      if (settingsRef.current && !settingsRef.current.contains(e.target as Node)) {
        setShowSettings(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [showSettings]);

  // UX-21: Keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Escape: close logout confirm dialog
      if (e.key === "Escape") {
        setShowLogoutConfirm(false);
        setShowSettings(false);
        return;
      }

      const meta = e.metaKey || e.ctrlKey;

      // ⌘R / Ctrl+R: retry loading projects
      if (meta && e.key.toLowerCase() === "r" && projectsError) {
        e.preventDefault();
        setInitialized(false);
        loadProjects();
        return;
      }

      // Space or ⌘Enter / Ctrl+Enter: toggle timer
      if (e.key === " " || (meta && e.key === "Enter")) {
        const tag = (e.target as HTMLElement).tagName;
        if (tag === "BUTTON" || tag === "SELECT" || tag === "INPUT" || tag === "TEXTAREA") return;
        e.preventDefault();
        if (isRunning) {
          invoke("stop_worker_timer").catch(console.error);
        } else if (selectedProject && !starting) {
          setStarting(true);
          invoke("start_worker_timer", { projectId: selectedProject.remote_id })
            .catch(console.error)
            .finally(() => setStarting(false));
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [isRunning, selectedProject, starting, projectsError, loadProjects]);

  const handleToggleLaunchAtLogin = async () => {
    const next = !launchAtLogin;
    try {
      await invoke(next ? "cmd_autostart_enable" : "cmd_autostart_disable");
      setLaunchAtLogin(next);
    } catch (e) {
      console.error(e);
    }
  };

  const handleStart = async () => {
    if (!selectedProject || starting) return;
    setStarting(true);
    try {
      await invoke("start_worker_timer", { projectId: selectedProject.remote_id });
    } catch (e) {
      console.error(e);
    } finally {
      setStarting(false);
    }
  };

  const handleRetry = () => {
    setInitialized(false);
    loadProjects();
  };

  const handleStop = () => invoke("stop_worker_timer").catch(console.error);

  // BUG-F02/F08: call cmd_logout (which stops timer unconditionally + clears DB)
  const handleLogout = async () => {
    await invoke("cmd_logout").catch(console.error);
    onLogout();
  };

  const handleReset = async () => {
    await invoke("reset_worker_timer").catch(console.error);
  };

  if (!initialized) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-[#0f0f0f]">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-[#6ee7b7] border-t-transparent" />
      </div>
    );
  }

  return (
    // UX-08 overlay needs relative parent; UX-10 pb-12 when update banner visible
    <div className={`flex flex-col h-screen w-screen bg-[#0f0f0f] text-white select-none relative${showUpdateBanner ? " pb-12" : ""}`}>
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-[#1e1e1e]">
        <div className="flex items-center gap-2">
          <div className="h-5 w-5 rounded bg-[#6ee7b7] flex items-center justify-center">
            <svg width="10" height="10" viewBox="0 0 16 16" fill="none">
              <circle cx="8" cy="8" r="3" fill="#0f0f0f"/>
              <path d="M8 2v2M8 12v2M2 8h2M12 8h2" stroke="#0f0f0f" strokeWidth="1.5" strokeLinecap="round"/>
            </svg>
          </div>
          <span className="text-xs font-semibold text-[#888]">Hubnity</span>
        </div>
        <div className="flex items-center gap-3">
          <span className="text-xs text-[#555]">{user.name}</span>
          {/* Settings dropdown */}
          <div className="relative" ref={settingsRef}>
            {/* BUG-F11: aria-expanded + aria-label */}
            <button
              onClick={() => setShowSettings((s) => !s)}
              aria-expanded={showSettings}
              aria-label="Settings"
              className={`text-sm transition leading-none ${showSettings ? "text-white" : "text-[#555] hover:text-white"}`}
              title="Settings"
            >
              ⚙
            </button>
            {showSettings && (
              <div className="absolute right-0 top-full mt-1.5 w-44 bg-[#1a1a1a] border border-[#2a2a2a] rounded-lg py-1 z-10 shadow-xl">
                <button
                  onClick={handleToggleLaunchAtLogin}
                  className="w-full flex items-center gap-2.5 px-3 py-2 text-xs hover:bg-[#222] transition text-left"
                >
                  <span className={launchAtLogin ? "text-[#6ee7b7]" : "text-[#444]"}>
                    {launchAtLogin ? "☑" : "☐"}
                  </span>
                  <span className="text-[#aaa]">Launch at login</span>
                </button>
              </div>
            )}
          </div>
          {/* UX-08: "Sign out" text button that opens confirm dialog */}
          <button
            onClick={() => setShowLogoutConfirm(true)}
            aria-label="Sign out"
            className="text-[#555] hover:text-red-400 transition text-xs"
          >
            Sign out
          </button>
        </div>
      </div>

      {/* Timer */}
      <div className="flex flex-col items-center justify-center flex-1 gap-5 px-6">

        {/* Today label */}
        <span className="text-xs text-[#444] uppercase tracking-widest">Today</span>

        <div className="relative">
          <div className={`text-5xl font-mono font-bold tracking-tight transition-colors ${isRunning ? "text-[#6ee7b7]" : "text-white"}`}>
            {formatTime(totalSecs)}
          </div>
          {isRunning && (
            <span className="absolute -top-1 -right-3 h-2 w-2 rounded-full bg-[#6ee7b7] animate-pulse" />
          )}
        </div>

        {/* Project selector */}
        <div className="w-full">
          <div className="flex items-baseline justify-between mb-1.5">
            <label className="text-xs text-[#555] uppercase tracking-wider">Project</label>
            {user.org_name && (
              <span className="text-[10px] text-[#555] truncate max-w-[160px]">{user.org_name}</span>
            )}
          </div>
          {projectsError ? (
            <div className="rounded-lg bg-[#1a1a1a] border border-red-900 px-3 py-2.5 flex items-center justify-between gap-3">
              <span className="text-xs text-red-400">
                Couldn't load projects — check your connection
              </span>
              <button
                onClick={handleRetry}
                className="shrink-0 text-xs text-[#6ee7b7] hover:text-[#a7f3d0] transition font-semibold"
              >
                Retry
              </button>
            </div>
          ) : projects.length === 0 ? (
            /* UX-04: No projects assigned state */
            <div className="rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-3 py-3">
              <p className="text-xs text-[#888]">No projects assigned.</p>
              <p className="text-xs text-[#555] mt-0.5">Contact your manager to get access.</p>
              <button
                onClick={handleRetry}
                className="mt-2 text-xs text-[#6ee7b7] hover:text-[#a7f3d0] transition font-semibold"
              >
                Retry
              </button>
            </div>
          ) : (
            <select
              value={selectedProject?.remote_id ?? ""}
              onChange={(e) => {
                const p = projects.find((p) => p.remote_id === e.target.value);
                setSelectedProject(p ?? null);
              }}
              disabled={isRunning}
              className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-3 py-2 text-sm text-white outline-none transition focus:border-[#6ee7b7] disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {projects.map((p) => (
                <option key={p.remote_id} value={p.remote_id}>{p.name}</option>
              ))}
            </select>
          )}
        </div>

        {/* Controls */}
        <div className="flex gap-2 w-full">
          {!isRunning ? (
            <button
              onClick={handleStart}
              disabled={!selectedProject || starting}
              className="flex-1 rounded-lg bg-[#6ee7b7] py-2.5 text-sm font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0] disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {starting ? "..." : "▶ Start"}
            </button>
          ) : (
            <button
              onClick={handleStop}
              className="flex-1 rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] py-2.5 text-sm font-semibold text-white transition hover:border-red-500 hover:text-red-400"
            >
              ■ Stop
            </button>
          )}
          {/* UX-17: title="Reset timer" */}
          <button
            onClick={handleReset}
            disabled={isRunning || totalSecs === 0}
            aria-label="Reset timer"
            title="Reset timer"
            className="rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-4 py-2.5 text-sm text-[#555] transition hover:text-white hover:border-[#444] disabled:opacity-30 disabled:cursor-not-allowed"
          >
            ↺
          </button>
        </div>
      </div>

      {/* Footer */}
      <div className="px-4 py-2 border-t border-[#1e1e1e] flex items-center justify-between">
        {isOnline === null ? (
          /* UX-20: Spinning indicator next to "Connecting..." */
          <div className="flex items-center gap-1.5">
            <div className="h-2.5 w-2.5 animate-spin rounded-full border-2 border-[#444] border-t-transparent shrink-0" />
            <span className="text-xs text-[#444]">Connecting…</span>
          </div>
        ) : isOnline ? (
          <div className="flex items-center gap-1.5">
            <svg width="10" height="10" viewBox="0 0 10 10" className="shrink-0">
              <circle cx="5" cy="5" r="4.5" fill="none" stroke="#6ee7b7" strokeWidth="1"/>
              <path d="M3 5l1.5 1.5L7 3.5" stroke="#6ee7b7" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round"/>
            </svg>
            <span className="text-xs text-[#555]">
              {lastSyncAt ? `Last sync: ${formatSyncTime(lastSyncAt)}` : "Syncing…"}
            </span>
          </div>
        ) : (
          <div className="flex items-center gap-1.5">
            <div className="h-1.5 w-1.5 rounded-full bg-red-500" />
            <span className="text-xs text-red-500">Offline</span>
          </div>
        )}
        {user.org_name && (
          <span className="text-xs text-[#444] truncate max-w-[180px]">{user.org_name}</span>
        )}
      </div>

      {/* UX-08: Sign out confirmation overlay */}
      {showLogoutConfirm && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/70">
          <div className="w-[260px] rounded-xl bg-[#141414] border border-[#242424] shadow-2xl p-5 flex flex-col gap-4">
            <div>
              <p className="text-sm font-semibold text-white">Sign out?</p>
              <p className="text-xs text-[#555] mt-1">Your local data will be cleared.</p>
            </div>
            <div className="flex flex-col gap-2">
              <button
                autoFocus
                onClick={handleLogout}
                className="w-full rounded-lg bg-red-500 py-2 text-xs font-semibold text-white transition hover:bg-red-400"
              >
                Sign out
              </button>
              <button
                onClick={() => setShowLogoutConfirm(false)}
                className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] py-2 text-xs text-[#888] transition hover:border-[#444] hover:text-white"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
