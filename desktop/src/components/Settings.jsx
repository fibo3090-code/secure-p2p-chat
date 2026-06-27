// Minimal real Settings pane — identity, appearance, about.
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar } from "./ui.jsx";
import { THEMES } from "../lib/themes.js";

export function Settings({ identity, theme, setTheme }) {
  return (
    <div className="chat-pane">
      <header className="chat-head">
        <div className="chat-head-l">
          <div className="chat-head-info">
            <div className="chat-head-name">Settings</div>
            <div className="chat-head-sub mono">appearance · identity</div>
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
