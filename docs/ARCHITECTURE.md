# NexDesk Enterprise Architecture

## 1. Vision

NexDesk is a cross-platform enterprise remote-access platform. It is not limited to a single protocol or operating system.

Supported and planned connection types:

| Protocol | Purpose | Status |
|----------|---------|--------|
| RDP | Interoperability with RDP-compatible endpoints | Active |
| SSH | Secure terminal, SFTP, port forwarding | Planned |
| NexDesk Remote | Full cross-platform remote control | Planned |
| Network Tunnel / VPN | Secure site-level network access | Planned |

---

## 2. High-Level Architecture

```mermaid
graph TD
    User[User]
    UI[nexdesk-ui<br/>Presentation Layer]
    App[nexdesk<br/>Application Entry]
    Core[nexdesk-core<br/>Domain State]
    Session[nexdesk-session<br/>Session Orchestration]
    Policy[Policy Engine<br/>Authorization]
    Identity[Identity<br/>Authentication]
    Crypto[nexdesk-crypto<br/>Cryptographic Boundary]

    RDP[nexdesk-rdp<br/>RDP Protocol]
    SSH[nexdesk-ssh<br/>SSH Protocol]
    Remote[nexdesk-remote<br/>Native Protocol]

    Clipboard[nexdesk-clipboard<br/>Clipboard & Files]
    Renderer[nexdesk-renderer<br/>Display Pipeline]
    Network[nexdesk-network<br/>Transport & Relay]

    User --> UI
    UI --> App
    App --> Core
    App --> Session
    App --> Policy
    Policy --> Identity
    Policy --> Crypto
    Session --> RDP
    Session --> SSH
    Session --> Remote
    Session --> Clipboard
    Session --> Renderer
    Session --> Network
```

**Key rule:** The UI displays and requests. The application orchestrates. Policy authorizes. Sessions manage lifecycle. Protocols communicate. Cryptography protects. Network transports.

---

## 3. Workspace Structure

```
crates/
├── nexdesk/                # Application entry point (thin)
├── nexdesk-core/           # Domain state: profiles, credentials, settings, vault
├── nexdesk-session/        # Session lifecycle, orchestration, capability negotiation
├── nexdesk-rdp/            # RDP protocol client implementation
├── nexdesk-clipboard/      # Clipboard sync and file transfer
├── nexdesk-renderer/       # Framebuffer and display pipeline
└── nexdesk-ui/             # User interface, navigation, theme, assets

vendor/
├── ironrdp-client/         # Vendored IronRDP client
└── ironrdp-tls/            # Vendored IronRDP TLS

docs/                       # Architecture, security, roadmap documentation
packaging/                  # Desktop files, install scripts
```

### Future crates (create only when boundary is real)

```
crates/
├── nexdesk-crypto/         # Hybrid PQC: ML-KEM, ML-DSA, X25519, Ed25519
├── nexdesk-ssh/            # SSH terminal, SFTP, port forwarding
├── nexdesk-remote/         # Native NexDesk remote protocol
├── nexdesk-network/        # Relay, rendezvous, NAT traversal
├── nexdesk-vpn/            # Network tunnel / site access
├── nexdesk-policy/         # Enterprise policy engine (if extracted from core)
└── nexdesk-identity/       # Enterprise identity (if extracted from core)
```

---

## 4. Dependency Direction

```mermaid
graph TD
    UI[nexdesk-ui] --> App[nexdesk]
    App --> Core[nexdesk-core]
    App --> Session[nexdesk-session]
    Session --> RDP[nexdesk-rdp]
    Session --> SSH[nexdesk-ssh]
    Session --> Remote[nexdesk-remote]
    Session --> Clipboard[nexdesk-clipboard]
    Session --> Renderer[nexdesk-renderer]
    Session --> Network[nexdesk-network]
    Policy[Policy Engine] --> Crypto[nexdesk-crypto]
    Session --> Policy
    App --> Policy
```

**Rules:**
- Dependencies point downward. No cycles.
- `nexdesk-crypto` must never depend on `nexdesk-session`, UI, or domain crates.
- `nexdesk-core` remains protocol-agnostic and UI-agnostic.
- Protocol crates (`rdp`, `ssh`, `remote`) do not own application-wide state.

---

## 5. Cross-Platform Model

RDP is a protocol. It is not Windows-only. NexDesk connects to any endpoint where a compatible server exists.

```mermaid
graph LR
    subgraph Protocols
        RDP[RDP]
        SSH[SSH]
        NRD[NexDesk Remote]
    end

    subgraph Endpoints
        WIN[Windows]
        LIN[Linux]
        MAC[macOS]
        AND[Android]
        IOS[iOS]
    end

    RDP --> WIN
    RDP --> LIN
    RDP --> MAC
    SSH --> WIN
    SSH --> LIN
    SSH --> MAC
    SSH --> AND
    NRD --> WIN
    NRD --> LIN
    NRD --> MAC
    NRD --> AND
    NRD --> IOS
```

### Client-platform independence

The NexDesk client runs on any platform and connects to any supported endpoint:

```
Linux client    → Windows RDP endpoint
macOS client    → Linux SSH endpoint
Windows client  → Android NexDesk agent
Android client  → macOS NexDesk agent
```

### Endpoint capability negotiation

Capabilities vary by endpoint OS and protocol. They are negotiated at session establishment, not hardcoded.

```mermaid
graph TD
    Session[Session Establishment]
    Cap[Capability Negotiation]
    Session --> Cap

    Cap --> Display[Display]
    Cap --> Input[Input: KB / Mouse / Touch]
    Cap --> Clip[Clipboard]
    Cap --> Files[File Transfer]
    Cap --> Audio[Audio]
    Cap --> Tunnel[Network Tunnel]
    Cap --> Record[Recording]
```

---

## 6. Session Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Created
    Created --> Starting
    Starting --> Connecting
    Connecting --> Connected : success
    Connecting --> Failed : error / timeout
    Starting --> Failed : config error
    Connected --> Reconnecting : connection lost
    Reconnecting --> Connected : restored
    Reconnecting --> Disconnecting : retry limit
    Connected --> Disconnecting : user / policy
    Failed --> [*]
    Disconnecting --> Disconnected
    Disconnected --> [*]
```

**Rules:**
- The session manager owns lifecycle state.
- Protocol implementations must not directly mutate UI state.
- Reconnection requires a protocol-safe contract. No silent credential replay.

---

## 7. Security Architecture

### 7.1 Security boundaries

```mermaid
graph TD
    subgraph Untrusted
        UIInput[UI Input]
        RemotePeer[Remote Peer]
        Relay[Relay / Rendezvous]
    end

    subgraph Trusted
        PolicyEngine[Policy Enforcement]
        SessionLayer[Session Layer]
        CryptoBoundary[Cryptographic Boundary]
    end

    UIInput --> PolicyEngine
    RemotePeer --> CryptoBoundary
    Relay --> CryptoBoundary
    PolicyEngine --> SessionLayer
    SessionLayer --> CryptoBoundary
```

**Critical principle:** A higher-level component must never bypass a security decision enforced by a lower-level component.

- UI cannot grant a denied capability.
- A relay cannot decrypt an end-to-end encrypted session.
- A remote peer cannot silently enable disabled capabilities.

### 7.2 Capability-based authorization

Every remote capability is independently authorized. A connection does not inherit all features.

```mermaid
graph LR
    Session[Session Request]
    Policy[Policy Engine]
    Session --> Policy

    Policy --> Screen[Screen: allow]
    Policy --> Input[Input: allow]
    Policy --> Clip[Clipboard: deny]
    Policy --> Upload[File Upload: deny]
    Policy --> Download[File Download: allow]
    Policy --> Audio[Audio: deny]
    Policy --> Tunnel[Tunnel: deny]
    Policy --> Rec[Recording: allow]
```

### 7.3 Cryptographic design (target)

```mermaid
graph TD
    subgraph Hybrid Key Exchange
        X25519[X25519]
        MLKEM[ML-KEM-768]
    end

    subgraph Hybrid Identity
        ED25519[Ed25519]
        MLDSA[ML-DSA-65]
    end

    subgraph Session Encryption
        AEAD[XChaCha20-Poly1305 / AES-256-GCM]
    end

    X25519 --> HKDF[HKDF-SHA-256]
    MLKEM --> HKDF
    ED25519 --> Transcript[Transcript Binding]
    MLDSA --> Transcript
    HKDF --> SessionKeys[Per-Direction Session Keys]
    SessionKeys --> AEAD
```

**Properties:**
- Hybrid: secure if either classical or PQ algorithm holds.
- Forward secrecy via ephemeral key material.
- Transcript binding prevents downgrade attacks.
- Algorithm identifiers are explicit. No implicit negotiation.
- Rekeying by time and data volume.

### 7.4 RDP post-quantum mitigation

NexDesk cannot control RDP server TLS parameters. For PQ protection of RDP:

```mermaid
graph LR
    Client[NexDesk Client] --> Tunnel[Secure Tunnel<br/>SSH / WireGuard]
    Tunnel --> RDP[RDP / TLS Session]
    RDP --> Server[Endpoint]
```

The tunnel provides the PQ protection. The inner RDP session retains its own security properties.

---

## 8. Enterprise Identity

```mermaid
graph TD
    Human[Human User]
    Device[Managed Device]

    Human --> Auth[User Authentication<br/>FIDO2 / TOTP / SSO]
    Device --> Cert[Device Identity<br/>Certificate / mTLS]

    Auth --> PolicyEngine[Policy Engine]
    Cert --> PolicyEngine
    PolicyEngine --> Session[Session Authorization]
```

**Rules:**
- Human identity and device identity are separate.
- Authentication ≠ authorization.
- A permanent shared password must not be the only factor for enterprise access.
- Identity supports rotation and revocation.

---

## 9. Enterprise Policy Enforcement

```mermaid
graph TD
    Admin[Enterprise Administrator]
    PolicyService[Policy Service / Signed File]
    LocalEngine[Local Policy Engine]
    Session[Session Layer]
    Enforcement[Capability Enforcement]

    Admin --> PolicyService
    PolicyService --> LocalEngine
    LocalEngine --> Session
    Session --> Enforcement
```

Policy controls include:
- Allowed device IDs and user identities
- Required authentication methods
- Per-capability allow/deny
- Required relay or tunnel
- Minimum client version
- Session duration and idle timeout
- Geographic or network boundaries

**The UI may display policy state. The UI must never override it.**

---

## 10. Network Architecture

```mermaid
graph TD
    subgraph Endpoint A
        ClientA[NexDesk Client]
    end

    subgraph Infrastructure
        Rendezvous[Rendezvous Service]
        Relay[Encrypted Relay]
    end

    subgraph Endpoint B
        AgentB[NexDesk Agent / Server]
    end

    ClientA -->|discovery| Rendezvous
    ClientA -->|encrypted payload| Relay
    Relay -->|encrypted payload| AgentB
    ClientA -.->|direct if possible| AgentB
```

**Rules:**
- Relay sees only ciphertext and routing metadata.
- Relay must not receive session plaintext, clipboard contents, or file contents.
- Metadata (timing, volume, addresses) is visible to infrastructure by design.

---

## 11. Audit Architecture

```mermaid
graph LR
    Session[Session Events]
    Auth[Authentication Events]
    PolicyD[Policy Decisions]
    FileOps[File Transfer Metadata]

    Session --> AuditLog[Audit Record]
    Auth --> AuditLog
    PolicyD --> AuditLog
    FileOps --> AuditLog

    AuditLog --> Local[Local Log]
    AuditLog --> SIEM[Enterprise SIEM<br/>syslog / OTLP]
```

**Audit records must NOT contain:**
- Passwords, tokens, private keys
- Clipboard contents
- File contents
- Screen contents

**Target:** Hash-chained, signed audit records for tamper evidence.

---

## 12. Renderer Architecture

```mermaid
graph LR
    RDP[nexdesk-rdp] --> FB[Framebuffer]
    Remote[nexdesk-remote] --> FB
    FB --> Renderer[nexdesk-renderer]
    Renderer --> Backend[GPU / Windowing Backend]
    Backend --> UI[nexdesk-ui]
```

The renderer is independent of protocol decoding. This allows future GPU-accelerated presentation without coupling to any specific protocol implementation.

---

## 13. Credential Flow

```mermaid
graph LR
    UI[UI: profile selection]
    Core[nexdesk-core:<br/>credential metadata]
    Store[Secure Store:<br/>keyring / vault]
    Session[nexdesk-session:<br/>Secret type]

    UI --> Core
    Core --> Store
    Store --> Session
    Session --> Protocol[Protocol Layer]
```

**Rules:**
- Profiles contain connection metadata, not plaintext secrets.
- Secrets enter the session boundary as dedicated secret types.
- The UI never receives plaintext from the persistence layer.
- Secrets are never logged, serialized, or exposed via `Debug`.

---

## 14. Logging

Use structured logging with fields:

```
session_id, device_id, user_id, protocol, destination, state, error_code
```

**Never log:** passwords, tokens, private keys, clipboard contents, file contents, screen contents.

**High-frequency events** (packets, frames, input, clipboard polling): use `trace` or throttled logging. Never unbounded `debug!`.

---

## 15. Crate Responsibility Summary

| Crate | Owns | Must NOT contain |
|-------|------|-----------------|
| `nexdesk` | App startup, composition | Business logic |
| `nexdesk-core` | Domain state, profiles, credentials, settings, vault | Protocol logic, UI state |
| `nexdesk-session` | Session lifecycle, capability negotiation | Cryptographic primitives, UI state |
| `nexdesk-rdp` | RDP protocol client | App-wide state, policy decisions |
| `nexdesk-clipboard` | Clipboard sync, file transfer | Session management, UI |
| `nexdesk-renderer` | Framebuffer, display pipeline | Protocol decoding, business logic |
| `nexdesk-ui` | Presentation, navigation, theme | Security decisions, protocol logic |
| `nexdesk-crypto` *(future)* | Key exchange, signatures, KDF, AEAD | Protocol logic, domain state |
| `nexdesk-ssh` *(future)* | SSH, SFTP, port forwarding | App-wide state |
| `nexdesk-remote` *(future)* | Native remote protocol | App-wide state |
| `nexdesk-network` *(future)* | Relay, rendezvous, NAT traversal | Protocol decoding |
| `nexdesk-vpn` *(future)* | Network tunnel, routing | Session management |

---

## 16. Architectural Principles

1. **Protocol ≠ Platform.** RDP connects to any RDP endpoint. SSH connects to any SSH server. Platform-specific code lives in endpoint adapters.

2. **Security below UI.** The UI requests. Policy decides. A bypassed UI must not grant capabilities.

3. **Capability ≠ Connection.** A connected session does not implicitly receive all features. Each capability is independently authorized.

4. **Crypto is isolated.** Cryptographic primitives live in a dedicated crate with no knowledge of protocols, UI, or domain.

5. **Minimal coupling.** Protocol crates are independent. Adding a new protocol does not require redesigning existing ones.

6. **No premature abstraction.** Create a crate when the ownership boundary is real, not when the name sounds clean.

7. **Fail closed.** Security decisions default to deny. Unknown states produce clean failures, not silent fallbacks.

8. **Forward secrecy.** Long-term identity keys never directly encrypt session traffic.

9. **Audit without exposure.** Security events are logged. Secrets and content are never logged.

10. **Cross-platform by design.** Client OS and endpoint OS are independent variables. The architecture must not encode platform assumptions into protocol or session logic.
