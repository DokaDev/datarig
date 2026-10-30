# SSH tunnels

A profile can reach its database through one SSH bastion. datarig opens the tunnel itself (in
Rust, without the `ssh` program) and dials every connection of the profile through
`direct-tcpip` channels: each tab's session, the explorer's metadata session, cancel requests
and test connections. Nothing else is requested from the SSH server, and no local port is
opened.

## Pick a tunnel in the profile form

The form's SSH section has three choices: **off**, a **tunnel preset**, or **this profile
only** (its own bastion fields). `Enter` on the value lists them with each preset's bastion,
filtered as you type. Its last entry, **New SSH tunnel…**, opens the tunnel form and comes back
with the new preset picked. In a key file field, `Ctrl+O` picks the file in a tree.

## Authentication

| `auth` | What it uses |
|---|---|
| `key` (the default) | A private key file: OpenSSH format (ed25519, ECDSA, RSA) or PEM (PKCS#1, PKCS#8, EC, including AWS `.pem` files), encrypted or not. A certificate next to the key (`<file>-cert.pub`) is used too. |
| `password` | The bastion account's password. |
| `agent` | Your running ssh-agent. |
| `keyboard-interactive` | The server's questions, such as one-time codes, asked in a dialog. |

- As with OpenSSH, a key file that others may read is refused on Unix: `chmod 600` it. PuTTY
  keys (`.ppk`) are named as such; convert them with `puttygen`.
- The key's passphrase or the SSH password comes from the same sources as database passwords
  (`secret_source`, default the keychain; see [Connections](connections.md#passwords)). It is
  stored apart from the database password.

## Host keys

Host keys are checked against `~/.ssh/known_hosts` (read only, never written: hashed names,
`*` and `?` wildcards, negations, `[name]:port` and `@revoked` lines are understood) and
against datarig's own `known_hosts` in its data directory, the only one datarig writes. An
unknown or changed key is always asked about, never accepted silently.

## Tunnel presets

A tunnel preset is a named tunnel that profiles share, written as `[tunnels.<name>]`; a profile
names it with `tunnel = "<name>"`.

```toml
[tunnels.office]
host = "bastion.example.com"
port = 22
user = "ec2-user"
auth = "key"
key_file = "~/.ssh/prod.pem"
keepalive = 15                 # seconds between keepalives (the default); 0: none
timeout = 10                   # seconds each step of opening the tunnel may take (the default)

[[connections]]
name = "prod"
tunnel = "office"
# ...

[[connections]]
name = "lab"
# ...
[connections.ssh]              # a tunnel of this profile only
enabled = true
host = "lab-bastion.example.com"
user = "me"
auth = "agent"
```

- Presets live in the explorer's **Tunnels** section, below the connections: `n` makes one,
  `e` edits it (renaming takes the profiles that name it along), `c` copies it, `d` deletes it,
  `t` tests it, and `Enter` lists the profiles connected through it now. The command line also
  finds the action `tunnel.new`.
- Each preset shows whether its SSH connection is closed, opening, open (with how many
  profiles are on it) or lost (and why) by the color of its icon (`○`, `●` and `✕` with icons
  off).
- Profiles that use the same preset at the same time share **one SSH connection**: one login,
  one host key question and one secret prompt for every profile waiting. It closes when the
  last of them disconnects. When it is lost, every profile on it says so and connects again on
  its next use.
- A preset's secret is kept once, under the preset's own account, which a rename keeps.
  Deleting a preset asks first, lists the profiles that use it and removes its saved secret.
- A preset changed while profiles are connected through it: they keep their connection until
  they connect again, and then use the new settings.
- Testing a preset opens its SSH connection and then one `direct-tcpip` channel to each
  database address of the profiles that use it, closing each again. Nothing is sent to a
  database.
- **Save as tunnel preset** (`Ctrl+B` in the form, on the profile's own tunnel) turns the
  profile's own tunnel into a preset when the form is saved. The stored secret is copied to
  the preset and read back before the old copy is removed.

## When something is wrong

- A profile whose preset does not exist, or that names a preset and also has its own tunnel
  turned on, does not connect and says why. It **never** connects directly instead.
- Keepalives detect a dead tunnel; the explorer marks the profile until its next use
  reconnects.
- The app never rewrites tunnels on its own: presets change only when you save a form, rename
  or delete one.

## Not yet

- Several hops (a bastion behind a bastion).
- Reading hosts from `~/.ssh/config`: the bastion is always entered in the profile or preset.
