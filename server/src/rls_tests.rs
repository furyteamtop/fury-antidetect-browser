// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Row-level security, against a real PostgreSQL.
//!
//! These are the only tests in this repository that need a database, and they
//! need one because the thing under test is not Rust: it is whether PostgreSQL
//! refuses a query. A mock would assert that the code calls `set_config`, which
//! it plainly does, and would have passed every day of the eight months the
//! policies sat inert.
//!
//! ```bash
//! DATABASE_URL=postgres://fury@127.0.0.1:5432/furytest cargo test -p fury-server
//! ```
//!
//! Skipped, loudly, when `DATABASE_URL` is unset — a hosted CI runner has no
//! Postgres and a silently-skipped security test is worse than a missing one.
//!
//! The database must be OWNED by the connecting role. That is not incidental:
//! the whole first failure was that the app owns its tables and PostgreSQL
//! exempts an owner from its own policies. A test run as some other role would
//! pass against a schema that protects nobody.

use sqlx::{Connection, Executor, PgConnection};

/// One database, one test at a time.
///
/// Each fixture drops and rebuilds the schema, and cargo runs tests in
/// parallel by default — six of those at once against one database closed the
/// connection mid-migration and reported it as a protocol error, which reads
/// like a broken migration and is not one. Serialised rather than given a
/// database each, because a self-hoster running these has one database and the
/// tests take under a second.
static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ORG_A: &str = "11111111-1111-1111-1111-111111111111";
const ORG_B: &str = "22222222-2222-2222-2222-222222222222";
const USER_A: &str = "aaaaaaaa-0000-0000-0000-000000000001";
const USER_B: &str = "bbbbbbbb-0000-0000-0000-000000000002";

fn url() -> Option<String> {
    std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty())
}

/// A connection to a freshly migrated, freshly seeded database.
async fn fixture() -> Option<PgConnection> {
    let url = url()?;
    let mut c = PgConnection::connect(&url).await.expect("connect");

    // Torn down and rebuilt per test rather than shared. Policies are global
    // state; a test that leaves app.user_id set would make the next one pass
    // for the wrong reason, which is the failure mode these tests exist to
    // catch in the server.
    c.execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .await
        .expect("reset schema");

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("migrations directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    files.sort();
    for f in files {
        let sql = std::fs::read_to_string(&f).expect("read migration");
        c.execute(sql.as_str())
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
    }

    // Two organisations, one member each, so "sees only its own" has something
    // to be wrong about.
    let seed = format!(
        "INSERT INTO organizations (id, name) VALUES ('{ORG_A}','A'),('{ORG_B}','B');
         INSERT INTO users (id, email, password_hash, public_key, wrapped_private_key, kdf_salt)
           VALUES ('{USER_A}','a@example.com','x','\\x00','\\x00','\\x00'),
                  ('{USER_B}','b@example.com','x','\\x00','\\x00','\\x00');
         INSERT INTO org_members (org_id, user_id, role, wrapped_ork, ork_generation)
           VALUES ('{ORG_A}','{USER_A}','owner','\\x00',1),
                  ('{ORG_B}','{USER_B}','owner','\\x00',1);"
    );
    c.execute(seed.as_str()).await.expect("seed");
    Some(c)
}

async fn bind(c: &mut PgConnection, user: &str) {
    sqlx::query("SELECT set_config('app.user_id', $1, false)")
        .bind(user)
        .execute(&mut *c)
        .await
        .expect("bind");
}

async fn make_project(c: &mut PgConnection, id: &str, org: &str, owner: &str) {
    sqlx::query("INSERT INTO projects (id, org_id, name, created_by) VALUES ($1::uuid,$2::uuid,$3,$4::uuid)")
        .bind(id)
        .bind(org)
        .bind("p")
        .bind(owner)
        .execute(&mut *c)
        .await
        .expect("insert project");
}

async fn count_projects(c: &mut PgConnection) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(&mut *c)
        .await
        .expect("count")
}

macro_rules! db_test {
    ($name:ident, $conn:ident, $body:block) => {
        #[tokio::test]
        async fn $name() {
            let _guard = DB_LOCK.lock().await;
            let Some(mut $conn) = fixture().await else {
                eprintln!(
                    "SKIPPED {}: set DATABASE_URL to a database owned by the connecting role",
                    stringify!($name)
                );
                return;
            };
            $body
        }
    };
}

db_test!(policies_are_forced_not_merely_enabled, c, {
    // The first failure, as an assertion. ENABLE without FORCE reads as
    // protected in every tool that shows it, and protects nothing from the role
    // the application actually connects as.
    let rows: Vec<(String, bool, bool)> = sqlx::query_as(
        "SELECT c.relname, c.relrowsecurity, c.relforcerowsecurity
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relkind = 'r'
           AND c.relname IN ('projects','profiles','proxies','bundles','credentials',
                             'audit_events','project_grants','profile_grants',
                             'profile_locks','autofill_tokens')
         ORDER BY c.relname",
    )
    .fetch_all(&mut c)
    .await
    .expect("read pg_class");

    assert_eq!(rows.len(), 10, "a table lost its policy: {rows:?}");
    for (name, enabled, forced) in rows {
        assert!(enabled, "{name} has row security disabled");
        assert!(forced, "{name} is ENABLE without FORCE — the owner is exempt, so it protects nobody");
    }
});

db_test!(an_unbound_connection_sees_nothing, c, {
    bind(&mut c, USER_A).await;
    make_project(&mut c, "cccccccc-0000-0000-0000-000000000001", ORG_A, USER_A).await;

    // The failure direction that makes a mechanical conversion safe: a handler
    // that forgets to take a bound connection reads an empty set, not somebody
    // else's rows.
    sqlx::query("SELECT set_config('app.user_id', '', false)")
        .execute(&mut c)
        .await
        .expect("unbind");
    assert_eq!(count_projects(&mut c).await, 0, "an unbound connection could read rows");
});

db_test!(a_bound_connection_sees_only_its_own_organisation, c, {
    bind(&mut c, USER_A).await;
    make_project(&mut c, "cccccccc-0000-0000-0000-000000000001", ORG_A, USER_A).await;
    bind(&mut c, USER_B).await;
    make_project(&mut c, "dddddddd-0000-0000-0000-000000000002", ORG_B, USER_B).await;

    bind(&mut c, USER_A).await;
    assert_eq!(count_projects(&mut c).await, 1, "user A saw more than their own organisation");
    bind(&mut c, USER_B).await;
    assert_eq!(count_projects(&mut c).await, 1, "user B saw more than their own organisation");
});

db_test!(a_row_cannot_be_written_into_another_organisation, c, {
    bind(&mut c, USER_A).await;
    // The interesting half: reading is the obvious thing to protect, and
    // planting a row in somebody else's organisation is the one that would let
    // a compromised handler grow into a foothold.
    let err = sqlx::query("INSERT INTO projects (id, org_id, name, created_by) VALUES (gen_random_uuid(),$1::uuid,'x',$2::uuid)")
        .bind(ORG_B)
        .bind(USER_A)
        .execute(&mut c)
        .await
        .expect_err("an insert into a foreign organisation was accepted");
    assert!(
        err.to_string().contains("row-level security"),
        "refused for the wrong reason: {err}"
    );
});

db_test!(the_tables_0001_missed_are_covered_too, c, {
    // project_grants says who may open which account. It had no policy at all,
    // so the second line covered the profiles and not the keys to them.
    bind(&mut c, USER_A).await;
    make_project(&mut c, "cccccccc-0000-0000-0000-000000000001", ORG_A, USER_A).await;
    sqlx::query("INSERT INTO project_grants (project_id, user_id, permissions, granted_by) VALUES ($1::uuid,$2::uuid,255,$3::uuid)")
        .bind("cccccccc-0000-0000-0000-000000000001")
        .bind(USER_A)
        .bind(USER_A)
        .execute(&mut c)
        .await
        .expect("grant");

    bind(&mut c, USER_B).await;
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM project_grants")
        .fetch_one(&mut c)
        .await
        .expect("count grants");
    assert_eq!(seen, 0, "user B could read another organisation's access grants");
});

db_test!(the_reset_hook_clears_the_binding, c, {
    // main::connect installs an after_release hook so a pooled connection
    // cannot carry one request's identity into the next. Asserted here against
    // a real pool rather than by reading the hook, because the failure it
    // prevents is one user reading another user's rows — the exact thing the
    // policies exist for, arriving through the plumbing instead.
    let Some(u) = url() else { return };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1) // one connection, so the second acquire is the first one back
        .after_release(|conn, _| {
            Box::pin(async move {
                sqlx::query("SELECT set_config('app.user_id','',false)")
                    .execute(conn)
                    .await?;
                Ok(true)
            })
        })
        .connect(&u)
        .await
        .expect("pool");

    {
        let mut held = pool.acquire().await.expect("acquire");
        bind(&mut held, USER_A).await;
    } // released here

    let mut again = pool.acquire().await.expect("re-acquire");
    let carried: Option<String> = sqlx::query_scalar("SELECT current_setting('app.user_id', true)")
        .fetch_one(&mut *again)
        .await
        .expect("read setting");
    let carried = carried.unwrap_or_default();
    assert!(
        carried.is_empty(),
        "a connection carried {carried} back into the pool — the next request would run as them"
    );
    let _ = &mut c;
});

db_test!(audit_cannot_be_rewritten, c, {
    bind(&mut c, USER_A).await;
    sqlx::query("INSERT INTO audit_events (org_id, actor_user_id, action, detail) VALUES ($1::uuid,$2::uuid,'test','{}'::jsonb)")
        .bind(ORG_A)
        .bind(USER_A)
        .execute(&mut c)
        .await
        .expect("write an audit row");

    // 0001 asked for this in a comment addressed to whoever installed the
    // server. 0006 states it, and this is what makes the difference visible.
    for sql in [
        "UPDATE audit_events SET action = 'rewritten'",
        "DELETE FROM audit_events",
    ] {
        let err = sqlx::query(sql)
            .execute(&mut c)
            .await
            .expect_err(&format!("`{sql}` was permitted"));
        assert!(
            err.to_string().contains("permission denied"),
            "refused for the wrong reason: {err}"
        );
    }
});
