// Settings pane — identity, messaging/privacy, hosting, files, appearance, about.
// Only settings the runtime actually honors are shown; every change saves
// immediately (config persists inside the encrypted history file).
import { useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar, Button, Modal, PasswordInput } from "./ui.jsx";
import { THEMES } from "../lib/themes.js";
import { api, friendlyError } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";

function Toggle({ on, onChange, label, hint }) {
  return (
    <button className={cx("set-row", "set-toggle")} onClick={() => onChange(!on)} role="switch" aria-checked={on}>
      <span className="set-row-txt">
        <span className="set-row-label">{label}</span>
        {hint && <span className="set-row-hint">{hint}</span>}
      </span>
      <span className={cx("set-switch", on && "is-on")}><span className="set-knob" /></span>
    </button>
  );
}

// Rotate the password protecting the identity file. The key inside is
// untouched, so history stays readable and contacts keep seeing the same
// fingerprint — the copy says so, because "change password" in a messenger is
// exactly the kind of action people expect to cost them their history.
function ChangePasswordDialog({ open, onClose, minLength }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  // Clear on every close: this component stays mounted, and leaving a typed
  // password sitting in state (and in the DOM) until the next open is exactly
  // what a password field must not do.
  useEffect(() => {
    if (open) return;
    setCurrent(""); setNext(""); setConfirm(""); setErr(""); setBusy(false);
  }, [open]);

  if (!open) return null;

  const tooShort = next.length > 0 && [...next].length < minLength;
  const mismatch = confirm.length > 0 && next !== confirm;
  const ready = current && next && next === confirm && !tooShort && !busy;

  async function submit() {
    if (!ready) return;
    setBusy(true);
    setErr("");
    try {
      await api.changePassword(current, next);
      toast("Password changed. Use the new one next time you unlock.", "success");
      onClose();
    } catch (e) {
      setErr(friendlyError(e));
    } finally { setBusy(false); }
  }

  return (
    <Modal open onClose={onClose} width={440} title="Change password" icon="lock"
      sub="Re-encrypts the identity file on this device">
      <div className="creator-pane">
        <p className="creator-lead">
          Your messages, contacts and fingerprint stay exactly as they are — only
          the password that unlocks this device changes. There is still no reset:
          if you forget the new one, the identity is gone.
        </p>
        <PasswordInput value={current} autoFocus placeholder="Current password"
          autoComplete="current-password"
          autoComplete="current-password"
          onChange={(e) => setCurrent(e.target.value)} />
        <PasswordInput value={next} placeholder={`New password (at least ${minLength} characters)`}
          autoComplete="new-password"
          autoComplete="new-password"
          onChange={(e) => setNext(e.target.value)} />
        <PasswordInput value={confirm} placeholder="Repeat the new password"
          autoComplete="new-password"
          autoComplete="new-password"
          onChange={(e) => setConfirm(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()} />
        {tooShort && <div className="onb-err"><Icon name="alert" size={13} /> At least {minLength} characters.</div>}
        {mismatch && <div className="onb-err"><Icon name="alert" size={13} /> The two new passwords don't match.</div>}
        {err && <div className="onb-err"><Icon name="alert" size={13} /> {err}</div>}
        <div style={{ display: "flex", gap: 10, justifyContent: "flex-end" }}>
          <Button variant="ghost" onClick={onClose} disabled={busy}>Cancel</Button>
          <Button icon="check" onClick={submit} disabled={!ready}>
            {busy ? "Re-encrypting…" : "Change password"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

export function Settings({ identity, theme, setTheme, onIdentityChanged }) {
  const [s, setS] = useState(null);
  const [portDraft, setPortDraft] = useState("");
  const [nameDraft, setNameDraft] = useState(identity?.name || "");
  const [pwOpen, setPwOpen] = useState(false);

  useEffect(() => {
    api.getSettings().then((v) => { setS(v); setPortDraft(String(v.listen_port)); }).catch(() => {});
  }, []);
  useEffect(() => { setNameDraft(identity?.name || ""); }, [identity?.name]);

  async function commitName() {
    const name = nameDraft.trim();
    if (!name || name === identity?.name) { setNameDraft(identity?.name || ""); return; }
    try {
      await api.setDisplayName(name);
      toast("Display name updated.", "success");
      onIdentityChanged && onIdentityChanged();
    } catch (e) { toast(friendlyError(e), "error"); setNameDraft(identity?.name || ""); }
  }

  // Apply one field and save; roll back the UI if the bridge rejects it.
  async function apply(patch) {
    const next = { ...s, ...patch };
    setS(next);
    try {
      await api.updateSettings({
        enable_notifications: next.enable_notifications,
        enable_typing_indicators: next.enable_typing_indicators,
        auto_host_on_startup: next.auto_host_on_startup,
        listen_port: next.listen_port,
        enable_upnp: next.enable_upnp,
        auto_accept_files: next.auto_accept_files,
        auto_connect: next.auto_connect,
        enable_mdns: next.enable_mdns,
      });
    } catch (e) { toast(friendlyError(e), "error"); setS(s); }
  }

  function commitPort() {
    const port = Number(portDraft);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      toast("Port must be a number between 1 and 65535.", "error");
      setPortDraft(String(s.listen_port));
      return;
    }
    if (port !== s.listen_port) apply({ listen_port: port });
  }

  const backedUp = !!s?.identity_backed_up_at;

  async function changeDownloadDir() {
    try {
      const dir = await api.pickDownloadDir();
      if (dir) { setS((cur) => ({ ...cur, download_dir: dir })); toast("Download folder updated.", "success"); }
    } catch (e) { toast(friendlyError(e), "error"); }
  }

  return (
    <div className="chat-pane">
      <header className="chat-head">
        <div className="chat-head-l">
          <div className="chat-head-info">
            <div className="chat-head-name">Settings</div>
            <div className="chat-head-sub mono">identity · privacy · hosting · files · appearance</div>
          </div>
        </div>
      </header>

      <div className="chat-scroll" style={{ padding: 24, maxWidth: 620 }}>
        <section className="set-block">
          <div className="set-h">Your identity</div>
          <div className="set-id">
            <Avatar name={identity?.name} size={46} />
            <div>
              <input className="set-id-name-input" value={nameDraft} maxLength={48}
                aria-label="Display name"
                onChange={(e) => setNameDraft(e.target.value)}
                onBlur={commitName}
                onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()} />
              <code className="set-id-fp">{(identity?.fingerprint || "").replace(/(.{4})/g, "$1 ").trim()}</code>
            </div>
          </div>
          <div className="set-note">Your display name goes into the invite links you share. Keys are stored encrypted on this device and never leave it.</div>
          {/* Losing the identity file is the one failure this app cannot undo,
              so the row states plainly whether a backup exists rather than
              offering an Export button with no indication either way. */}
          <div className={cx("set-row", !backedUp && "is-warn")}>
            <span className="set-row-txt">
              <span className="set-row-label">
                Identity backup
                {s && (backedUp
                  ? <span className="set-badge is-ok">Backed up</span>
                  : <span className="set-badge is-warn">Never backed up</span>)}
              </span>
              <span className="set-row-hint">
                {backedUp
                  ? `Last saved ${new Date(s.identity_backed_up_at).toLocaleString()}. Save a fresh copy if you have changed devices.`
                  : "Save an encrypted copy of your identity file. Without it (and your password), a lost disk means a lost identity — there is no reset."}
              </span>
            </span>
            <button className="set-change" onClick={async () => {
              try {
                const dest = await api.exportIdentity();
                if (dest) {
                  toast(`Backup saved to ${dest}`, "success");
                  api.getSettings().then(setS).catch(() => {});
                }
              } catch (e) { toast(friendlyError(e), "error"); }
            }}>
              <Icon name="copy" size={14} /> {backedUp ? "Export again" : "Export now"}
            </button>
          </div>
          {/* A security product with no way to rotate its one password is a gap
              users notice — and a leaked or shoulder-surfed password is exactly
              the moment someone goes looking for this. */}
          <div className="set-row">
            <span className="set-row-txt">
              <span className="set-row-label">Password</span>
              <span className="set-row-hint">
                Unlocks this device's identity file. Changing it re-encrypts the
                file — your messages, contacts and fingerprint are unaffected.
              </span>
            </span>
            <button className="set-change" onClick={() => setPwOpen(true)}>
              <Icon name="lock" size={14} /> Change
            </button>
          </div>
        </section>

        <ChangePasswordDialog open={pwOpen} onClose={() => setPwOpen(false)}
          minLength={identity?.min_password_len ?? 12} />

        {s && (
          <>
            <section className="set-block">
              <div className="set-h">Messaging &amp; privacy</div>
              <Toggle label="Desktop notifications" hint="Notify when a message arrives in the background"
                on={s.enable_notifications} onChange={(v) => apply({ enable_notifications: v })} />
              <Toggle label="Send typing indicators" hint="Peers can see when you are typing (they see nothing when off)"
                on={s.enable_typing_indicators} onChange={(v) => apply({ enable_typing_indicators: v })} />
            </section>

            <section className="set-block">
              <div className="set-h">Hosting</div>
              <Toggle label="Host automatically on startup" hint="Start listening for peers as soon as the app unlocks"
                on={s.auto_host_on_startup} onChange={(v) => apply({ auto_host_on_startup: v })} />
              <Toggle label="UPnP port mapping" hint="Ask the router to make the host reachable from the internet; the external address goes into your invite"
                on={s.enable_upnp} onChange={(v) => apply({ enable_upnp: v })} />
              <Toggle label="Reconnect contacts on startup" hint="Dial your saved contacts automatically after unlock"
                on={s.auto_connect} onChange={(v) => apply({ auto_connect: v })} />
              <Toggle label="LAN peer discovery (mDNS)" hint="Find nearby peers and advertise yourself on the local network — reveals your name and fingerprint on the LAN"
                on={s.enable_mdns} onChange={(v) => apply({ enable_mdns: v })} />
              <div className="set-row">
                <span className="set-row-txt">
                  <span className="set-row-label">Listening port</span>
                  <span className="set-row-hint">Used when hosting (default 12345)</span>
                </span>
                <input className="set-port mono" value={portDraft} inputMode="numeric"
                  onChange={(e) => setPortDraft(e.target.value)}
                  onBlur={commitPort}
                  onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()} />
              </div>
            </section>

            <section className="set-block">
              <div className="set-h">Files</div>
              <Toggle label="Auto-accept incoming files" hint="When off, each incoming file must be accepted in the conversation before it is saved"
                on={s.auto_accept_files} onChange={(v) => apply({ auto_accept_files: v })} />
              <div className="set-row">
                <span className="set-row-txt">
                  <span className="set-row-label">Download folder</span>
                  <span className="set-row-hint mono">{s.download_dir}</span>
                </span>
                <button className="set-change" onClick={changeDownloadDir}>
                  <Icon name="folder" size={14} /> Change
                </button>
              </div>
            </section>
          </>
        )}

        <section className="set-block">
          <div className="set-h">Appearance</div>
          <div className="set-themes">
            {THEMES.map((t) => (
              <button key={t.id} className={cx("set-theme", theme === t.id && "is-active")} onClick={() => setTheme(t.id)}>
                <span className="set-theme-dot" style={{ background: t.swatch }} />
                {t.label}
                {theme === t.id && <Icon name="check" size={15} />}
              </button>
            ))}
          </div>
        </section>

        <section className="set-block">
          <div className="set-h">Support</div>
          <div className="set-row">
            <span className="set-row-txt">
              <span className="set-row-label">Export diagnostics</span>
              <span className="set-row-hint">A bundle with app state metadata and config — never keys or message content. Useful when reporting a bug.</span>
            </span>
            <button className="set-change" onClick={async () => {
              try { const p = await api.exportDiagnostics(); toast(`Diagnostics exported to ${p}`, "success"); }
              catch (e) { toast(friendlyError(e), "error"); }
            }}>
              <Icon name="file" size={14} /> Export
            </button>
          </div>
          <div className="set-row">
            <span className="set-row-txt">
              <span className="set-row-label">Data folder</span>
              <span className="set-row-hint">Where your identity, encrypted history and diagnostics live</span>
            </span>
            <button className="set-change" onClick={async () => {
              try { await api.openDataDir(); } catch (e) { toast(friendlyError(e), "error"); }
            }}>
              <Icon name="folder" size={14} /> Open
            </button>
          </div>
        </section>

        <section className="set-block">
          <div className="set-h">How P2PEM works</div>
          <div className="set-note">
            P2PEM is an encrypted peer-to-peer messenger: conversations go directly
            between you and your peer (or through a blind relay that only forwards
            bytes), end-to-end encrypted with forward secrecy. There is no account
            and no central server holding your messages.
          </div>
          <div className="set-note">
            <strong>Connecting:</strong> one side hosts (or opens a relay session) and shares
            an address, invite link, or relay token; the other side dials it. With LAN
            discovery enabled, nearby peers appear automatically in the connect pane.
          </div>
          <div className="set-note">
            <strong>Trust:</strong> the first time you talk to someone, compare the short
            verification code (six digits + three emoji) over another channel — a call
            or in person — before accepting. If it matches on both ends, nobody is in
            the middle; the app then remembers that fingerprint and warns you if it
            ever changes. A ✓ next to your message means the peer's device confirmed
            receipt.
          </div>
        </section>
      </div>
    </div>
  );
}
