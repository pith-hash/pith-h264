# Security Policy

## Reporting a Vulnerability

Report privately through GitHub: open the repository's **Security** tab and choose
**Report a vulnerability**. The report stays visible only to the maintainers until a
fix ships.

Direct link: <https://github.com/pith-hash/pith-h264/security/advisories/new>

Please do not open a public issue for a security vulnerability.

If the link above returns a 404, private vulnerability reporting is not enabled yet on
this repository; open an issue asking for it to be turned on, without describing the
vulnerability.

## Suite hardening guarantees

This repository is part of the **pith** suite. Beyond the reporting policy above,
the following properties are enforced by CI and are relevant when assessing a
report:

- `#![forbid(unsafe_code)]` is declared in every crate root; a report of memory
  unsafety implies a compiler bug or a violation of this declaration.
- The dependency graph is limited to other `pith-*` crates and `std`
  (`scripts/check-zero-deps.py`); supply-chain reports should name the offending
  dependency edge.
- Parser crates ship a fuzz corpus (`fuzz/corpus/`) replayed in CI; a crash
  reproduction should include the failing corpus input.
- `reference.json` vectors are hex-exact and shared across the Python, Node and
  Go SDKs; a correctness report should state which vector diverges, in which
  SDK.
