import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export default function IdlePage() {
  const params = new URLSearchParams(window.location.search);
  // BUG-F09: validate idleMins — parseInt may return NaN, clamp to minimum 1
  const idleMins = String(Math.max(1, parseInt(params.get("idle_mins") ?? "5") || 5));

  const [resuming, setResuming] = useState(false);
  const [stopping, setStopping] = useState(false);

  // BUG-F04: try-catch + loading state on both buttons
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

        <div className="text-center">
          <h2 className="text-white font-semibold text-base">You've been idle</h2>
          <p className="text-[#555] text-sm mt-1">
            No activity for {idleMins} min.<br />
            What would you like to do?
          </p>
        </div>

        <div className="flex flex-col gap-2 w-full">
          <button
            onClick={handleResume}
            disabled={resuming || stopping}
            style={{ cursor: "default" }}
            className="w-full rounded-lg bg-[#6ee7b7] py-2.5 text-sm font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0] disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {resuming ? "…" : "▶ Resume Timer"}
          </button>
          <button
            onClick={handleStop}
            disabled={resuming || stopping}
            style={{ cursor: "default" }}
            className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] py-2.5 text-sm font-semibold text-white transition hover:border-red-500 hover:text-red-400 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {stopping ? "…" : "■ Stop Timer"}
          </button>
        </div>
      </div>
    </div>
  );
}
