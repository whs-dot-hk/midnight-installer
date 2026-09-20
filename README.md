# midnight-installer

Sets up a Midnight Foundation **federated-node-operator (FNO)** host: a Cardano relay
bootstrapped from a Mithril snapshot, PostgreSQL with `cardano-db-sync`, the Midnight node
with its validator keys, the WireGuard identity, and finally the node running as a validator.

The steps are the ones the FNO runbook prescribes, in the same order — but as plans that can
be inspected before they run, and receipts that can undo them afterwards.

## Why it is shaped this way

The machinery is the [`installer`](https://github.com/swimming-bookstore/installer)
framework: actions, plans, receipts, and the `plan` / `install` / `uninstall` CLI. This crate
supplies what is specific to an FNO host — the planners, the actions they are built from,
the settings, and `status`.

Three concepts, all of them the framework's, carry it:

- **`Action`** — one executable, revertable step (`CreateDirectory`, `InstallBinary`,
  `StartSystemdUnit`, `FetchMithrilSnapshot`). Some are composites which orchestrate
  sub-actions. An action which can tell *while planning* that its work is done (a directory
  which exists, a key which has been generated, a service which is running) says so and is
  skipped; one which cannot (a release archive, a rendered unit file) is applied again, so a
  second run of a stage refreshes the installed release rather than being a strict no-op.
- **`InstallPlan`** — the ordered sequence of actions, plus the planner and version that
  produced it. It is what gets shown for confirmation, and what is written out as the
  receipt.
- **`Planner`** — produces the plan, and holds its settings.

`all` is one planner whose plan is the whole host, so the usual way to build one is a single
command with a single confirmation and a single receipt that unwinds everything in reverse.
The per-component planners below it exist for redoing one part of a host.

A stage installed on its own checks, as its first action, that what the stage before it
produced is in place (`db-sync` looks for the relay, `validator` for the node binary and keys
and for a database it can log in to with the password it was given), and stops with the name
of the missing piece and the step which produces it. Within `all` those checks always pass,
because the same plan produced the piece a few actions earlier.

Installing does not wait for the host to catch up, because nothing needs it to: db-sync
follows a relay which is still syncing, and the node follows a db-sync which is still
filling, each retrying until the one below it has what it wants. So `pre_install_check` only
asks what must be true before anything runs — root, the service users, and the secret root —
and how far along the host actually is belongs to `status`.

## Stages

| Stage | What it does |
| --- | --- |
| `all` | Every stage below, in order, as one plan |
| `directories` | The `/data` and `/secret` layout everything else writes into |
| `cardano` | `cardano-node` + `cardano-cli`, the Mithril client, a snapshot download, and `cardano-node.service` |
| `db-sync` | PostgreSQL from PGDG, the cluster moved onto the data disk and tuned, the role and database, `cardano-db-sync` and its service |
| `midnight` | The `midnight-node` binary, the AURA / GRANDPA / cross-chain keys, the network identity, the keystore, and the registration file |
| `wireguard` | `wireguard-tools` at the pinned tag, and this host's tunnel keypair |
| `validator` | The seed files, the node's environment, `midnight-node.service`, starting it, and checking it really started with its secret-root flags |

## Using it

```console
# Build the whole host (shows the plan and asks first; --no-confirm for automation)
$ sudo midnight-installer install all

# See what it would do, without doing it
# (planning inspects the machine, so it needs the same privileges an install does)
$ sudo midnight-installer plan all
$ sudo midnight-installer plan all --explain    # with the reasoning for each action

# Where has this host got to, and has it caught up?
# (it reads the root-only credentials and asks the relay as its user, so it runs as root)
$ sudo midnight-installer status

# One component at a time, to redo part of a host
$ sudo midnight-installer install cardano
```

After `install all` the host is built but not yet caught up. The relay restores a Mithril
snapshot and follows the chain, db-sync fills the database behind it, and the node restarts
until the database has what it reads — hours for the first two, longer for the third. The
`READINESS` section of `status` is what says when each is there: `[OK]` and `[WAIT]` are a
working service which has or has not caught up, `[MISS]` a stage which has not been run, and
`[FAIL]` a service which is installed but broken, with the reason.

Running a stage again refreshes it: release archives are fetched and unpacked again, units
are rewritten, and directories, keys and databases which already exist are left as they are.
A service is restarted only when the re-run changed its unit file or its binary; a re-run
which changed neither leaves the relay serving blocks. The validator is the exception, and is
always restarted, because its environment file is rewritten with whatever the run was given.

Each `install` writes one receipt, named for the stage: `install all` writes `all.json`, and
`uninstall all` follows it. `uninstall cardano` follows `cardano.json`, which exists only if
the cardano stage was installed on its own, so a host built with `all` is unwound with
`uninstall all`, not stage by stage. Running `install cardano` on such a host does work, to
refresh that part, and writes its own receipt alongside `all.json`.

Settings are flags with environment variable equivalents, and every stage takes the common
ones:

```console
$ sudo midnight-installer install cardano \
    --data-root /data \
    --cardano-user ubuntu \
    --cardano-node-version 10.6.2
```

The db-sync and validator stages need the PostgreSQL password. Set
`MIDNIGHT_INSTALLER_POSTGRES_PASSWORD`, or let it ask at the terminal; `--postgres-password`
also works, but a flag is visible in the process list and in `sudo`'s log. The db-sync stage
saves it, root-only, at `<data root>/postgresql/fno-db-credentials.env`, and the validator
stage reads it from there rather than asking again.

## Releases and checksums

Every release archive (`cardano-node`, `cardano-db-sync`, the Mithril distribution the client
comes from, `midnight-node`) is downloaded, checked against a SHA-256, and only then
unpacked. The checksums for the default versions are built in, taken from what each project
publishes alongside its release. To install another version, pass its checksum as well:

```console
$ sudo midnight-installer install cardano \
    --cardano-node-version 10.7.0 \
    --cardano-node-sha256 <from cardano-node-10.7.0-sha256sums.txt>
```

A version this installer has no checksum for, with none given, is refused rather than
installed unverified.

A `midnight-node` release the host cannot download can be staged on it out of band and
installed from there with `--midnight-archive-path`. Its checksum must then be given with
`--midnight-sha256`, and the file must already be there when the plan is made, so a mistyped
path is caught before the stages ahead of it have run:

```console
$ sudo midnight-installer install all \
    --midnight-archive-path /root/midnight-node-0.22.2-linux-amd64.tar.gz \
    --midnight-sha256 <from the release's checksum file>
```

## What revert will not do

Reverting is for undoing a stage that failed or is being redone — not for wiping the host.
Actions which own irreplaceable state say so in their revert description and leave it alone:

- **Validator keys, the keystore, and the network identity.** A validator's identity cannot
  be regenerated; destroying it is a deliberate act, not a side effect.
- **The WireGuard private key**, once the Foundation may have peered with it.
- **The chain database and the db-sync database**, which would take days to rebuild.
- **System packages and the PostgreSQL server**, which other services may depend on.

Read the uninstall plan (`--explain`) before confirming; it names every one of these.

## Secrets

Every secret this installer creates lives under one root, `--secret-root` (`/secret` by
default): the validator keys, the keystore, the network identity, the node's environment
file, the saved database credentials and the WireGuard keypair. One root so the whole set
can be backed up, audited and restored as a unit rather than hunted for across the data
root. What is *not* secret — the chain database, `res/`, the registration file — stays on
the data root.

The installer will not create that root. It is meant to be a mount backed by the same
stateful disk as the data root, and creating it would quietly put a validator's keys on
whatever carries `/`, which is the disk a rebuilt host throws away:

```console
$ sudo install -d -m 0755 /data/secret /secret
$ echo '/data/secret /secret none bind 0 0' | sudo tee -a /etc/fstab
$ sudo mount /secret
```

Two of those paths are ones `midnight-node` would otherwise derive from `--base-path`, so
the unit has to name them: `--node-key-file` and `--keystore-path`. Writing them into the
unit is not the same as the node being started with them — a systemd drop-in cannot amend
`ExecStart`, only reset it and restate the whole command, so a drop-in written before those
flags existed silently wins without them, and the node falls back to `--base-path` and
re-creates a keystore of real private keys outside the root. `systemctl cat` shows both
lines and looks fine. The `validator` stage therefore reads `/proc/<pid>/cmdline` after
starting the node and **fails** if either flag is missing, rather than reporting success
over a node writing keys to the wrong place.

A host installed before the secret root existed is **not** migrated by this installer. Its
secrets are still in their old places, so the stages which look for them under the root
would find none and generate a fresh identity — which would strand the registration already
sent for that node. Move them into the root by hand first, then run the stages.

The receipt sits next to the installed system, so nothing secret goes into it: `Secret`
serializes as `"<redacted>"` and its `Debug` output is redacted too. Nothing in a revert
needs the value.

Files which hold secrets (`.pgpass`, the saved credentials, the node's environment file, the
validator and network keys) are created with their mode and owner already set, and only then
renamed into place, so they are never briefly readable by anyone else. SQL with a password in
it goes to `psql` on stdin, not in `argv`; the node's `key insert` reads the seed phrase from
a short-lived root-only file rather than a flag; and a command which carries a secret in its
environment or prints one on stdout is run by a helper which never logs either. When the
installer re-runs itself under `sudo`, its `MIDNIGHT_INSTALLER_*` variables are carried
across by name (`--preserve-env`), never as `KEY=VALUE` arguments `sudo` would log.

## Development

```console
$ cargo test
$ cargo clippy --all-targets
$ cargo fmt
```

The crate is Linux/x86-64 only, which `platform_check` enforces: the FNO release archives
are `linux-amd64`.
