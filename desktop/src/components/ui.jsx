// Shared presentational primitives — clean ES-module port of the mockup's
// ui.jsx, reusing the same class names so the design-system CSS applies.
import { useState, useId, useRef, useEffect } from "react";
import { Icon } from "../lib/Icon.jsx";

export const cx = (...a) => a.filter(Boolean).join(" ");

export function Button({ children, variant = "primary", size = "md", icon, iconRight, full, ...rest }) {
  return (
    <button className={cx("btn", "btn-" + variant, "btn-" + size, full && "btn-full")} {...rest}>
      {icon && <Icon name={icon} size={size === "sm" ? 15 : 17} />}
      {children && <span>{children}</span>}
      {iconRight && <Icon name={iconRight} size={size === "sm" ? 15 : 17} />}
    </button>
  );
}

export function IconButton({ name, label, active, size = 18, ...rest }) {
  return (
    <button className={cx("icon-btn", active && "is-active")} title={label} aria-label={label} {...rest}>
      <Icon name={name} size={size} />
    </button>
  );
}

export function Avatar({ name, size = 38, state, party }) {
  const initials = (name || "?").split(/\s+/).map((w) => w[0]).join("").slice(0, 2).toUpperCase();
  const hue = [...(name || "")].reduce((a, c) => a + c.charCodeAt(0), 0) % 360;
  const bg = `oklch(0.62 0.13 ${hue})`;
  return (
    <span className="avatar-wrap" style={{ width: size, height: size }}>
      <span className="avatar" style={{
        background: party ? "transparent" : bg, width: size, height: size,
        fontSize: size * 0.36, border: party ? "1.5px solid var(--accent)" : "none",
        color: party ? "var(--accent)" : "#fff",
      }}>
        {party ? <Icon name="users" size={size * 0.5} /> : initials}
      </span>
      {state && <span className={cx("state-dot", "state-" + state)} title={state} />}
    </span>
  );
}

const TRUST = {
  verified: { icon: "shieldCheck", label: "Verified", cls: "trust-ok" },
  unverified: { icon: "alert", label: "Unverified", cls: "trust-warn" },
  party: { icon: "users", label: "Party", cls: "trust-party" },
};
export function TrustBadge({ trust, mini }) {
  const t = TRUST[trust] || TRUST.unverified;
  return (
    <span className={cx("trust-badge", t.cls, mini && "is-mini")}>
      <Icon name={t.icon} size={mini ? 12 : 13} />
      {!mini && <span>{t.label}</span>}
    </span>
  );
}

// Transport = how the bytes travel (Phase 3 conversation model). Distinct from
// trust: a relayed DM can still be verified. Kind chip rides along for groups.
const TRANSPORT = {
  direct: { icon: "plug", label: "Direct P2P", cls: "tr-direct" },
  relay: { icon: "satellite", label: "Via relay", cls: "tr-relay" },
  server: { icon: "server", label: "Server", cls: "tr-server" },
};
const KIND = {
  group: { icon: "users", label: "Group" },
  channel: { icon: "hash", label: "Channel" },
};
export function TransportBadge({ transport, kind, mini }) {
  const t = TRANSPORT[transport] || TRANSPORT.direct;
  const k = KIND[kind];
  return (
    <span className={cx("transport-badge", t.cls, mini && "is-mini")} title={t.label}>
      <Icon name={t.icon} size={mini ? 12 : 13} />
      {!mini && <span>{t.label}</span>}
      {k && <span className="tb-kind"><Icon name={k.icon} size={mini ? 11 : 12} />{!mini && k.label}</span>}
    </span>
  );
}

export function Input({ mono, ...rest }) {
  return <input className={cx("input", mono && "is-mono")} {...rest} />;
}

export function PasswordInput(props) {
  const [show, setShow] = useState(false);
  return (
    <span className="pw-wrap">
      <input className="input" type={show ? "text" : "password"} {...props} />
      <button type="button" className="pw-toggle" onClick={() => setShow((s) => !s)} aria-label="Toggle visibility">
        <Icon name={show ? "eyeOff" : "eye"} size={16} />
      </button>
    </span>
  );
}

export function Modal({ open, onClose, children, width = 460, title, icon, sub }) {
  // Per-instance ids: several Modals can be mounted at once (rename + verify,
  // etc.), so static ids would collide and break aria-labelledby/describedby.
  const titleId = useId();
  const subId = useId();
  // Move focus into the dialog on open so the Escape handler is reachable for
  // keyboard users and screen readers announce it.
  const ref = useRef(null);
  useEffect(() => { if (open) ref.current?.focus(); }, [open]);
  if (!open) return null;
  return (
    <div className="modal-scrim" onMouseDown={onClose}>
      <div
        ref={ref}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={title ? titleId : undefined}
        aria-describedby={sub ? subId : undefined}
        tabIndex={-1}
        style={{ width }}
        onKeyDown={(e) => { if (e.key === "Escape") onClose?.(); }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {title && (
          <div className="modal-head">
            <div className="modal-head-l">
              {icon && <span className="modal-icon"><Icon name={icon} size={18} /></span>}
              <div>
                <div id={titleId} className="modal-title">{title}</div>
                {sub && <div id={subId} className="modal-sub">{sub}</div>}
              </div>
            </div>
            <button className="icon-btn" onClick={onClose} aria-label="Close"><Icon name="x" size={18} /></button>
          </div>
        )}
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}
