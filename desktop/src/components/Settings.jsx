// Settings pane — identity, messaging/privacy, hosting, files, appearance, about.
// Only settings the runtime actually honors are shown; every change saves
// immediately (config persists inside the encrypted history file).
import { useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar } from "./ui.jsx";
import { THEMES } from "../lib/themes.js";
import { api } from "../lib/bridge.js";
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

export function Settings({ identity, theme, setTheme }) {
  const [s, setS] = useState(null);
  const [portDraft, setPortDraft] = useState("");

  useEffect(() => {
    api.getSettings().then((v) => { setS(v); setPortDraft(String(v.listen_port)); }).catch(() => {});
  }, []);

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
      });
    } catch (e) { toast(String(e), "error"); setS(s); }
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

  async function changeDownloadDir() {
    try {
      const dir = await api.pickDownloadDir();
      if (dir) { setS((cur) => ({ ...cur, download_dir: dir })); toast("Download folder updated.", "success"); }
    } catch (e) { toast(String(e), "error"); }
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
              <div className="set-id-name">{identity?.name || "—"}</div>
              <code className="set-id-fp">{(identity?.fingerprint || "").replace(/(.{4})/g, "$1 ").trim()}</code>
            </div>
          </div>
          <div className="set-note">Keys are stored encrypted on this device and never leave it.</div>
        </section>

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
          <div className="set-h">About</div>
          <div className="set-note">P2PEM — encrypted peer-to-peer messenger. Direct, end-to-end encrypted conversations with trust-on-first-use fingerprint verification.</div>
        </section>
      </div>
    </div>
  );
}
