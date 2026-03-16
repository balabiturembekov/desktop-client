import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { User } from "../types";

interface Props {
  onLogin: (user: User) => void;
}

export default function LoginPage({ onLogin }: Props) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setLoading(true);
    try {
      const user = await invoke<User>("cmd_login", { email, password });
      onLogin(user);
    } catch (err) {
      setError(typeof err === "string" ? err : "Login failed");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="flex h-screen w-screen items-center justify-center bg-[#0f0f0f]">
      <div className="w-full max-w-[320px] px-6">

        {/* Logo */}
        <div className="mb-8 text-center">
          <div className="inline-flex items-center gap-2">
            <div className="h-7 w-7 rounded-lg bg-[#6ee7b7] flex items-center justify-center">
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <circle cx="8" cy="8" r="3" fill="#0f0f0f"/>
                <path d="M8 2v2M8 12v2M2 8h2M12 8h2" stroke="#0f0f0f" strokeWidth="1.5" strokeLinecap="round"/>
              </svg>
            </div>
            <span className="text-white font-semibold tracking-wide text-lg">Hubnity</span>
          </div>
          <p className="mt-2 text-[#4a4a4a] text-sm">Track your work time</p>
        </div>

        {/* Form */}
        <form onSubmit={handleSubmit} className="space-y-3">
          <div>
            <input
              type="email"
              placeholder="Email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              required
              className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-4 py-2.5 text-sm text-white placeholder-[#4a4a4a] outline-none transition focus:border-[#6ee7b7] focus:ring-1 focus:ring-[#6ee7b7]/20"
            />
          </div>
          <div>
            <input
              type="password"
              placeholder="Password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-4 py-2.5 text-sm text-white placeholder-[#4a4a4a] outline-none transition focus:border-[#6ee7b7] focus:ring-1 focus:ring-[#6ee7b7]/20"
            />
          </div>

          {error && (
            <p className="text-xs text-red-400 px-1">{error}</p>
          )}

          <button
            type="submit"
            disabled={loading}
            className="w-full rounded-lg bg-[#6ee7b7] py-2.5 text-sm font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0] disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {loading ? (
              <span className="flex items-center justify-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-[#0f0f0f] border-t-transparent" />
                Signing in...
              </span>
            ) : (
              "Sign in"
            )}
          </button>
        </form>
      </div>
    </div>
  );
}
