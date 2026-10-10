# NexDesk native remote control

NexDesk is not only an RDP client. RDP needs a server on the target (Windows Pro/Server). The native protocol covers what RDP cannot:
Linux and macOS hosts, machines behind NAT, and Windows editions without an RDP server. RDP stays the way to reach Windows Server.

Status of every capability: `docs/product/FEATURE_MATRIX.md`. Phases: `docs/product/ROADMAP.md` (P5). What exists today: `docs/architecture/PEER.md`. Security design: `docs/security/SECURITY_DESIGN.md`, `docs/security/CRYPTO.md`.

## Own code, own protocol
NexDesk's remote protocol, agent and servers are written for this project. Do not copy source code or protocol definitions from
other remote-desktop projects, in particular copyleft (AGPL) ones: it would bind NexDesk to their licence. Public documentation and
general behaviour may inform design. Interoperating with other products' clients or servers is not a goal.
The project licence (permissive or copyleft) is still to be decided before P5b.

## Parts
```
crates/
  nexdesk-crypto/   hybrid post-quantum handshake and record layer (exists, prototype)
  nexdesk-peer/     wire messages, agent (host) and viewer, X11 capture and input (exists, prototype)
  (planned)         capture and codec crates per platform, a rendezvous/relay service (nexdesk-network)
```
`docs/architecture/ARCHITECTURE.md` names `nexdesk-remote` for the native protocol; `nexdesk-peer` is its prototype and can be renamed or split
when the boundaries are real. The manager reaches it through the Remote Control screen; the viewer is a separate process, like
`nexdesk-rdp`, so the process-isolation rule stays.

## Risks
* Wayland capture and input injection depend on the compositor and prompt the user; unattended Wayland access is the weakest area.
* Video encoding latency needs tuning on real networks; plan for hardware encoders.
* A relay/rendezvous service is infrastructure that must be run and secured; abuse prevention and key management are real work.
* Agents that accept remote control are high-value targets: security review, signed updates and consent-by-default come before features.
