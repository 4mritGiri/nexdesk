# Security policy

NexDesk is pre-1.0 and its native remote-control crypto (`crates/nexdesk-crypto`) has **not been independently reviewed**. Do not rely on it for high-risk use yet.

Report vulnerabilities privately to the maintainers (use the repository's private security advisory feature, or the contact address in the repository profile), not in public issues. Include the version, steps to reproduce and impact. We aim to acknowledge within 7 days and to publish a fix and advisory once users can update.

In scope: the handshake and record layer, the relay, the agent's consent and file-transfer rules, credential handling, and anything that runs a program or writes a file because of remote input.
