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

// Initials from the letters of a name only: "Listening :12345" used to render
// as "L:", because the first character of each word was taken verbatim.
function initialsOf(name) {
  const words = String(name || "").split(/\s+/)
    .map((w) => w.replace(/[^\p{L}\p{N}]/gu, ""))
    .filter(Boolean);
  return (words.map((w) => [...w][0]).join("").slice(0, 2) || "?").toUpperCase();
}

// A curated avatar palette instead of a computed hue: `oklch(.62 .13 h)` landed
// on olive and sage as often as on anything clean. Daylight tints these (see
// .avatar in the CSS).
// Eight hues spaced evenly round the wheel (no two reds), each holding white
// initials at ≥5:1, so neighbours in a list differ at a glance.
const AVATAR_COLORS = [
  "#4f46e5", "#0369a1", "#0f766e", "#15803d", "#a16207", "#c2410c", "#be123c", "#7e22ce",
];

// FNV-1a over code points: a plain character sum sent "Alice" and "Carol" to
// neighbouring reds; this spreads short names across the palette.
function nameHash(name) {
  let h = 2166136261;
  for (const ch of String(name || "")) { h ^= ch.codePointAt(0); h = Math.imul(h, 16777619) >>> 0; }
  return h;
}

const STATE_WORD = { connected: "connected", hosting: "waiting for a peer", offline: "offline", connecting: "connecting" };

function AvatarInner({ name, size = 38, state, party, icon }) {
  // Both derive only from `name`, and an avatar is rendered once per
  // conversation row on every poll tick — so the string walk was running for
  // every row, several times a second, to produce the same number each time.
  const { initials, bg } = useMemo(() => {
    const letters = initialsOf(name);
    return { initials: letters, bg: AVATAR_COLORS[nameHash(name) % AVATAR_COLORS.length] };
  }, [name]);
  return (
    <span className="avatar-wrap" style={{ width: size, height: size }}>
      <span className={cx("avatar", (party || icon) && "is-glyph")} style={{
        "--av": party || icon ? undefined : bg, width: size, height: size,
        fontSize: size * 0.38,
      }}>
        {party ? <Icon name="users" size={size * 0.5} />
          : icon ? <Icon name={icon} size={size * 0.46} />
            : initials}
      </span>
      {state && <span className={cx("state-dot", "state-" + state)} title={STATE_WORD[state] || state} />}
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
    // A mini badge is an icon, so it carries its word for everyone else: a
    // tooltip for the pointer and hidden text for a screen reader. The mini
    // form used to be silent, which made "Verified" the one fact about a
    // conversation a reader could not hear.
    <span className={cx("trust-badge", t.cls, mini && "is-mini")} title={mini ? t.label : undefined}>
      <Icon name={t.icon} size={mini ? 12 : 13} />
      {mini ? <span className="sr-only">{t.label}</span> : <span>{t.label}</span>}
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

/// One page template for every full-width destination (Contacts, Relays,
/// Communities, Settings). Each used to lay itself out differently — a centred
/// form, an offset column, cards packed top-left — so the app read as several
/// tools sharing a sidebar. Title + one plain-language subtitle, actions on the
/// right, and one content column.
export function Page({ title, sub, actions, wide, nav, children, bodyClass }) {
  const head = (
    <header className="page-head">
      <div className="page-head-txt">
        <h1 className="page-title">{title}</h1>
        {sub && <p className="page-sub">{sub}</p>}
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </header>
  );
  // With a section index (Settings), the index is a column against the rail
  // and the content sits left-aligned beside it, as in a system settings
  // window — not a floating nav centred in empty space.
  if (nav) {
    return (
      <div className="page is-settings">
        {nav}
        <div className="page-main page-scroll">
          {head}
          <div className={cx("page-body", bodyClass)}>{children}</div>
        </div>
      </div>
    );
  }
  return (
    <div className="page">
      <div className="page-scroll">
        {head}
        <div className={cx("page-body", wide && "is-wide", bodyClass)}>{children}</div>
      </div>
    </div>
  );
}

/// A ⋯ menu with labelled items. Secondary and destructive actions live here
/// instead of a row of look-alike icon buttons, where "block" (an ✕) sat next
/// to "delete" (a bin) with nothing but a tooltip to tell them apart.
/// `items`: `{ icon, label, onClick, danger, disabled }`, or `"sep"`.
export function OverflowMenu({ label = "More actions", items }) {
  const [open, setOpen] = useState(false);
  const ref = useRef(null);
  useEffect(() => {
    if (!open) return undefined;
    const onDown = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    const onKey = (e) => { if (e.key === "Escape") setOpen(false); };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("mousedown", onDown); window.removeEventListener("keydown", onKey); };
  }, [open]);
  return (
    <div className="pop-wrap" ref={ref}>
      <IconButton name="more" label={label} active={open} aria-haspopup="menu" aria-expanded={open}
        onClick={() => setOpen((o) => !o)} />
      {open && (
        <div className="pop-menu" role="menu">
          {items.filter(Boolean).map((it, i) => (it === "sep"
            ? <div key={`sep${i}`} className="pop-sep" role="separator" />
            : (
              <button key={it.label} role="menuitem" className={cx(it.danger && "is-danger")}
                disabled={it.disabled}
                onClick={() => { setOpen(false); it.onClick && it.onClick(); }}>
                {it.icon && <Icon name={it.icon} size={15} />} {it.label}
              </button>
            )))}
        </div>
      )}
    </div>
  );
}
