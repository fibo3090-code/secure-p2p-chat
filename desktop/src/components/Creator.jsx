// New-connection creator — three clearly separate paths: dial a peer (Connect),
// wait for a peer (Host), or share/import an invite link (Invite).
import { useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Modal, Button, Input, PasswordInput } from "./ui.jsx";
import { api, friendlyError } from "../lib/bridge.js";
import { parsePort } from "../lib/parse.js";
import { toast } from "../lib/toast.js";

const MODES = [
  { id: "connect", icon: "send", label: "Connect" },
  { id: "host", icon: "server", label: "Host" },
  { id: "invite", icon: "user", label: "Invite" },
];

function CopyLine({ value }) {
  const [done, setDone] = useState(false);
  // writeText returns a promise, so a sync try/catch never sees a rejection and
  // "Copied ✓" appeared even when nothing reached the clipboard — the user then
  // pastes stale content into a chat and wonders why their peer can't connect.
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

const TRANSPORTS = [
  { id: "direct", icon: "plug", label: "Direct" },
  { id: "relay", icon: "satellite", label: "Via relay" },
];

const DEFAULT_PORT = 12345;

export function Creator({ open, onClose, initialMode }) {
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

  // Open on the tab the caller asked for. The get-started panel routes each of
  // its three suggestions straight to the matching path, so the user is never
  // dropped on a generic dialog and left to find the right tab themselves.
  useEffect(() => {
    if (open && initialMode) setMode(initialMode);
  }, [open, initialMode]);

  // Clear per-attempt state when the dialog closes. This component stays
  // mounted, so everything typed for one peer survived into the next open: a
  // connection password meant for host A was then silently sent to host B, and
  // a stale error sat above a fresh, unrelated form. Addresses and tokens are
  // cleared for the same reason — they belong to the attempt, not to the app.
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
  }, [open]);

  useEffect(() => {
    if (open && mode === "invite" && !myLink) {
      api.myInviteLink().then(setMyLink).catch(() => setMyLink(""));
    }
  }, [open, mode, myLink]);

  // The UPnP external address can resolve up to ~15s after hosting starts;
  // keep refreshing the shown addresses while the host pane is open.
  useEffect(() => {
    if (!open || !hostAddr) return;
    const t = setInterval(async () => {
      try { setHostAddr(await api.myAddresses()); } catch { /* keep last */ }
    }, 3000);
    return () => clearInterval(t);
  }, [open, hostAddr !== null]);

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

  // Single in-flight guard: each connect/host call creates a session in
  // ChatManager, so a double-submit (repeat click or Enter) would spawn
  // duplicate conversations / overlapping host attempts.
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
    // A typo'd port used to fall back to 12345 silently, so the app dialled a
    // different machine's port than the one on screen and reported a plain
    // "connection refused". Say what is wrong instead of guessing.
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
        // `import_invite` answers with { contact, signed }, not with the contact
        // itself. Reading `.id` off the wrapper meant `connect_contact` was
        // called with `undefined`, failed to parse as a UUID, and the dialog
        // reported "Saved undefined to contacts." — so importing a link from
        // here never actually connected.
        const res = await api.importInvite(importLink.trim());
        const c = res?.contact ?? res;
        setImportLink("");
        // An unsigned (v1) link carries no proof of who made it. Same warning
        // the Contacts pane gives, because it is the same decision.
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

  return (
    <Modal open={open} onClose={onClose} width={460} title="New connection" icon="plus"
      sub="Start an encrypted conversation">
      <div className="creator-seg">
        {MODES.map((m) => (
          <button key={m.id} className={mode === m.id ? "is-active" : ""}
            onClick={() => { setMode(m.id); reset(); }}>
            <Icon name={m.icon} size={18} />{m.label}
          </button>
        ))}
      </div>

      {(mode === "connect" || mode === "host") && (
        <div className="creator-seg creator-seg-sub">
          {TRANSPORTS.map((t) => (
            <button key={t.id} className={transport === t.id ? "is-active" : ""}
              onClick={() => { setTransport(t.id); reset(); }}>
              <Icon name={t.icon} size={15} />{t.label}
            </button>
          ))}
        </div>
      )}

      {mode === "connect" && transport === "direct" && (
        <div className="creator-pane">
          <p className="creator-lead"><strong>Dial a peer</strong> who is hosting. You'll verify their fingerprint before any message is trusted.</p>
          {nearby && nearby.length > 0 && (
            <div className="creator-nearby">
              <span className="creator-nearby-h"><Icon name="search" size={13} /> Nearby peers</span>
              {nearby.map((p) => (
                <button key={`${p.address}:${p.port}`} className="creator-nearby-row"
                  onClick={() => setAddr(`${p.address}:${p.port}`)}>
                  <span>{p.name}</span>
                  <code>{p.address}:{p.port}</code>
                </button>
              ))}
            </div>
          )}
          <div className="creator-row">
            <Input value={addr} autoFocus placeholder="192.168.1.20:12345"
              onChange={(e) => setAddr(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && connect()} />
            <Button icon="send" onClick={connect} disabled={busy}>Connect</Button>
          </div>
          <div className="creator-row">
            <PasswordInput value={password} placeholder="Connection password (if the host set one)"
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && connect()} />
          </div>
        </div>
      )}

      {mode === "connect" && transport === "relay" && (
        <div className="creator-pane">
          <p className="creator-lead"><strong>Dial through a relay.</strong> The relay only forwards bytes — your conversation stays end-to-end encrypted and you still verify the fingerprint.</p>
          <div className="creator-row">
            <Input value={relay} autoFocus placeholder="relay.example.com:9000"
              onChange={(e) => setRelay(e.target.value)} />
          </div>
          <div className="creator-row">
            <Input value={relayToken} placeholder="Connection token from your peer"
              onChange={(e) => setRelayToken(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && relayConnect()} />
            <Button icon="satellite" onClick={relayConnect} disabled={busy}>Connect</Button>
          </div>
        </div>
      )}

      {mode === "host" && transport === "direct" && (
        <div className="creator-pane">
          <p className="creator-lead"><strong>Wait for a peer</strong> to dial you. Share your address (or an invite link) and keep the app open.</p>
          <div className="creator-row">
            <Input value={port} onChange={(e) => setPort(e.target.value)} style={{ maxWidth: 130 }} />
            <Button variant="ghost" icon="server" onClick={host} disabled={busy}>Start hosting</Button>
          </div>
          <div className="creator-row">
            <PasswordInput value={password} placeholder="Require a connection password (optional)"
              onChange={(e) => setPassword(e.target.value)} />
          </div>
          {hostAddr && (
            <>
              <p className="creator-lead" style={{ marginTop: 10 }}><strong>Share this address</strong> with your peer:</p>
              <CopyLine value={hostAddr.local || "address unavailable — share an invite link instead"} />
              {hostAddr.external && (
                <>
                  <p className="creator-lead" style={{ marginTop: 6 }}>Reachable from the internet (UPnP):</p>
                  <CopyLine value={hostAddr.external} />
                </>
              )}
            </>
          )}
        </div>
      )}

      {mode === "host" && transport === "relay" && (
        <div className="creator-pane">
          <p className="creator-lead"><strong>Host through a relay</strong> — no port-forwarding needed. The relay brokers the connection; share the relay address and token below with your peer.</p>
          <div className="creator-row">
            <Input value={relay} autoFocus placeholder="relay.example.com:9000"
              onChange={(e) => setRelay(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && relayHost()} />
            <Button variant="ghost" icon="satellite" onClick={relayHost} disabled={busy}>Open relay</Button>
          </div>
          {hostToken && (
            <>
              <p className="creator-lead" style={{ marginTop: 10 }}><strong>Connection token</strong> — share with your peer alongside <code>{relay.trim()}</code>:</p>
              <CopyLine value={hostToken} />
            </>
          )}
        </div>
      )}

      {mode === "invite" && (
        <div className="creator-pane">
          <p className="creator-lead"><strong>Your invite link</strong> — send it to a friend; it carries your address, fingerprint and key.</p>
          <CopyLine value={myLink} />
          <p className="creator-lead" style={{ marginTop: 6 }}><strong>Have a link?</strong> Paste it to add the contact and connect.</p>
          <div className="creator-row">
            <Input value={importLink} placeholder="chat-p2p://invite/…"
              onChange={(e) => setImportLink(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && doImport()} />
            <Button icon="plus" onClick={doImport} disabled={busy}>Add</Button>
          </div>
        </div>
      )}

      {err && <div className="onb-err" style={{ marginTop: 14 }}><Icon name="alert" size={13} /> {err}</div>}
      {note && <div className="creator-lead" style={{ marginTop: 14, color: "var(--success)" }}>{note}</div>}
    </Modal>
  );
}
