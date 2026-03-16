import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import LoginPage from "./pages/LoginPage";
import TrackerPage from "./pages/TrackerPage";
import { User } from "./types";

export default function App() {
  const [user, setUser] = useState<User | null>(null);
  const [checking, setChecking] = useState(true);

  useEffect(() => {
    invoke<User | null>("cmd_get_current_user")
      .then((u) => setUser(u))
      .catch(() => setUser(null))
      .finally(() => setChecking(false));
  }, []);

  if (checking) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-[#0f0f0f]">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-[#6ee7b7] border-t-transparent" />
      </div>
    );
  }

  return user ? (
    <TrackerPage user={user} onLogout={() => setUser(null)} />
  ) : (
    <LoginPage onLogin={setUser} />
  );
}
