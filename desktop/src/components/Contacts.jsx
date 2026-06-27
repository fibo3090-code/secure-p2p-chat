// Contacts directory — saved peers (from invite links), with connect + import.
import { useCallback, useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar, Button, IconButton, Input, TrustBadge } from "./ui.jsx";
import { api } from "../lib/bridge.js";

export function Contacts({ onConnected }) {
  const [contacts, setContacts] = useState([]);
  const [link, setLink] = useState("");
  const [err, setErr] = useState("");

  const load = useCallback(() => {
    api.listContacts().then(setContacts).catch(() => {});
  }, []);
  useEffect(() => { load(); }, [load]);

  async function importLink() {
    setErr("");
    if (!link.trim()) return;
    try { await api.importInvite(link.trim()); setLink(""); load(); }
    catch (e) { setErr(String(e)); }
  }
  async function connect(id) {
    setErr("");
    try { await api.connectContact(id); onConnected && onConnected(); }
    catch (e) { setErr(String(e)); }
  }

  return (
    <div className="chat-pane">
      <header className="chat-head">
        <div className="chat-head-l">
          <div className="chat-head-info">
            <div className="chat-head-name">Contacts</div>
            <div className="chat-head-sub mono">{contacts.length} saved</div>
          </div>
        </div>
      </header>

      <div className="contacts-pane">
        <div className="creator-row contact-import">
          <Input value={link} placeholder="Paste an invite link to add a contact…"
            onChange={(e) => setLink(e.target.value)} onKeyDown={(e) => e.key === "Enter" && importLink()} />
          <Button icon="plus" onClick={importLink}>Add</Button>
        </div>
        {err && <div className="onb-err" style={{ marginTop: 12 }}><Icon name="alert" size={13} /> {err}</div>}

        <div className="contacts-grid contact-grid-tight">
          {contacts.length === 0 && (
            <div className="conv-empty">No contacts yet — add one with an invite link.</div>
          )}
          {contacts.map((c) => (
            <div key={c.id} className="contact-card">
              <div className="contact-card-top">
                <Avatar name={c.name} size={42} />
                <div className="contact-card-id">
                  <div className="contact-card-name">{c.name}</div>
                  <span className="contact-card-addr">{c.address || "no saved address"}</span>
                </div>
                <TrustBadge trust={c.trust} mini />
              </div>
              <div className="contact-card-fp">
                <span className="vf-grid-label">Fingerprint</span>
                <code>{c.fingerprint || "not verified yet"}</code>
              </div>
              <div className="contact-card-foot">
                <span className="contact-state">
                  <span className={cx("state-dot", c.address ? "state-offline" : "state-connecting")} />
                  {c.address ? "ready" : "missing address"}
                </span>
                <div className="contact-card-actions">
                  <IconButton name="send" label="Connect" onClick={() => connect(c.id)} disabled={!c.address} />
                  <IconButton name="copy" label="Copy fingerprint" onClick={() => navigator.clipboard?.writeText(c.fingerprint || "")} />
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
