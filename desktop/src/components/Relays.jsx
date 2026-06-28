// Relays surface — a relay is a blind byte-broker that lets two peers pair when
// neither can dial the other (no port-forwarding). It retains no data; a relayed
// conversation is still an end-to-end-encrypted, fingerprint-verified DM.
//
// Honest by design: this pane only exposes actions the backend actually backs
// (connect/host via relay through the bridge, run a local relay). Saved-relay
// and live-route tracking don't exist yet, so they're shown as explicit empty
// states rather than fabricated data.
import { useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Button, Input } from "./ui.jsx";
import { api } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";

const LOCAL_RELAY_CMD = "cargo run -p encodeur_rsa_rust -- --relay-server --port 9000";

function Copyable({ value }) {
  const [done, setDone] = useState(false);
  return (
    <div className="rl-cmd">
      <code>{value}</code>
      <button className="copy-btn" title="Copy" onClick={() => {
        try { navigator.clipboard.writeText(value); setDone(true); setTimeout(() => setDone(false), 1200); } catch { /* */ }
      }}><Icon name={done ? "check" : "copy"} size={15} /></button>
    </div>
  );
}

export function Relays({ onConnected }) {
  const [host, setHost] = useState("");
  const [token, setToken] = useState("");
  const [hostToken, setHostToken] = useState("");
  const [busy, setBusy] = useState(false);

  async function pair() {
    if (!host.trim()) return toast("Enter the relay address.", "error");
    if (!token.trim()) return toast("Enter the connection token your peer shared.", "error");
    setBusy(true);
    try {
      await api.connectViaRelay(host.trim(), token.trim());
      toast(`Connecting via relay ${host.trim()}…`, "success");
      onConnected && onConnected();
    } catch (e) { toast(String(e), "error"); }
    finally { setBusy(false); }
  }

  async function openHost() {
    if (!host.trim()) return toast("Enter the relay address to broker through.", "error");
    setBusy(true);
    setHostToken("");
    try {
      const t = await api.hostViaRelay(host.trim());
      setHostToken(t);
      toast("Relay session open — share the token with your peer", "success");
    } catch (e) { toast(String(e), "error"); }
    finally { setBusy(false); }
  }

  return (
    <div className="relay-pane">
      <div className="rl-scroll">
        <header className="rl-head">
          <div>
            <h2>Relays</h2>
            <p>A relay is a blind broker for peers that can't dial each other directly — no port-forwarding. It forwards bytes only; your chat stays end-to-end encrypted and fingerprint-verified.</p>
          </div>
        </header>

        <section className="rl-connect">
          <div className="rl-block-head">
            <h3>Pair through a relay</h3>
          </div>
          <p className="creator-lead" style={{ marginBottom: 10 }}>Both peers use the same relay address. The host opens a session to get a token; the other peer connects with that token.</p>
          <div className="creator-row" style={{ marginBottom: 8 }}>
            <Input value={host} onChange={(e) => setHost(e.target.value)} placeholder="relay.example.com:9000" />
            <Button variant="ghost" icon="satellite" onClick={openHost} disabled={busy}>Host</Button>
          </div>
          <div className="creator-row">
            <Input value={token} onChange={(e) => setToken(e.target.value)} placeholder="connection token"
              onKeyDown={(e) => e.key === "Enter" && pair()} />
            <Button icon="swap" onClick={pair} disabled={busy}>Connect</Button>
          </div>
          {hostToken && (
            <div style={{ marginTop: 12 }}>
              <p className="creator-lead" style={{ marginBottom: 6 }}><strong>Share this token</strong> alongside <code>{host.trim()}</code>:</p>
              <Copyable value={hostToken} />
            </div>
          )}
        </section>

        <section className="rl-block">
          <div className="rl-block-head">
            <h3>Saved relays</h3>
          </div>
          <div className="rl-empty">
            <Icon name="globe" size={18} />
            <span>Saved relays and live routes aren't tracked yet — pair above to start a relayed session.</span>
          </div>
        </section>

        <section className="rl-block">
          <div className="rl-block-head">
            <h3>Run your own relay</h3>
          </div>
          <p className="creator-lead" style={{ marginBottom: 8 }}>Host a relay on a machine both peers can reach (a VPS, or a LAN box). It stores nothing.</p>
          <Copyable value={LOCAL_RELAY_CMD} />
        </section>
      </div>
    </div>
  );
}
