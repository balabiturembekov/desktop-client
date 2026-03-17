import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { User, Project, TimerPayload } from "../types";

interface Props {
  user: User;
  onLogout: () => void;
}

function formatTime(secs: number): string {
  const h = Math.floor(secs / 3600).toString().padStart(2, "0");
  const m = Math.floor((secs % 3600) / 60).toString().padStart(2, "0");
  const s = (secs % 60).toString().padStart(2, "0");
  return `${h}:${m}:${s}`;
}

export default function TrackerPage({ user, onLogout }: Props) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedProject, setSelectedProject] = useState<Project | null>(null);
  const [totalSecs, setTotalSecs] = useState(0);
  const [isRunning, setIsRunning] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [starting, setStarting] = useState(false);

  // Refs for tray throttle — not state, so they don't trigger re-renders
  const lastTrayUpdate = useRef<number>(0);
  const lastTrayIsRunning = useRef<boolean | null>(null);

  useEffect(() => {
    // Загружаем проекты и today secs параллельно
    Promise.all([
      invoke<Project[]>("cmd_get_projects"),
      invoke<number>("cmd_get_today_secs"),
    ]).then(([p, secs]) => {
      setProjects(p);
      if (p.length > 0) setSelectedProject(p[0]);
      setTotalSecs(secs);
      setInitialized(true);
    });
  }, []);

useEffect(() => {
    let unlisten: (() => void) | undefined;
    let unlistenDayRollover: (() => void) | undefined;

    listen<TimerPayload>("timer-tick", (e) => {
      setTotalSecs(e.payload.total_secs);
      setIsRunning(e.payload.is_running);

      // Throttle tray updates: call on is_running change OR every 10s.
      // Calling set_text/set_tooltip every second causes a race condition
      // in tao on macOS that crashes the process.
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
    }).then((fn) => (unlisten = fn));

    listen<void>("day-rollover", () => {
      setTotalSecs(0);
    }).then((fn) => (unlistenDayRollover = fn));

    return () => {
      unlisten?.();
      unlistenDayRollover?.();
    };
  }, []);
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
  const handleStop = () => invoke("stop_worker_timer").catch(console.error);
  const handleReset = async () => {
    if (isRunning) {
      if (!window.confirm("Timer is running. Reset will discard the current session. Continue?")) return;
    }
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
    <div className="flex flex-col h-screen w-screen bg-[#0f0f0f] text-white select-none">
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
          <button onClick={onLogout} className="text-[#555] hover:text-red-400 transition text-xs">⏏</button>
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
          <label className="block text-xs text-[#555] mb-1.5 uppercase tracking-wider">Project</label>
          <select
            value={selectedProject?.remote_id ?? ""}
            onChange={(e) => {
              const p = projects.find((p) => p.remote_id === e.target.value);
              setSelectedProject(p ?? null);
            }}
            disabled={isRunning}
            className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-3 py-2 text-sm text-white outline-none transition focus:border-[#6ee7b7] disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {projects.length === 0 && <option value="">No projects</option>}
            {projects.map((p) => (
              <option key={p.remote_id} value={p.remote_id}>{p.name}</option>
            ))}
          </select>
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
          <button
            onClick={handleReset}
            disabled={isRunning || totalSecs === 0}
            className="rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-4 py-2.5 text-sm text-[#555] transition hover:text-white hover:border-[#444] disabled:opacity-30 disabled:cursor-not-allowed"
          >
            ↺
          </button>
        </div>
      </div>

      {/* Footer */}
      <div className="px-4 py-2 border-t border-[#1e1e1e] flex items-center justify-between">
        <div className="flex items-center gap-1.5">
          <div className={`h-1.5 w-1.5 rounded-full ${isRunning ? "bg-[#6ee7b7]" : "bg-[#333]"}`} />
          <span className="text-xs text-[#555]">{isRunning ? "Tracking" : "Idle"}</span>
        </div>
        {selectedProject && (
          <span className="text-xs text-[#444] truncate max-w-[150px]">{selectedProject.name}</span>
        )}
      </div>
    </div>
  );
}
