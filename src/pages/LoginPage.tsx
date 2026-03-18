import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { User } from "../types";

interface Props {
  onLogin: (user: User) => void;
}

// UX-03: Parse raw Rust errors into user-friendly messages.
function parseLoginError(err: unknown): string {
  const msg = typeof err === "string" ? err : String(err);
  if (
    msg.includes("401") ||
    /unauthorized|invalid.*credential|wrong.*password|incorrect.*password/i.test(msg)
  ) {
    return "Invalid email or password";
  }
  if (/timed?\s*out|timeout/i.test(msg)) {
    return "Connection timed out";
  }
  if (
    /network|connection refused|unreachable|no.*internet|offline|dns|socket/i.test(msg)
  ) {
    return "No internet connection";
  }
  return "Something went wrong. Please try again.";
}

export default function LoginPage({ onLogin }: Props) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false); // UX-19
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // UX-16: noValidate on form — we handle validation manually
  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);

    const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
    if (!emailRegex.test(email)) {
      setError("Please enter a valid email address");
      return;
    }
    if (password.length < 6) {
      setError("Password must be at least 6 characters");
      return;
    }

    setLoading(true);
    try {
      const user = await invoke<User>("cmd_login", { email, password });
      onLogin(user);
    } catch (err) {
      setError(parseLoginError(err)); // UX-03
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

        {/* UX-16: noValidate — validation handled above */}
        <form onSubmit={handleSubmit} noValidate className="space-y-3">
          <div>
            {/* UX-15: autoFocus on email input */}
            <input
              type="email"
              placeholder="Email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              autoFocus
              required
              className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-4 py-2.5 text-sm text-white placeholder-[#4a4a4a] outline-none transition focus:border-[#6ee7b7] focus:ring-1 focus:ring-[#6ee7b7]/20"
            />
          </div>
          {/* UX-19: Show/hide password toggle */}
          <div className="relative">
            <input
              type={showPassword ? "text" : "password"}
              placeholder="Password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              className="w-full rounded-lg bg-[#1a1a1a] border border-[#2a2a2a] px-4 py-2.5 pr-16 text-sm text-white placeholder-[#4a4a4a] outline-none transition focus:border-[#6ee7b7] focus:ring-1 focus:ring-[#6ee7b7]/20"
            />
            <button
              type="button"
              onClick={() => setShowPassword((v) => !v)}
              tabIndex={-1}
              aria-label={showPassword ? "Hide password" : "Show password"}
              className="absolute right-3 top-1/2 -translate-y-1/2 text-[10px] text-[#4a4a4a] hover:text-[#6ee7b7] transition font-medium select-none"
            >
              {showPassword ? "Hide" : "Show"}
            </button>
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
