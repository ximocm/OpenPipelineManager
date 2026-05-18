# Security Policy

Open Pipeline Manager is a local-first tool that can execute user-provided pipeline commands. Treat pipeline files as executable local input.

## Supported Versions

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |

## Reporting A Vulnerability

Use GitHub private vulnerability reporting if it is enabled for this repository. If it is not available, open a minimal public issue asking for a private maintainer contact, but do not include exploit details, secrets, or sensitive logs in the issue body.

Please include:

- affected version or commit,
- operating system and runtime versions,
- a minimal reproduction,
- expected impact,
- any known workaround.

## Security Expectations

- Do not commit secrets, tokens, `.env` files, local project runtime state, or logs that may contain credentials.
- Review imported pipelines before running them.
- Keep branch protection enabled for `main`.
- Require CI before merging public changes.
- Keep dependencies updated through reviewed pull requests.
