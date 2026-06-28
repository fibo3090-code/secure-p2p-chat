import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App.jsx";
import "./styles.css";

// Hide WebView2's default right-click menu everywhere except editable fields,
// so inputs keep their native copy/paste menu but the app chrome stays clean.
document.addEventListener("contextmenu", (e) => {
  const el = e.target;
  const editable = el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  if (!editable) e.preventDefault();
});

createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
