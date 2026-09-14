# midnight-installer

Sets up a Midnight Foundation **federated-node-operator (FNO)** host: a Cardano relay
bootstrapped from a Mithril snapshot, PostgreSQL with `cardano-db-sync`, the Midnight node
with its validator keys, the WireGuard identity, and finally the node running as a validator.

The steps are the ones the FNO runbook prescribes, in the same order — but as plans that can
be inspected before they run, and receipts that can undo them afterwards.

## Why it is shaped this way

Three concepts carry the whole crate:

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

Installing does not wait for the host to catch up, because nothing needs it to: db-sync
follows a relay which is still syncing, and the node follows a db-sync which is still
filling, each retrying until the one below it has what it wants. So `pre_install_check` only
asks what must be true before anything runs — root, and the service users — and how far along
the host actually is belongs to `status`.

## Stages

| Stage | What it does |
| --- | --- |
| `all` | Every stage below, in order, as one plan |
| `directories` | The `/data` layout everything else writes into |
| `cardano` | `cardano-node` + `cardano-cli`, the Mithril client, a snapshot download, and `cardano-node.service` |
| `db-sync` | PostgreSQL from PGDG, the cluster moved onto the data disk and tuned, the role and database, `cardano-db-sync` and its service |
| `midnight` | The `midnight-node` binary, the AURA / GRANDPA / cross-chain keys, the network identity, the keystore, and the registration file |
| `wireguard` | `wireguard-tools` at the pinned tag, and this host's tunnel keypair |
| `validator` | The seed files, the node's environment, `midnight-node.service`, and starting it |

## Using it

```console
# Build the whole host (shows the plan and asks first; --no-confirm for automation)
$ sudo midnight-installer install all

# See what it would do, without doing it
# (planning inspects the machine, so it needs the same privileges an install does)
$ sudo midnight-installer plan all
$ sudo midnight-installer plan all --explain    # with the reasoning for each action

# Where has this host got to, and has it caught up?
$ midnight-installer status

# One component at a time, to redo part of a host
$ sudo midnight-installer install cardano
$ sudo midnight-installer uninstall cardano
```

After `install all` the host is built but not yet caught up. The relay restores a Mithril
snapshot and follows the chain, db-sync fills the database behind it, and the node restarts
until the database has what it reads — hours for the first two, longer for the third. The
`READINESS` section of `status` is what says when each is there.

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
