import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IdleUpdatePayload } from "../types";

function formatIdleTime(secs: number): string {
  if (secs < 3600) {
    return `${Math.max(1, Math.floor(secs / 60))} min`;
  } else if (secs < 86400) {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    return m > 0 ? `${h} h ${m} min` : `${h} h`;
  } else {
    const d = Math.floor(secs / 86400);
    const h = Math.floor((secs % 86400) / 3600);
    return h > 0 ? `${d} days ${h} h` : `${d} days`;
  }
}

export default function IdlePage() {
  const params = new URLSearchParams(window.location.search);
  const initialIdleSecs = Math.max(0, parseInt(params.get("idle_secs") ?? "0") || 0);

  const [idleSecs, setIdleSecs] = useState(initialIdleSecs);
  const [resuming, setResuming] = useState(false);
  const [stopping, setStopping] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    listen<IdleUpdatePayload>("timer-idle-update", (event) => {
      setIdleSecs(event.payload.idle_secs);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const handleResume = async () => {
    if (resuming || stopping) return;
    setResuming(true);
    try {
      await invoke("cmd_resume_after_idle");
    } catch (e) {
      console.error(e);
      setResuming(false);
    }
  };

  const handleStop = async () => {
    if (resuming || stopping) return;
    setStopping(true);
    try {
      await invoke("cmd_stop_after_idle");
    } catch (e) {
      console.error(e);
      setStopping(false);
    }
  };

  const handleMouseDown = async (e: React.MouseEvent) => {
    if (e.button === 0 && !(e.target as HTMLElement).closest("button")) {
      await getCurrentWindow().startDragging();
    }
  };

  return (
    <div
      onMouseDown={handleMouseDown}
      className="flex flex-col h-screen w-screen items-center justify-center bg-[#0f0f0f] text-white select-none px-6"
      style={{ cursor: "grab" }}
    >
      <div className="w-full max-w-[280px] flex flex-col items-center gap-6">
        <div className="h-12 w-12 rounded-2xl bg-yellow-500/10 border border-yellow-500/20 flex items-center justify-center">
          <span className="text-2xl">⏸</span>
        </div>

        {/* UX-05: "Your timer has been paused." subtitle */}
        <div className="text-center">
          <h2 className="text-white font-semibold text-base">You've been idle</h2>
          <p className="text-[#f59e0b] text-xs font-medium mt-0.5">Your timer has been paused.</p>
          <p className="text-[#555] text-sm mt-1.5">
            No activity for {formatIdleTime(idleSecs)}.<br />
            What would you like to do?
          </p>
        </div>

        {/* UX-06: New button labels and explanatory subtexts */}
        <div className="flex flex-col gap-2 w-full">
          <button
            autoFocus
            onClick={handleResume}
            disabled={resuming || stopping}
            style={{ cursor: "default" }}
            className="w-full rounded-lg bg-[#6ee7b7] py-2.5 px-3 text-[#0f0f0f] transition hover:bg-[#a7f3d0] disabled:opacity-50 disabled:cursor-not-allowed text-left"
          >
            <p className="text-sm font-semibold leading-tight">
              {resuming ? "…" : "▶ I was working — Resume"}
            </p>
            <p className="text-[10px] opacity-60 mt-0.5">Continue tracking where you left off</p>
          </button>
          <button
            onClick={handleStop}
            disabled={resuming || stopping}
            style={{ cursor: "default" }}
            className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] py-2.5 px-3 text-white transition hover:border-red-500 hover:text-red-400 disabled:opacity-50 disabled:cursor-not-allowed text-left"
          >
            <p className="text-sm font-semibold leading-tight">
              {stopping ? "…" : "✕ Discard idle time — Stop"}
            </p>
            <p className="text-[10px] opacity-60 mt-0.5">Idle time will not be counted</p>
          </button>
        </div>
      </div>
    </div>
  );
}
