// New-connection creator — three clearly separate paths: dial a peer (Connect),
// wait for a peer (Host), or share/import an invite link (Invite).
import { useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Modal, Button, Input, PasswordInput } from "./ui.jsx";
import { api, friendlyError } from "../lib/bridge.js";
import { parsePort } from "../lib/parse.js";
import { toast } from "../lib/toast.js";

const MODES = [
  { id: "connect", icon: "send", label: "Dial" },
  { id: "host", icon: "radio", label: "Host" },
  { id: "invite", icon: "link", label: "Invite link" },
];

function CopyLine({ value }) {
  const [done, setDone] = useState(false);
  // writeText is async: await it, or "Copied ✓" shows when nothing was copied.
  async function copy() {
    if (!value) return;
    try {
      await navigator.clipboard?.writeText(value);
      setDone(true);
      setTimeout(() => setDone(false), 1200);
    } catch {
      toast("Could not copy — select the text and copy it manually.", "error");
    }
  }
  return (
    <div className="copy-line">
      <code>{value || "…"}</code>
      <button className="icon-btn" title={done ? "Copied" : "Copy"} aria-label={done ? "Copied" : "Copy"}
        onClick={copy}><Icon name={done ? "check" : "copy"} size={15} /></button>
    </div>
  );
}


const DEFAULT_PORT = 12345;

const TRANSPORTS = [
  { id: "direct", icon: "plug", label: "Direct" },
  { id: "relay", icon: "satellite", label: "Via relay" },
];

export function Creator({ open, onClose, initialMode, initialRelay = false }) {
  const [mode, setMode] = useState(initialMode || "connect");
  const [transport, setTransport] = useState("direct");
  const [addr, setAddr] = useState("");
  const [port, setPort] = useState("12345");
  const [relay, setRelay] = useState("");
  const [relayToken, setRelayToken] = useState("");
  const [hostToken, setHostToken] = useState("");
  const [myLink, setMyLink] = useState("");
  const [importLink, setImportLink] = useState("");
  const [password, setPassword] = useState("");
  const [hostAddr, setHostAddr] = useState(null);
  const [nearby, setNearby] = useState(null);
  const [err, setErr] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);

  // Open on the tab the caller asked for (the get-started panel's suggestions).
  useEffect(() => {
    if (open && initialMode) setMode(initialMode);
    if (open) setTransport(initialRelay ? "relay" : "direct");
  }, [open, initialMode, initialRelay]);

  // Clear per-attempt state on close: this stays mounted, and a connection
  // password meant for host A was once silently sent to host B.
  useEffect(() => {
    if (open) return;
    setPassword("");
    setErr("");
    setNote("");
    setAddr("");
    setRelayToken("");
    setHostToken("");
    setImportLink("");
    setHostAddr(null);
    // The link names this device's addresses and display name, which change;
    // cached for the whole session it went stale (UI-07). Fetched again on
    // the next open.
    setMyLink("");
  }, [open]);

  useEffect(() => {
    if (open && mode === "invite" && !myLink) {
      api.myInviteLink().then(setMyLink).catch(() => setMyLink(""));
    }
  }, [open, mode, myLink]);

  // The UPnP external address can resolve up to ~15s after hosting starts;
  // keep refreshing the shown addresses while the host pane is open.
  const hosting = hostAddr !== null;
  useEffect(() => {
    if (!open || !hosting) return;
    const t = setInterval(async () => {
      try { setHostAddr(await api.myAddresses()); } catch { /* keep last */ }
    }, 3000);
    return () => clearInterval(t);
  }, [open, hosting]);

  // Nearby peers (mDNS) for the direct-connect pane, refreshed while it is
  // open. Hidden entirely when the setting is off (nearby === null).
  useEffect(() => {
    if (!open || mode !== "connect" || transport !== "direct") return;
    let live = true;
    const tick = async () => {
      try {
        const r = await api.listDiscoveredPeers();
        if (live) setNearby(r.enabled ? r.peers : null);
      } catch { if (live) setNearby(null); }
    };
    tick();
    const t = setInterval(tick, 2000);
    return () => { live = false; clearInterval(t); };
  }, [open, mode, transport]);

  function reset() { setErr(""); setNote(""); setHostToken(""); setHostAddr(null); }
  function done(msg) { if (msg) { setNote(msg); } else onClose(); }

  // In-flight guard: each call creates a session, so a double submit would
  // spawn duplicate conversations or overlapping hosts.
  async function run(fn) {
    if (busy) return;
    setBusy(true);
    try { await fn(); } finally { setBusy(false); }
  }

  async function connect() {
    reset();
    const raw = addr.trim();
    if (!raw) return setErr("Enter a peer address.");
    const i = raw.lastIndexOf(":");
    const h = i > -1 ? raw.slice(0, i) : raw;
    // A bad port is an error, never a silent fall back to the default.
    let p = DEFAULT_PORT;
    if (i > -1) {
      p = parsePort(raw.slice(i + 1));
      if (p === null) return setErr(`"${raw.slice(i + 1)}" is not a valid port (1–65535).`);
    }
    await run(async () => {
      try { await api.connectPeer(h, p, password.trim()); toast(`Connecting to ${h}:${p}…`); onClose(); } catch (e) { setErr(friendlyError(e)); }
    });
  }
  async function host() {
    reset();
    const p = parsePort(port);
    if (p === null) return setErr(`"${port.trim()}" is not a valid port (1–65535).`);
    await run(async () => {
      try {
        await api.startHost(p, password.trim());
        toast(`Hosting on :${p} — waiting for a peer`, "success");
        // Keep the dialog open and show the address to share.
        try { setHostAddr(await api.myAddresses()); } catch { onClose(); }
      } catch (e) { setErr(friendlyError(e)); }
    });
  }
  async function relayConnect() {
    reset();
    if (!relay.trim()) return setErr("Enter the relay address.");
    if (!relayToken.trim()) return setErr("Enter the connection token your peer shared.");
    await run(async () => {
      try { await api.connectViaRelay(relay.trim(), relayToken.trim()); toast(`Connecting via relay ${relay.trim()}…`); onClose(); }
      catch (e) { setErr(friendlyError(e)); }
    });
  }
  async function relayHost() {
    reset();
    if (!relay.trim()) return setErr("Enter the relay address to broker through.");
    await run(async () => {
      try {
        const token = await api.hostViaRelay(relay.trim());
        setHostToken(token);
        toast("Relay session open — share the relay + token", "success");
      } catch (e) { setErr(friendlyError(e)); }
    });
  }
  async function doImport() {
    reset();
    if (!importLink.trim()) return setErr("Paste an invite link.");
    await run(async () => {
      try {
        // `import_invite` answers { contact, signed }, not the contact itself.
        const res = await api.importInvite(importLink.trim());
        const c = res?.contact ?? res;
        setImportLink("");
        // An unsigned (v1) link proves nothing about its maker (as in Contacts).
        if (res && res.signed === false) {
          toast(
            "Added from an unsigned invite link — anyone can create one. Compare the safety code when you first connect.",
            "error",
          );
        }
        try { await api.connectContact(c.id); onClose(); }
        catch { done(`Saved ${c.name || "the contact"} to contacts.`); }
      } catch (e) { setErr(friendlyError(e)); }
    });
  }

  // How the bytes travel, chosen up front for dialling and hosting: a direct
  // connection, or through a relay when neither side can accept one.
  const viaRelay = transport === "relay";
  const transportPicker = (mode === "connect" || mode === "host") && (
    <div className="seg seg-sub" role="radiogroup" aria-label="How to connect">
      {TRANSPORTS.map((t) => (
        <button key={t.id} role="radio" aria-checked={transport === t.id}
          className={cx("seg-btn", transport === t.id && "is-active")}
          onClick={() => { setTransport(t.id); reset(); }}>
          <Icon name={t.icon} size={14} />{t.label}
        </button>
      ))}
    </div>
  );
  let primary = null;
  if (mode === "connect" && !viaRelay) primary = <Button icon="send" onClick={connect} disabled={busy}>Connect</Button>;
  if (mode === "connect" && viaRelay) primary = <Button icon="satellite" onClick={relayConnect} disabled={busy}>Connect</Button>;
  if (mode === "host" && !viaRelay) primary = <Button icon="radio" onClick={host} disabled={busy}>Start hosting</Button>;
  if (mode === "host" && viaRelay) primary = <Button icon="satellite" onClick={relayHost} disabled={busy}>Open relay session</Button>;
  if (mode === "invite") primary = <Button icon="userPlus" onClick={doImport} disabled={busy}>Add</Button>;

  return (
    <Modal open={open} onClose={onClose} width={480} title="New connection" icon="plus"
      sub="Start an end-to-end encrypted conversation">
      <div className="seg" role="group" aria-label="What to do">
        {MODES.map((m) => (
          <button key={m.id} aria-pressed={mode === m.id} className={cx("seg-btn", mode === m.id && "is-active")}
            onClick={() => { setMode(m.id); reset(); }}>
            <Icon name={m.icon} size={15} />{m.label}
          </button>
        ))}
      </div>
      {transportPicker}

      {mode === "connect" && !viaRelay && (
        <div className="creator-pane">
          <p className="creator-lead">Dial someone who is already hosting. You'll compare a safety code before anything is trusted.</p>
          {nearby && nearby.length > 0 && (
            <div className="creator-nearby">
              <span className="creator-nearby-h"><Icon name="search" size={13} /> Nearby</span>
              {nearby.map((p) => (
                <button key={`${p.address}:${p.port}`} className="creator-nearby-row"
                  onClick={() => setAddr(`${p.address}:${p.port}`)}>
                  <span>{p.name}</span>
                  <code>{p.address}:{p.port}</code>
                </button>
              ))}
            </div>
          )}
          <div className="field">
            <label className="field-label" htmlFor="cr-addr">Their address</label>
            <Input id="cr-addr" value={addr} autoFocus placeholder="192.168.1.20:12345"
              onChange={(e) => setAddr(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && connect()} />
          </div>
          <div className="field">
            <label className="field-label" htmlFor="cr-pass">Connection password <span className="field-opt">if they set one</span></label>
            <PasswordInput id="cr-pass" value={password} autoComplete="off"
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && connect()} />
          </div>
        </div>
      )}

      {mode === "connect" && viaRelay && (
        <div className="creator-pane">
          <p className="creator-lead">Use the relay and token the other person shared with you. The relay can't read your messages, but it sees both IP addresses and when you talk.</p>
          <div className="field">
            <label className="field-label" htmlFor="cr-relay">Relay address</label>
            <Input id="cr-relay" value={relay} autoFocus placeholder="relay.example.com:9000"
              onChange={(e) => setRelay(e.target.value)} />
          </div>
          <div className="field">
            <label className="field-label" htmlFor="cr-token">Token from your peer</label>
            <Input id="cr-token" value={relayToken} placeholder="Connection token from your peer"
              onChange={(e) => setRelayToken(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && relayConnect()} />
          </div>
        </div>
      )}

      {mode === "host" && !viaRelay && (
        <div className="creator-pane">
          <p className="creator-lead">Wait for someone to dial you. Share your address or invite link, and keep the app open.</p>
          <div className="field-grid is-port">
            <div className="field">
              <label className="field-label" htmlFor="cr-port">Port</label>
              <Input id="cr-port" value={port} onChange={(e) => setPort(e.target.value)} inputMode="numeric" />
            </div>
            <div className="field">
              <label className="field-label" htmlFor="cr-hpass">Require a password <span className="field-opt">optional</span></label>
              <PasswordInput id="cr-hpass" value={password} autoComplete="new-password"
                onChange={(e) => setPassword(e.target.value)} />
            </div>
          </div>
          {hostAddr && (
            <div className="field">
              <span className="field-label">Share this address</span>
              <CopyLine value={hostAddr.local || "address unavailable — share an invite link instead"} />
              {hostAddr.external && (
                <>
                  <span className="field-hint">Reachable from the internet (UPnP):</span>
                  <CopyLine value={hostAddr.external} />
                </>
              )}
            </div>
          )}
        </div>
      )}

      {mode === "host" && viaRelay && (
        <div className="creator-pane">
          <p className="creator-lead">No port forwarding needed: the relay brokers the connection and gives you a token to share. It can't read your messages, but it sees both IP addresses and when you talk.</p>
          <div className="field">
            <label className="field-label" htmlFor="cr-hrelay">Relay address</label>
            <Input id="cr-hrelay" value={relay} autoFocus placeholder="relay.example.com:9000"
              onChange={(e) => setRelay(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && relayHost()} />
          </div>
          {hostToken && (
            <div className="field">
              <span className="field-label">Share this token, along with <code>{relay.trim()}</code></span>
              <CopyLine value={hostToken} />
            </div>
          )}
        </div>
      )}

      {mode === "invite" && (
        <div className="creator-pane">
          <div className="field">
            <span className="field-label">Your invite link</span>
            <CopyLine value={myLink} />
            <span className="field-hint">Send it however you already talk. It carries your address and key — they'll still compare a safety code with you.</span>
          </div>
          <div className="field">
            <label className="field-label" htmlFor="cr-import">Have their link? Paste it</label>
            <Input id="cr-import" value={importLink} placeholder="chat-p2p://invite/…"
              onChange={(e) => setImportLink(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && doImport()} />
          </div>
        </div>
      )}

      {err && <div className="onb-err" style={{ marginTop: 12 }}><Icon name="alert" size={13} /> {err}</div>}
      {note && <div className="creator-lead" style={{ marginTop: 12, color: "var(--success)" }}>{note}</div>}

      <div className="modal-foot">
        <span />
        <div className="modal-foot-actions">{primary}</div>
      </div>
    </Modal>
  );
}
