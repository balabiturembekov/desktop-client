import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import IdlePage from "./pages/IdlePage";
import "./App.css";

const isIdle = window.location.pathname === "/idle";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isIdle ? <IdlePage /> : <App />}
  </React.StrictMode>,
);
