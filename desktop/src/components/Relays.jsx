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
import { Button, Page } from "./ui.jsx";
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

      {/* A build command is for people running infrastructure, not for
          someone who installed the app: it waits behind a disclosure. */}
      <details className="card card-details">
        <summary className="card-title">Advanced: run your own relay</summary>
        <p className="card-lead">
          Host one on a machine you both can reach — a small VPS or a box on your network.
          It stores nothing. From a checkout of the source:
        </p>
        <Copyable value={LOCAL_RELAY_CMD} />
      </details>
    </Page>
  );
}
