# fury-server

Self-hosted coordination backend. Deliberately dumb: it stores ciphertext and
metadata, resolves permissions, and hands out presigned URLs. It cannot decrypt
a profile bundle, which is what makes running it on a cheap VPS acceptable.

## Running it locally

No Docker needed. Any PostgreSQL 16+ will do; these are the exact steps used to
verify the API:

```bash
initdb -D /tmp/furypg -U fury --auth=trust
```

```bash
pg_ctl -D /tmp/furypg -o "-p 55432 -k /tmp/fpg -c listen_addresses=127.0.0.1" start
```

The socket directory has to be short — a Unix socket path is capped at 103
bytes, and a long temp path fails with "could not create any Unix-domain
sockets", which reads like a permissions problem and is not.

```bash
createdb -h 127.0.0.1 -p 55432 -U fury fury
```

```bash
DATABASE_URL=postgres://fury@127.0.0.1:55432/fury BIND=127.0.0.1:8901 cargo run -p fury-server
```

**Let the server run the migrations.** Applying them by hand with `psql` leaves
`_sqlx_migrations` empty, and the next start fails with "relation
organizations already exists".

## What the API guarantees

Verified against a real database, not just compiled:

| | Owner | Operator (view+launch) | Member with no grant |
|---|---|---|---|
| `GET /v1/projects` | the project | the project | `[]` |
| Profile's proxy host | `res-eu-01.provider.net` | `res***0236.provider.net` | — |
| `GET .../profiles` on a project they cannot see | — | — | **404**, never 403 |

The masked host is deliberately still distinguishable: an operator has to be
able to tell which exit a profile uses without being able to reuse it. The
trailing digest is what makes that work — a plain prefix collapses
`res-eu-01` and `res-us-02` onto the same string.

Locks behave as docs/06 requires: a second holder gets 409 naming who has it,
`force` needs `manage_access`, and a force-released agent's heartbeat is
rejected so it cannot extend a lock it no longer owns or upload stale state.
