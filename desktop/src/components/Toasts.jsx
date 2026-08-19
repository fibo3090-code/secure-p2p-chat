import { useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { subscribe, dismiss } from "../lib/toast.js";

const ICON = { error: "alert", success: "check", info: "shieldCheck" };

export function Toasts() {
  const [list, setList] = useState([]);
  useEffect(() => subscribe(setList), []);
  // Toasts are the app's only channel for errors — "not delivered", "fingerprint
  // verification required", "transfer interrupted". Without a live region a
  // screen-reader user is told none of it and the message is gone in four
  // seconds. Errors are `assertive` (interrupt: something failed); success and
  // info are `polite` (wait for a pause). They are split into two regions
  // because a region's politeness is fixed per element.
  const errors = list.filter((t) => t.level === "error");
  const rest = list.filter((t) => t.level !== "error");
  const item = (t) => (
    <button key={t.id} className={`toast toast-${t.level}`} onClick={() => dismiss(t.id)}
      aria-label={`${t.level === "error" ? "Error" : "Notice"}: ${t.message}. Activate to dismiss.`}>
      <Icon name={ICON[t.level] || "shieldCheck"} size={15} />
      <span>{t.message}</span>
    </button>
  );
  return (
    <div className="toasts">
      <div role="alert" aria-live="assertive" aria-atomic="false" className="toast-region">
        {errors.map(item)}
      </div>
      <div role="status" aria-live="polite" aria-atomic="false" className="toast-region">
        {rest.map(item)}
      </div>
    </div>
  );
}
