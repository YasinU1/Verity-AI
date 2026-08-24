import React from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import { Dashboard } from "./windows/Dashboard";
import { Overlay } from "./windows/Overlay";

// Two windows, one bundle, routed by URL hash (#main / #overlay). Tauri loads
// index.html#main and index.html#overlay; a plain browser defaults to the dashboard.
function route() {
  const hash = window.location.hash.replace(/^#/, "");
  return hash === "overlay" ? <Overlay /> : <Dashboard />;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>{route()}</React.StrictMode>,
);
