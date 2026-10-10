# Contributing

NexDesk is open source under `MIT OR Apache-2.0`; contributions are accepted under the same terms.

* **Original code only.** Do not paste or port code from other remote-desktop or terminal projects, whatever their licence. Using a permissively licensed library as a dependency is fine; say so in the pull request.
* `make check` and `make test` must pass. Changes to `nexdesk-crypto`, the relay or the agent's consent logic need two reviewers and tests.
* Keep security defaults: consent on incoming connections, no secrets in files or arguments, no telemetry.
* Update `docs/FEATURE_MATRIX.md` with an honest status (done / unverified / planned).
* Read `docs/VISION.md` for scope and `docs/ROADMAP.md` for priorities.
