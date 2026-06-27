import { useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Button, Input, cx } from "./ui.jsx";

const RELAYS = [
  { host: "relay.home:12345", status: "online", latency: "18 ms", active: 2 },
  { host: "192.168.1.42:12346", status: "offline", latency: "-", active: 0 },
];

const ROUTES = [
  { peer: "Alice", direction: "out", via: "relay.home:12345", since: "12 min", state: "active" },
  { peer: "Bob", direction: "in", via: "relay.home:12345", since: "idle", state: "idle" },
];

export function Relays() {
  const [host, setHost] = useState("relay.home:12345");
  const [token, setToken] = useState("classroom");

  return (
    <div className="relay-pane">
      <div className="rl-scroll">
        <header className="rl-head">
          <div>
            <h2>Relays</h2>
            <p>Self-hosted rendezvous for peers that cannot dial each other directly.</p>
          </div>
          <Button icon="refresh">Refresh</Button>
        </header>

        <section className="rl-hero">
          <div className="rl-hero-l">
            <span className="rl-hero-ic"><Icon name="globe" size={24} /></span>
            <div>
              <div className="rl-hero-host">{host}</div>
              <div className="rl-hero-tags">
                <span className="rl-status is-online"><span className="rl-status-dot" />online</span>
                <span className="rl-sep">•</span>
                <span className="rl-selfhost"><Icon name="server" size={13} />self-hosted</span>
              </div>
            </div>
          </div>
          <div className="rl-hero-stats">
            <div className="rl-stat">
              <div className="rl-stat-v">2</div>
              <div className="rl-stat-l">routes</div>
            </div>
            <div className="rl-stat">
              <div className="rl-stat-v">18 ms</div>
              <div className="rl-stat-l">latency</div>
            </div>
          </div>
        </section>

        <section className="rl-connect">
          <div className="rl-block-head">
            <h3>Pair through relay</h3>
          </div>
          <div className="rl-connect-row">
            <Input value={host} onChange={(e) => setHost(e.target.value)} placeholder="relay host" />
            <Input value={token} onChange={(e) => setToken(e.target.value)} placeholder="token" />
            <Button icon="swap">Pair</Button>
          </div>
        </section>

        <section className="rl-block">
          <div className="rl-block-head">
            <h3>Known relays</h3>
            <span className="pd-block-note">{RELAYS.length} configured</span>
          </div>
          <div className="rl-servers">
            {RELAYS.map((relay) => (
              <div key={relay.host} className={cx("rl-server", relay.status !== "online" && "is-off")}>
                <span className={cx("rl-server-dot", relay.status === "online" ? "is-online" : "is-offline")} />
                <div className="rl-server-main">
                  <div className="rl-server-host">{relay.host}</div>
                  <div className="rl-server-tags">
                    <span>{relay.status}</span>
                    <span>{relay.active} active</span>
                  </div>
                </div>
                <span className="rl-server-lat">{relay.latency}</span>
                <div className="rl-server-actions">
                  <button className="cc-act" title="Use relay"><Icon name="check" size={15} /></button>
                  <button className="cc-act danger" title="Remove"><Icon name="trash" size={15} /></button>
                </div>
              </div>
            ))}
          </div>
        </section>

        <section className="rl-block">
          <div className="rl-block-head">
            <h3>Active routes</h3>
          </div>
          <div className="rl-routes">
            <div className="rl-route rl-route-head">
              <span>Peer</span><span>Direction</span><span>Relay</span><span>Since</span><span></span>
            </div>
            {ROUTES.map((route) => (
              <div className="rl-route" key={route.peer}>
                <span className="rl-r-peer"><span className={cx("rl-r-state", `is-${route.state}`)} />{route.peer}</span>
                <span className={cx("rl-r-dir", `dir-${route.direction}`)}>{route.direction}</span>
                <span className="rl-r-via">{route.via}</span>
                <span className="rl-r-since">{route.since}</span>
                <span className="rl-r-right"><button className="cc-act" title="Reconnect"><Icon name="refresh" size={15} /></button></span>
              </div>
            ))}
          </div>
        </section>

        <section className="rl-block">
          <div className="rl-block-head">
            <h3>Run locally</h3>
          </div>
          <div className="rl-cmd">
            <code>cargo run -p encodeur_rsa_rust -- --relay-server</code>
            <button className="copy-btn" title="Copy"><Icon name="copy" size={15} /></button>
          </div>
        </section>
      </div>
    </div>
  );
}
