import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import IdlePage from "./pages/IdlePage";
import "./App.css";

// BUG-F14: Catch unexpected render errors so the app shows a recovery UI
// instead of a blank white screen.
class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { hasError: boolean; message: string }
> {
  constructor(props: { children: React.ReactNode }) {
    super(props);
    this.state = { hasError: false, message: "" };
  }

  static getDerivedStateFromError(error: unknown) {
    return {
      hasError: true,
      message: error instanceof Error ? error.message : String(error),
    };
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="flex h-screen w-screen flex-col items-center justify-center gap-4 bg-[#0f0f0f] text-white px-6">
          <p className="text-sm text-[#888] text-center">
            Something went wrong. Please restart the app.
          </p>
          <p className="text-xs text-[#444] text-center font-mono max-w-xs break-words">
            {this.state.message}
          </p>
          <button
            onClick={() => window.location.reload()}
            className="rounded-lg bg-[#6ee7b7] px-4 py-2 text-xs font-semibold text-[#0f0f0f] transition hover:bg-[#a7f3d0]"
          >
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

const isIdle = window.location.pathname === "/idle";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      {isIdle ? <IdlePage /> : <App />}
    </ErrorBoundary>
  </React.StrictMode>,
);
