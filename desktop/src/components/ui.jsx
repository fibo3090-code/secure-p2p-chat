// Shared presentational primitives — clean ES-module port of the mockup's
// ui.jsx, reusing the same class names so the design-system CSS applies.
import { memo, useMemo, useState, useId, useRef, useEffect } from "react";
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

function AvatarInner({ name, size = 38, state, party }) {
  // Both derive only from `name`, and an avatar is rendered once per
  // conversation row on every poll tick — so the string walk was running for
  // every row, several times a second, to produce the same number each time.
  const { initials, bg } = useMemo(() => {
    const letters = (name || "?").split(/\s+/).map((w) => w[0]).join("").slice(0, 2).toUpperCase();
    const hue = [...(name || "")].reduce((a, c) => a + c.charCodeAt(0), 0) % 360;
    return { initials: letters, bg: `oklch(0.62 0.13 ${hue})` };
  }, [name]);
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

/// Memoised: props are scalars, so the default shallow compare is exactly right
/// here — unlike the message and conversation rows, whose props are rebuilt
/// objects and need comparison by value.
export const Avatar = memo(AvatarInner);

const TRUST = {
  verified: { icon: "shieldCheck", label: "Verified", cls: "trust-ok" },
  trusted: { icon: "shieldCheck", label: "Trusted", cls: "trust-ok" },
  unverified: { icon: "alert", label: "Unverified", cls: "trust-warn" },
  blocked: { icon: "x", label: "Blocked", cls: "trust-blocked" },
  party: { icon: "users", label: "Community", cls: "trust-party" },
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

/// `autoComplete` is required, not optional, and there is no sensible default:
/// a password manager offered "current-password" on a *new* password field will
/// happily fill the old one, and offered "new-password" on an unlock field it
/// suggests a fresh password to someone trying to get back in. Callers say which
/// this is; the prop is passed through explicitly so it cannot be forgotten
/// silently.
export function PasswordInput({ autoComplete = "off", ...props }) {
  const [show, setShow] = useState(false);
  return (
    <span className="pw-wrap">
      <input className="input" type={show ? "text" : "password"}
        autoComplete={autoComplete} {...props} />
      <button type="button" className="pw-toggle" onClick={() => setShow((s) => !s)} aria-label="Toggle visibility">
        <Icon name={show ? "eyeOff" : "eye"} size={16} />
      </button>
    </span>
  );
}

// Everything the browser will let a user Tab to. Used to keep focus inside an
// open dialog — `aria-modal` tells assistive tech the rest of the page is inert
// but does nothing to stop Tab actually leaving it.
const FOCUSABLE =
  'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

/// `dismissable={false}` removes Escape, the scrim click, and the close button.
/// Reserve it for dialogs that represent a decision the app cannot make on the
/// user's behalf — the TOFU prompt, where dismissing leaves a live session
/// waiting and the peer hanging with no explanation.
export function Modal({ open, onClose, children, width = 460, title, icon, sub, dismissable = true }) {
  // Per-instance ids: several Modals can be mounted at once (rename + verify,
  // etc.), so static ids would collide and break aria-labelledby/describedby.
  const titleId = useId();
  const subId = useId();
  const ref = useRef(null);
  // Whatever had focus before the dialog opened, so it can be handed back.
  const returnTo = useRef(null);

  useEffect(() => {
    if (!open) return undefined;
    returnTo.current = document.activeElement;
    // Focus the dialog itself rather than its first control: a confirmation
    // dialog must not open with the destructive button already focused, one
    // Space away from firing.
    ref.current?.focus();
    return () => {
      // Hand focus back to what opened the dialog, so a keyboard user is not
      // dumped at the top of the document.
      const el = returnTo.current;
      if (el && typeof el.focus === "function" && document.contains(el)) el.focus();
    };
  }, [open]);

  // Keep Tab inside the dialog. Without this, tabbing past the last control
  // walks into the app behind the scrim, where clicks are blocked but focus is
  // not — the user ends up typing into something they cannot see.
  function onKeyDown(e) {
    if (e.key === "Escape") {
      if (dismissable) onClose?.();
      return;
    }
    if (e.key !== "Tab" || !ref.current) return;
    const items = [...ref.current.querySelectorAll(FOCUSABLE)].filter(
      (el) => el.offsetParent !== null || el === document.activeElement,
    );
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    // Focus sitting on the dialog container counts as "before the first item".
    if (e.shiftKey && (document.activeElement === first || document.activeElement === ref.current)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }

  if (!open) return null;
  return (
    <div className="modal-scrim" onMouseDown={dismissable ? onClose : undefined}>
      <div
        ref={ref}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={title ? titleId : undefined}
        aria-describedby={sub ? subId : undefined}
        tabIndex={-1}
        style={{ width }}
        onKeyDown={onKeyDown}
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
            {dismissable && (
              <button className="icon-btn" onClick={onClose} aria-label="Close"><Icon name="x" size={18} /></button>
            )}
          </div>
        )}
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}
