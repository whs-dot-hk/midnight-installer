# midnight-installer

Sets up a Midnight Foundation **federated-node-operator (FNO)** host: a Cardano relay
bootstrapped from a Mithril snapshot, PostgreSQL with `cardano-db-sync`, the Midnight node
with its validator keys, the WireGuard identity, and finally the node running as a validator.

The steps are the ones the FNO runbook prescribes, in the same order — but as plans that can
be inspected before they run, and receipts that can undo them afterwards.

It can also run a **devnet** of your own — a Midnight chain with its own validators, on one
host or many, reached over SSH. See [Devnets](#devnets).

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
saves it, root-only, at `<secret root>/fno-db-credentials.env`, and the validator
stage reads it from there rather than asking again.

## Where things go

The files are the ones the FNO guides
create, with the same names and the same paths relative to the service user's home. The
difference is that `~` is split across two roots: `--data-root` (`/data`) for everything
which can be rebuilt, and `--secret-root` (`/secret`) for everything which cannot. A
command from the guides works with `~` replaced by the right root.

| Guides | Here |
| --- | --- |
| `~/cardano-data/{db,schema,db-sync-state,db-sync-config.json}` | `/data/cardano-data/...` |
| `~/data` (node base path), `~/res` | `/data/midnight-data`, `/data/res` |
| `~/partner-chains-public-keys.json` | `/data/partner-chains-public-keys.json` |
| `~/aura.json`, `~/grandpa.json`, `~/cross_chain.json` | `/secret/aura.json`, ... |
| `~/data/chains/<chain>/network/secret_ed25519` | `/secret/data/chains/<chain>/network/secret_ed25519` |
| `~/data/chains/<chain>/keystore` | `/secret/data/chains/<chain>/keystore` |
| `~/keystore/<type><public key>` (`*_SEED_FILE`) | `/secret/keystore/<type><public key>` |
| `~/.env`, `~/.pgpass` | `/secret/.env`, `/secret/.pgpass` |

`<chain>` is `midnight` on mainnet and `midnight_preprod` on preprod. The binaries and the
Cardano configuration stay in `~/.local/bin` and `~/.local/share`, as in the guides, and
`~/res` links to `/data/res`. The WireGuard keypair and the saved database credentials,
which the guides do not place, are `/secret/wireguard/{privatekey,publickey}` and
`/secret/fno-db-credentials.env`.

Two places where it departs from the guides' letter:

- The seed files hold the bare secret phrase. They are named the way the keystore names its
  entries, which is what the guides' `.env` shows, but they are not the keystore's own files:
  those hold the phrase as a quoted JSON string, which the node does not unquote when it reads
  a seed file. The type in each name is the one the node files the seed under: `aura`
  (`61757261`), `gran` (`6772616e`) and `crch` (`63726368`). The guides' example labels
  GRANDPA and CROSS_CHAIN with each other's prefixes.
- The network key and the keystore are under the secret root rather than the base path, so
  the unit names them with `--node-key-file` and `--keystore-path`.

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
file, the saved database credentials, the `.pgpass` `cardano-db-sync` authenticates with,
and the WireGuard keypair. One root so the whole set
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

Three of those are ones a program would otherwise find by convention, so the units have to
name them: `--node-key-file` and `--keystore-path` for `midnight-node`, which otherwise
derives both from `--base-path`, and `PGPASSFILE` for `cardano-db-sync`, because libpq
reads `~/.pgpass` and nothing else unless it is told otherwise. Writing them into the
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

## Devnets

`midnight-installer devnet` runs a Midnight chain of its own over SSH: its validators are the
ones you list, its main chain is the node's built-in mock follower rather than Cardano, and it
can be torn down and rebuilt at will. It runs from your machine, which needs only to reach the
hosts, and installs nothing there.

Describe the devnet in a TOML file. One host:

```toml
chain = "devnet"

[ssh]
kind = "gcloud"               # `gcloud compute ssh`, through an IAP tunnel
project = "my-project"
zone = "europe-west1-b"

[[hosts]]
name = "devnet-0"
validators = ["alice", "bob", "charlie"]
```

Several hosts: list each, with the nodes it runs. Node names are devnet-wide.

```toml
[[hosts]]
name = "devnet-0"
validators = ["alice", "bob"]

[[hosts]]
name = "devnet-1"
validators = ["charlie"]
full_nodes = ["archive"]
```

```console
# The hosts run this installer too, uploaded by `up`, so it has to be a static build
$ cargo build --release --target x86_64-unknown-linux-musl
$ BIN=target/x86_64-unknown-linux-musl/release/midnight-installer

$ $BIN devnet up devnet.toml        # install, generate keys, build the genesis, start
$ $BIN devnet status devnet.toml    # each node's service, best and finalized block, peers
$ $BIN devnet down devnet.toml      # stop everything, delete keys and chain data
```

(`ring`, through `rustls`, compiles some C: if the musl build cannot find
`x86_64-linux-musl-gcc`, set `CC_x86_64_unknown_linux_musl=gcc`.)

`up` runs in four steps, and is safe to run again:

1. **On every host at once:** upload this installer, and run the `devnet-keys` stage with it.
   That installs the pinned `midnight-node` release (checksum-verified, `1.0.400` by default)
   and generates each node's keys **on the host**: seeds never leave it. Only each node's
   `public.json` — public keys and peer id — is read back.
2. **On this machine:** assemble the genesis committee from every validator, in the order
   the file lists them.
3. **On the first host:** run `devnet-nodes`, which builds the chain spec from the release's
   `dev` preset with that committee, writes a systemd unit per node, and starts them.
4. **On every other host at once:** hand it the first host's chain spec, byte for byte, and
   run `devnet-nodes` there.

The first host's first node is the bootnode every other node dials. Each node's ports are the
base port (`[ports]`: `p2p = 30333`, `rpc = 9944`, `prometheus = 9615`) plus its index on
its host. RPC and Prometheus stay local to each host.

Running `up` again keeps every key, and keeps the chain spec as long as the committee is the
same. A changed committee means a new genesis, so the nodes' chain data is deleted with the
old one.

| On each host | |
| --- | --- |
| `<root>/<chain>/release/` | `midnight-node` and its `res/` (`root` defaults to `/var/lib/midnight-devnet`) |
| `<root>/<chain>/nodes/<node>/secrets/` | The seeds, mode `700`, as the node's user |
| `<root>/<chain>/nodes/<node>/{node.key,public.json,node.env,data}` | |
| `<root>/<chain>/chain-spec.json`, `committee.json`, `mock-registrations.json` | The genesis |
| `<root>/<chain>/receipts/` | Receipts of `devnet-keys` and `devnet-nodes` |
| `midnight-devnet-<chain>-<node>.service` | One unit per node |

Only the first host's first node has to be reachable from the others, on its p2p port: every
other node dials it, and libp2p carries everything both ways over that connection. Where a
firewall opens just one port (say `30333`), give the first host that port and the others any
free ones:

```toml
[[hosts]]
name = "devnet-1"
validators = ["charlie"]

[hosts.ports]               # this host only; the rest of [ports] still applies
p2p = 31333
rpc = 10944
prometheus = 10615
```

Other settings, all optional: `version` and `sha256` (another release), `slots_per_epoch`
(`10`), `user` (`midnight`; it must exist on every host), `root`, `[ports]`, and per host
`zone` and `address` (the address the others dial; by default the host's first). With
`kind = "ssh"`, `name` is an `ssh` destination and `options` adds flags to `ssh` and `scp`:

```toml
[ssh]
kind = "ssh"
options = ["-i", "~/.ssh/devnet"]

[[hosts]]
name = "ubuntu@10.0.0.5"
validators = ["alice"]
```

Two devnets can share hosts if they have different `chain` names and port ranges.

## Development

```console
$ cargo test
$ cargo clippy --all-targets
$ cargo fmt
```

The crate is Linux/x86-64 only, which `platform_check` enforces: the FNO release archives
are `linux-amd64`.
