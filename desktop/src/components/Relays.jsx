// Relays surface. A relay is a blind byte-broker that pairs two peers when neither
// can dial the other; a relayed conversation is still an end-to-end-encrypted,
// fingerprint-verified DM. Only actions the backend backs are offered; saved
// relays and live routes do not exist yet, so they show as explicit empty states.
import { useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Button, Page } from "./ui.jsx";
import { toast } from "../lib/toast.js";

// The crate that provides `--relay-server` (client/src/main.rs); users paste this.
const LOCAL_RELAY_CMD = "cargo run --release -p p2pem-classic -- --relay-server --port 9000";

function Copyable({ value }) {
  const [done, setDone] = useState(false);
  // writeText is async: await it, or "Copied ✓" shows when nothing was copied.
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

// Pairing through a relay lives in New connection ("Via relay"), next to
// every other way of starting a conversation; this page explains what a relay
// is and how to run one, and sends you there. Two forms doing the same job in
// two places was a choice the user should never have had to make.
export function Relays({ onPair }) {
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
        <h2 className="card-title">Talk through a relay</h2>
        <p className="card-lead">
          One of you opens a session on a relay and shares the token it gives; the other
          connects with that token. Both happen in New connection, with “Via relay” selected.
        </p>
        <div className="card-actions is-start">
          <Button icon="satellite" onClick={() => onPair && onPair("connect")}>Connect with a token</Button>
          <Button variant="ghost" icon="radio" onClick={() => onPair && onPair("host")}>Open a relay session</Button>
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
