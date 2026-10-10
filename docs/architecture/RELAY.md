# Connect by ID: rendezvous and relay

For computers behind NAT or a firewall. Crate `nexdesk-network`, program `nexdesk-relay`. **Prototype.** All traffic goes through the relay; there is no hole punching yet.

## How it works
1. The agent opens one control connection to the relay and registers a nine digit ID (kept in `~/.config/nexdesk/peer/agent-id`, so it stays the same).
2. A viewer connects to the relay and asks for the ID. The relay tells the agent, the agent opens a second connection, and the relay copies bytes between the two connections.
3. Viewer and agent then run the normal `nexdesk-crypto` handshake **through** the relay. The relay carries ciphertext only. A malicious or hacked relay can refuse service or cut connections; it cannot read the screen, keystrokes or clipboard, change them, or pose as the agent.

The ID is a convenience, not a secret and not proof of identity. Real identity is the agent's key fingerprint, pinned by the viewer on first use, exactly as for direct connections. The first connection to an ID shows the fingerprint to compare out of band; a different identity later is refused. The agent still asks its user before sharing anything.

## Run a relay
```bash
cargo build --release -p nexdesk-network
nexdesk-relay --listen 0.0.0.0:21117      # open TCP 21117 in the firewall
```
Agent: `nexdesk-agent --relay relay.example.com:21117` (prints `this computer's ID: 123456789`).
Viewer: `nexdesk-peer-view --relay relay.example.com:21117 123456789`.
In the manager: Remote Control, type the relay server in the box, press Start sharing; the ID is shown. To connect, type the nine digit ID instead of an address.

**Which address goes in the manager's relay box:** the real address of the computer that runs `nexdesk-relay` (for example `192.168.1.20:21117`), the same on every computer. `0.0.0.0:21117` is only what the relay *listens* on; as a destination it means "this computer", so it never reaches a relay elsewhere (the manager now refuses it). On one local network you do not need a relay: connect to the other computer's address instead.

**ID buttons (Remote Control page):** *Copy ID* puts the nine digits on the clipboard; *New ID* replaces the saved ID and restarts sharing. The fingerprint does not change, so people who already pinned this computer are not warned; they only need the new ID.

## Access key (who may use your relay)
By default a relay serves anyone who can reach it. Give it a key and only people who know the key can register an agent or connect a viewer:
```bash
head -c 24 /dev/urandom | base64 > relay.key          # at least 16 characters
nexdesk-relay --key-file relay.key                    # or NEXDESK_RELAY_KEY=... in the environment
NEXDESK_RELAY_KEY="$(cat relay.key)" nexdesk-agent --relay relay.example.com:21117
```
How it works: after the first frame the relay sends a random 16 byte challenge; the client answers with HMAC-SHA256(key, challenge) and the relay compares in constant time. The key never crosses the network and an answer cannot be replayed (every connection gets a new challenge). A client without the right key is refused before the relay looks anything up, so it cannot probe which IDs exist. A client that has a key also works against an open relay.
This controls **who may use the relay**; it is not end-to-end identity (that stays the agent's pinned fingerprint) and it is not per-user: everyone shares one key, so rotate it by restarting the relay with a new key and updating clients. Per-user tokens come with the enterprise server tier. The relay speaks plain TCP, so a network observer can still see metadata (addresses, IDs).

Docker: `packaging/docker/Dockerfile.relay` (distroless, runs as a non-root user).

## What the relay knows and limits
* Sees: client addresses, IDs, when and how much traffic flows. Logs addresses and IDs only, never content.
* An ID cannot be taken over while its owner is registered. If the owner reappears after a crash, the relay frees the ID within 45 s (heartbeat silence).
* Limits (flags `--max-connections`, `--per-ip`): 2000 connections, 30 per address, 2 viewers waiting per agent, 15 s to pair, bridges idle for 120 s are closed. Control frames are at most 64 bytes.
* One thread per connection: fine for a team or a lab, not for thousands of agents. An async relay is a later step if needed.
* Put the relay behind TLS-terminating infrastructure only if you must; it is not needed for confidentiality, because the payload is already end-to-end encrypted. Metadata is visible to whoever runs the relay.

## Not done yet
Hole punching (direct path when possible), per-user tokens or an allow-list of IDs, rate limiting beyond connection counts, several relays and failover, ID rotation, an async server.

## Tests
`cargo test -p nexdesk-network` (bytes flow both ways, 3 MB intact, duplicate ID refused, unknown token refused, access key (wrong, missing, replayed), garbage input, per-address limit, ID freed after leaving) and `xvfb-run -a cargo test -p nexdesk-peer --test relayed` (a viewer controls an agent by ID through an in-process relay, wrong pin aborts).
