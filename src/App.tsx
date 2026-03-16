import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import LoginPage from "./pages/LoginPage";
import TrackerPage from "./pages/TrackerPage";
import { User } from "./types";

export default function App() {
  const [user, setUser] = useState<User | null>(null);
  const [checking, setChecking] = useState(true);
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);

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
        <div className="fixed top-0 left-0 right-0 z-50 bg-[#6ee7b7] text-[#0f0f0f] text-xs font-semibold text-center py-1.5">
          ↑ Updating to v{updateVersion}...
        </div>
      )}
      {user ? (
        <TrackerPage user={user} onLogout={() => setUser(null)} />
      ) : (
        <LoginPage onLogin={setUser} />
      )}
    </>
  );
}
