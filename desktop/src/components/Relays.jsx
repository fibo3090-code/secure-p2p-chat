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
import { Button, Input, Page } from "./ui.jsx";
import { api, friendlyError } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";

// The crate that actually provides `--relay-server` (see client/src/main.rs).
// This is a copy-paste command: a wrong crate name here is a user typing it into
// a terminal and being told the package does not exist.
const LOCAL_RELAY_CMD = "cargo run --release -p p2pem-classic -- --relay-server --port 9000";

function Copyable({ value }) {
  const [done, setDone] = useState(false);
  // writeText returns a promise, so a sync try/catch never sees a rejection:
  // "Copied ✓" appeared even when the clipboard was blocked and nothing was
  // copied. Await it, and say so when it fails.
  async function copy() {
    try {
      await navigator.clipboard?.writeText(value);
      setDone(true);
      setTimeout(() => setDone(false), 1200);
    } catch {
      toast("Could not copy — select the text and copy it manually.", "error");
    }
  }
  return (
    <div className="rl-cmd">
      <code>{value}</code>
      <button className="copy-btn" title={done ? "Copied" : "Copy"} aria-label={done ? "Copied" : "Copy"}
        onClick={copy}><Icon name={done ? "check" : "copy"} size={15} /></button>
    </div>
  );
}

export function Relays({ onConnected }) {
  const [host, setHost] = useState("");
  const [token, setToken] = useState("");
  const [hostToken, setHostToken] = useState("");
  const [busy, setBusy] = useState(false);

  async function pair() {
    if (busy) return;
    if (!host.trim()) return toast("Enter the relay address.", "error");
    if (!token.trim()) return toast("Enter the connection token your peer shared.", "error");
    setBusy(true);
    try {
      await api.connectViaRelay(host.trim(), token.trim());
      toast(`Connecting via relay ${host.trim()}…`, "success");
      onConnected && onConnected();
    } catch (e) { toast(friendlyError(e), "error"); }
    finally { setBusy(false); }
  }

  async function openHost() {
    if (busy) return;
    if (!host.trim()) return toast("Enter the relay address to broker through.", "error");
    setBusy(true);
    setHostToken("");
    try {
      const t = await api.hostViaRelay(host.trim());
      setHostToken(t);
      toast("Relay session open — share the token with your peer", "success");
    } catch (e) { toast(friendlyError(e), "error"); }
    finally { setBusy(false); }
  }

  return (
    <Page title="Relays"
      sub="Reach someone when neither of you can accept a direct connection — no port forwarding needed.">
      <div className="callout is-ok">
        <Icon name="lock" size={16} />
        <p>
          A relay is a blind broker: it forwards bytes only. Your conversation stays
          end-to-end encrypted and fingerprint-verified, and the relay never sees
          what you say.
        </p>
      </div>

      <section className="card">
        <h2 className="card-title">Pair through a relay</h2>
        <p className="card-lead">
          Both of you use the same relay address. One person opens a session and gets a
          token; the other connects with that token.
        </p>
        <div className="field">
          <label className="field-label" htmlFor="relay-host">Relay address</label>
          <div className="creator-row">
            <Input id="relay-host" mono value={host} onChange={(e) => setHost(e.target.value)} placeholder="relay.example.com:9000" />
            <Button variant="ghost" icon="satellite" onClick={openHost} disabled={busy}>Host</Button>
          </div>
          <p className="field-hint">Hosting opens a session on this relay and gives you a token to share.</p>
        </div>
        {hostToken && (
          <div className="field">
            <span className="field-label">Share this token with your peer, along with <code>{host.trim()}</code></span>
            <Copyable value={hostToken} />
          </div>
        )}
        <div className="field">
          <label className="field-label" htmlFor="relay-token">Token from your peer</label>
          <div className="creator-row">
            <Input id="relay-token" value={token} onChange={(e) => setToken(e.target.value)} placeholder="connection token"
              onKeyDown={(e) => e.key === "Enter" && pair()} />
            <Button icon="swap" onClick={pair} disabled={busy}>Connect</Button>
          </div>
        </div>
      </section>

      <section className="card">
        <h2 className="card-title">Run your own relay</h2>
        <p className="card-lead">
          Host one on a machine you both can reach — a small VPS or a box on your network.
          It stores nothing.
        </p>
        <Copyable value={LOCAL_RELAY_CMD} />
      </section>
    </Page>
  );
}
