import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export default function IdlePage() {
  const params = new URLSearchParams(window.location.search);
  const idleMins = params.get("idle_mins") ?? "5";

  const handleResume = async () => {
    await invoke("cmd_resume_after_idle");
  };

  const handleStop = async () => {
    await invoke("cmd_stop_after_idle");
  };

  const handleMouseDown = async (e: React.MouseEvent) => {
    // Только левая кнопка и не на кнопках
    if (e.button === 0 && !(e.target as HTMLElement).closest('button')) {
      await getCurrentWindow().startDragging();
    }
  };

  return (
    <div
      onMouseDown={handleMouseDown}
      className="flex flex-col h-screen w-screen items-center justify-center bg-[#0f0f0f] text-white select-none px-6"
      style={{ cursor: 'grab' }}
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
            style={{ cursor: 'default' }}
            className="w-full rounded-lg bg-[#6ee7b7] py-2.5 text-sm font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0]"
          >
            ▶ Resume Timer
          </button>
          <button
            onClick={handleStop}
            style={{ cursor: 'default' }}
            className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] py-2.5 text-sm font-semibold text-white transition hover:border-red-500 hover:text-red-400"
          >
            ■ Stop Timer
          </button>
        </div>

      </div>
    </div>
  );
}
