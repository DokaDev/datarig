# Security policy

## Reporting a vulnerability

Please report security issues **privately**, through GitHub's security advisories:
[Report a vulnerability](https://github.com/DokaDev/datarig/security/advisories/new) (the
"Security" tab of the repository, then "Report a vulnerability").

Do not open a public issue for a vulnerability. Include what you found, how to reproduce it,
and the version or commit you tested. You will get an answer as soon as possible, and credit in
the fix unless you prefer otherwise.

## Scope

datarig handles database credentials, SSH keys and host keys, and runs SQL against real
servers, so reports about any of these are especially welcome: passwords or secrets leaking to
disk, logs or the screen; host key checks that can be bypassed; statements that run without
the confirmation or read-only enforcement the safety model promises.

## Supported versions

datarig is pre-1.0: only the latest release is supported, and fixes go into the next release.
