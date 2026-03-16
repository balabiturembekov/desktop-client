import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import LoginPage from "./pages/LoginPage";
import TrackerPage from "./pages/TrackerPage";
import { User } from "./types";

interface ClosePayload {
  unsynced_count: number;
  timer_running: boolean;
}

export default function App() {
  const [user, setUser] = useState<User | null>(null);
  const [checking, setChecking] = useState(true);
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [closeModal, setCloseModal] = useState<ClosePayload | null>(null);
  const [closing, setClosing] = useState(false);

  useEffect(() => {
    invoke<User | null>("cmd_get_current_user")
      .then((u) => setUser(u))
      .catch(() => setUser(null))
      .finally(() => setChecking(false));
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<string>("update-available", (e) => {
      setUpdateVersion(e.payload);
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<ClosePayload>("close-requested-with-unsynced", (e) => {
      setCloseModal(e.payload);
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  const handleWaitForSync = () => setCloseModal(null);

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

  return (
    <>
      {/* Update banner */}
      {updateVersion && (
        <div className="fixed top-0 left-0 right-0 z-40 bg-[#6ee7b7] text-[#0f0f0f] text-xs font-semibold text-center py-1.5">
          ↑ Updating to v{updateVersion}...
        </div>
      )}

      {user ? (
        <TrackerPage user={user} onLogout={() => setUser(null)} />
      ) : (
        <LoginPage onLogin={setUser} />
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

            {/* Buttons */}
            <div className="flex flex-col gap-2">
              {closeModal.timer_running && (
                <button
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
              {!closeModal.timer_running && closeModal.unsynced_count > 0 && (
                <button
                  onClick={handleWaitForSync}
                  disabled={closing}
                  className="w-full rounded-lg bg-[#6ee7b7] py-2 text-xs font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0] disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  Wait for sync
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
