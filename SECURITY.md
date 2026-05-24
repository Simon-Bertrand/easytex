# Security Policy

EasyTex is intended primarily for local or trusted-network use.

## Supported Versions

Security fixes target the latest released version and the default branch.

## Reporting A Vulnerability

Please report security issues privately before opening a public issue. If the
repository does not yet expose a private security advisory channel, contact the
maintainer directly and include:

- The affected EasyTex version or commit.
- A minimal reproduction.
- Whether the issue requires network exposure, a malicious project file, or a
  malicious LaTeX document.

## Deployment Guidance

- Keep the default localhost bind unless you are behind a trusted reverse proxy.
- Set `EASYTEX_ADMIN_TOKEN` before binding outside localhost.
- Set `EASYTEX_REQUIRE_AUTH=true` for shared environments.
- Keep `allow_shell_escape: false` unless all documents are fully trusted.
- Avoid `RUST_LOG=trace` in production unless logs are tightly controlled.
