import { useMemo, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Avatar, Button, Input, cx } from "./ui.jsx";

const SERVERS = [
  {
    id: "class",
    name: "Classroom Party",
    host: "192.168.1.42:12345",
    tier: "Administered",
    status: "online",
    channel: "general",
    fingerprint: "8A21 C4E7 019F B642 77D9 2E13 4CFA 91B0",
    members: [
      { name: "Maya", state: "connected", role: "host" },
      { name: "Alice", state: "connected", role: "member" },
      { name: "Nora", state: "offline", role: "member" },
    ],
    messages: [
      { from: "Alice", text: "I can see the channel history after reconnecting.", t: "21:44" },
      { from: "Maya", text: "Good. Next pass is server fingerprint confirmation in this view.", t: "21:46" },
      { from: "Nora", text: "Leaving my laptop offline for the catch-up test.", t: "21:47" },
    ],
  },
  {
    id: "lab",
    name: "Lab Night",
    host: "10.0.0.18:12345",
    tier: "Administered",
    status: "offline",
    channel: "setup",
    fingerprint: "12FF 8830 A91C 0E44 B2F7 600A 19DD 42C1",
    members: [
      { name: "Maya", state: "offline", role: "member" },
      { name: "Bob", state: "offline", role: "host" },
    ],
    messages: [
      { from: "Bob", text: "Server will be back online after the router test.", t: "19:12" },
    ],
  },
];

export function Parties() {
  const [activeId, setActiveId] = useState(SERVERS[0].id);
  const [address, setAddress] = useState("");
  const [username, setUsername] = useState("Maya");
  const active = useMemo(
    () => SERVERS.find((server) => server.id === activeId) || SERVERS[0],
    [activeId],
  );

  return (
    <div className="party-pane">
      <aside className="party-sidebar">
        <div className="party-side-head">
          <h2>Parties</h2>
          <button className="conv-add" title="Join server">
            <Icon name="plus" size={16} />
          </button>
        </div>
        <div className="party-join">
          <Input value={address} placeholder="server address" onChange={(e) => setAddress(e.target.value)} />
          <Input value={username} placeholder="username" onChange={(e) => setUsername(e.target.value)} />
          <Button icon="server" full disabled={!address.trim() || !username.trim()}>Join</Button>
        </div>
        <div className="party-side-list">
          {SERVERS.map((server) => (
            <button
              key={server.id}
              className={cx("party-side-row", server.id === active.id && "is-active")}
              onClick={() => setActiveId(server.id)}
            >
              <Avatar name={server.name} size={38} state={server.status === "online" ? "connected" : "offline"} party />
              <span className="party-side-main">
                <span className="party-side-name">{server.name}</span>
                <span className="party-side-sub">
                  <span className={cx("rl-status-dot", server.status === "online" ? "is-online" : "is-offline")} />
                  {server.channel}
                  <span className="ps-guest">{server.members.length} members</span>
                </span>
              </span>
            </button>
          ))}
        </div>
      </aside>

      <main className="party-detail">
        <header className="pd-head">
          <Avatar name={active.name} size={44} state={active.status === "online" ? "connected" : "offline"} party />
          <div className="pd-head-info">
            <div className="pd-title">
              {active.name}
              <span className="role-tag role-host">{active.tier}</span>
            </div>
            <div className="pd-sub">
              <span>{active.host}</span>
              <span className="rl-sep">•</span>
              <span>{active.channel}</span>
              <span className="rl-sep">•</span>
              <span>{active.status}</span>
            </div>
          </div>
          <div className="pd-head-actions">
            <Button variant="ghost" size="sm" icon="fingerprint">Verify</Button>
            <Button size="sm" icon="refresh">Sync</Button>
          </div>
        </header>

        <div className="party-work">
          <section className="party-channel">
            <div className="pd-block-head">
              <h3>#{active.channel}</h3>
              <span className="pd-block-note">{active.messages.length} messages</span>
            </div>
            <div className="party-messages">
              {active.messages.map((message, idx) => (
                <div key={`${message.from}-${idx}`} className="party-msg">
                  <Avatar name={message.from} size={30} />
                  <div className="party-msg-main">
                    <div className="party-msg-meta">
                      <strong>{message.from}</strong>
                      <span>{message.t}</span>
                    </div>
                    <div className="party-msg-text">{message.text}</div>
                  </div>
                </div>
              ))}
            </div>
            <div className="party-composer">
              <Input placeholder={`Message #${active.channel}`} />
              <button className="composer-send is-ready" title="Send"><Icon name="send" size={17} /></button>
            </div>
          </section>

          <aside className="party-inspector">
            <section className="pd-block">
              <div className="pd-block-head">
                <h3>Fingerprint</h3>
              </div>
              <div className="pd-token">
                <code>{active.fingerprint}</code>
                <div className="pd-token-meta">
                  <Icon name="shieldCheck" size={14} />
                  TOFU pending
                </div>
              </div>
            </section>
            <section className="pd-block">
              <div className="pd-block-head">
                <h3>Members</h3>
              </div>
              <div className="pd-members">
                {active.members.map((member) => (
                  <div className="pm-row" key={member.name}>
                    <Avatar name={member.name} size={32} state={member.state} />
                    <div className="pm-main">
                      <div className="pm-line1">
                        <span className="pm-name">{member.name}</span>
                        <span className={cx("role-tag", member.role === "host" ? "role-host" : "role-member")}>{member.role}</span>
                      </div>
                    </div>
                    <span className="pm-state">
                      <span className={cx("state-dot", `state-${member.state}`)} />
                      {member.state === "connected" ? "online" : "offline"}
                    </span>
                  </div>
                ))}
              </div>
            </section>
          </aside>
        </div>
      </main>
    </div>
  );
}
