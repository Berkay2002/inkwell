# Security policy

## Reporting a vulnerability

Please do not open public issues for vulnerabilities. Report them privately through GitHub: Security tab >
Report a vulnerability (https://github.com/Berkay2002/inkwell/security/advisories/new).

Include the Inkwell commit or version, the Windows version, and steps or a file that reproduces the problem.

## Supported versions

Only the latest `main` (and the latest release, once there are releases) gets fixes.

## In scope

Inkwell opens untrusted PDFs, so these matter most:

- crashes or memory-safety bugs triggered by a crafted PDF
- an escape from the form-JavaScript sandbox ([docs/forms-js.md](docs/forms-js.md))
- a bypass of signature or certificate validation ([docs/sign.md](docs/sign.md))
- path or command injection in how the assistant launches the Codex or Claude Code CLI ([docs/ai-claude.md](docs/ai-claude.md))
- redaction that leaves the removed content recoverable, or encryption and permissions that do not hold
  ([docs/protect.md](docs/protect.md))

MuPDF bugs that also reproduce with upstream MuPDF should be reported to Artifex as well
(https://bugs.ghostscript.com/).
