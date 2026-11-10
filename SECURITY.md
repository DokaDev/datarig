# Security policy

## Reporting a vulnerability

Please report security issues **privately**, through GitHub's private vulnerability reporting:
[Report a vulnerability](https://github.com/DokaDev/datarig/security/advisories/new) (the
"Security" tab of the repository, then "Report a vulnerability"). Only you and the maintainers
see the report.

Please don't open a public issue, discussion or pull request for a vulnerability: that tells
everyone about it before a fix is available. If you are not sure whether something is a
vulnerability, report it privately anyway; it can always be moved to a public issue later.

A useful report says what you found, how to reproduce it, what an attacker could do with it,
and the version or commit you tested.

## What happens after a report

1. The report is acknowledged, and the discussion stays in the private advisory.
2. Once the problem is confirmed, the fix is prepared privately and ships in a new patch
   release (for example 0.40.1 after 0.40.0). The changelog describes the fix without the
   details that would help someone exploit older versions.
3. After the release, the advisory is published with the affected and the fixed versions, and
   it credits you as the reporter if you wish (say so in the report, or tell us later).

If a report turns out not to be a vulnerability, you will hear why.

datarig has no bug bounty: reports are thanked and credited, not paid.

## Supported versions

datarig is pre-1.0: only the latest release is supported. Security fixes ship as a new patch
release on top of it; older versions do not get fixes, so please upgrade to the latest release.

## Scope

datarig handles database credentials, SSH keys and host keys, and runs SQL against real
servers, so reports about any of these are especially welcome: passwords or secrets leaking to
disk, logs or the screen; host key checks that can be bypassed; statements that run without
the confirmation or read-only enforcement the safety model promises.
