import { useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { subscribe, dismiss } from "../lib/toast.js";

const ICON = { error: "alert", success: "check", info: "shieldCheck" };

export function Toasts() {
  const [list, setList] = useState([]);
  useEffect(() => subscribe(setList), []);
  return (
    <div className="toasts">
      {list.map((t) => (
        <button key={t.id} className={`toast toast-${t.level}`} onClick={() => dismiss(t.id)}>
          <Icon name={ICON[t.level] || "shieldCheck"} size={15} />
          <span>{t.message}</span>
        </button>
      ))}
    </div>
  );
}
