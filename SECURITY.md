# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in `@mimo-3/slack-cli`, please report it privately via GitHub's
[private vulnerability reporting](https://github.com/mimo-3/slack-cli/security/advisories/new).

Please do **not** open a public issue for security-related concerns. We aim to acknowledge reports
within 7 days and to ship a fix in the next patch release where feasible.

When reporting, please include:

- A clear description of the issue and its impact
- Reproduction steps or a minimal proof-of-concept
- The version of `@mimo-3/slack-cli` you tested against
- Any suggested remediation

## Supported Versions

Only the latest minor version receives security fixes.

| Version | Supported |
| ------- | --------- |
| Latest minor (0.22.x) | ✅ |
| Older versions | ❌ |

## Token Storage Notes

This CLI stores Slack tokens encrypted on disk under `~/.slack-cli/`. Notes:

- **Current format (v2)**: AES-256-GCM with a per-file random master key (PBKDF2, 100,000 iterations,
  SHA-256). Configuration files are written with `0o600` permissions and the directory with `0o700`.
- **Legacy format (v1, pre-0.20)**: AES-256-CBC with a derivation key hard-coded in the source
  (`'slack-cli-key'` + salt `'slack-cli-salt-v1'`). Tokens stored in v1 format are migrated to v2 the
  first time a v0.20+ release reads them, but **any backup of a v1 config file taken before the
  migration can be decrypted by anyone with access to the source**. If you upgraded from a pre-0.20
  release, please discard old configuration backups after the migration runs once.

If you suspect a token has leaked, revoke it in Slack and re-authenticate with `slack-cli config set-token`.
