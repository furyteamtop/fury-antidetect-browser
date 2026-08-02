// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The local store — what one machine knows without a server.
//!
//! Solo use is the default mode (docs/01), and it has to be complete on its own:
//! projects, profiles, proxies. The schema deliberately mirrors the server's,
//! field for field where they overlap, because the two directions that matter
//! both depend on it — pushing a local project up to a team server, and pulling
//! a shared project down to work offline.
//!
//! What is *not* here is anything about people. There are no users, no
//! passwords and no permissions in local mode, because there is nobody to
//! distinguish: whoever can read this file is the owner. Asking for an email
//! and a password with no server to authenticate against would be theatre.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    /// Seals the two columns that are secrets: a proxy password, and a rotation
    /// link that usually embeds an API key. Shared rather than owned, because
    /// the key is per machine and a second one would seal values the first
    /// could not open.
    vault: std::sync::Arc<crate::vault::Vault>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub profile_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proxy {
    pub id: String,
    pub name: String,
    /// "http" | "https" | "socks5"
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    /// Present in local mode: there is nobody to hide it from, and the operator
    /// has to be able to check what they typed. A team server masks this for
    /// anyone without `reveal_secrets`; that distinction only exists there.
    pub password: Option<String>,
    pub last_country: Option<String>,
    pub last_ip: Option<String>,
    /// The timezone of the exit, as the checker reported it.
    ///
    /// This is what a profile with no timezone of its own follows. A browser
    /// claiming Europe/Berlin while its traffic leaves in Amsterdam is one
    /// subtraction away from being noticed, and it is the cheapest check a
    /// detector runs.
    #[serde(default)]
    pub last_timezone: Option<String>,
    /// Where the exit says it is, as "lat,lng". The same lookup that answers
    /// with a timezone answers with this; a profile whose clock follows Berlin
    /// while its geolocation follows the host is the contradiction the timezone
    /// work existed to remove, one field over.
    pub last_location: Option<String>,
    /// A URL that makes the provider hand out a new exit IP.
    ///
    /// Mobile and rotating residential proxies are sold this way, and an
    /// operator who has to leave the app to curl a link will forget to. Stored
    /// with the proxy because it usually embeds an API key — which is also why
    /// a team server will have to encrypt it rather than show it around.
    #[serde(default)]
    pub rotate_url: Option<String>,
    /// Where to ask what the exit looks like. `None` uses the default.
    #[serde(default)]
    pub checker_url: Option<String>,
}

impl Proxy {
    /// The upstream URL the relay takes.
    pub fn url(&self) -> String {
        match (&self.username, &self.password) {
            (Some(u), Some(p)) => format!("{}://{u}:{p}@{}:{}", self.kind, self.host, self.port),
            _ => format!("{}://{}:{}", self.kind, self.host, self.port),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    /// `None` when the profile is in no project. The Profiles list is the
    /// master list — every profile, whatever it is filed under — and a project
    /// is a grouping the profile can be put into or taken out of.
    pub project_id: Option<String>,
    /// Filled in by the listing so the flat view can show where a profile is
    /// filed without a second request per row. Not stored.
    #[serde(default)]
    pub project_name: Option<String>,
    pub name: String,
    pub notes: String,
    pub tags: Vec<String>,
    pub persona_id: String,
    pub fp_seed: i64,
    /// The proxy in full, for a caller that is READING a profile: the list
    /// wants a name and a country to show without a second request per row.
    ///
    /// Optional on the way in, and that is the whole point of the field below.
    /// Writing a profile never used anything here but the id — see
    /// `upsert_profile` — while serde required the entire struct, so a client
    /// that sent `{"proxy": {"id": "..."}}` was refused with `missing field
    /// \`name\``. Which is exactly what the desktop dialog sent, so creating a
    /// profile with a proxy from it had never once worked. The error named a
    /// field the caller had never heard of, on a form where the visible name
    /// box was filled in.
    #[serde(default)]
    pub proxy: Option<Proxy>,
    /// What a caller WRITING a profile supplies: which proxy, and nothing else.
    ///
    /// Takes precedence over `proxy` above. A profile that carried a whole
    /// proxy on the way in could also rewrite that proxy's host and password as
    /// a side effect of being saved, which is not a thing saving a profile
    /// should be able to do.
    #[serde(default)]
    pub proxy_id: Option<String>,
    /// `None` means "follow the proxy's exit" once that resolution lands.
    pub timezone: Option<String>,
    pub languages: Option<Vec<String>>,
    pub start_urls: Vec<String>,
    pub last_opened_at: Option<String>,
}

impl Store {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            // Off by default in SQLite, and the profiles→proxies ON DELETE SET
            // NULL is load-bearing: without this it silently does nothing.
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        // The file holds proxy credentials and, next to it, cookie jars for live
        // accounts. Nothing else on the machine has any business reading it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }

        let store = Self {
            pool,
            vault: std::sync::Arc::new(crate::vault::Vault::open()),
        };
        store.migrate().await?;
        Ok(store)
    }

    /// A store whose vault key is a constant, so tests exercise sealing without
    /// asking the machine's keychain for anything — no permission dialog in the
    /// middle of `cargo test`, and nothing left behind on the machine that ran
    /// it.
    #[cfg(test)]
    pub async fn open_for_tests(path: &std::path::Path) -> anyhow::Result<Self> {
        let store = Self::open(path).await?;
        Ok(Self {
            pool: store.pool.clone(),
            vault: std::sync::Arc::new(crate::vault::Vault::for_tests([42u8; 32])),
        })
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::raw_sql(
            r#"
            CREATE TABLE IF NOT EXISTS projects (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                notes       TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL,
                deleted_at  TEXT
            );

            CREATE TABLE IF NOT EXISTS proxies (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                kind          TEXT NOT NULL,
                host          TEXT NOT NULL,
                port          INTEGER NOT NULL,
                username      TEXT,
                password      TEXT,
                rotate_url    TEXT,
                checker_url   TEXT,
                last_ip       TEXT,
                last_country  TEXT,
                checked_at    TEXT,
                created_at    TEXT NOT NULL,
                deleted_at    TEXT
            );

            CREATE TABLE IF NOT EXISTS profiles (
                id             TEXT PRIMARY KEY,
                -- Nullable, and SET NULL rather than CASCADE. A project is a
                -- way of grouping profiles, not the thing that owns them: the
                -- profile is the asset, and deleting a folder must never be a
                -- way to lose a warmed account. NULL means "not in a project",
                -- which the Profiles list shows like any other.
                project_id     TEXT REFERENCES projects(id) ON DELETE SET NULL,
                name           TEXT NOT NULL,
                notes          TEXT NOT NULL DEFAULT '',
                tags           TEXT NOT NULL DEFAULT '[]',
                persona_id     TEXT NOT NULL,
                fp_seed        INTEGER NOT NULL,
                -- SET NULL rather than CASCADE: deleting a proxy must not delete
                -- the profiles that used it. A warmed account is worth more than
                -- the exit it happened to go out through.
                proxy_id       TEXT REFERENCES proxies(id) ON DELETE SET NULL,
                timezone       TEXT,
                languages      TEXT,
                start_urls     TEXT NOT NULL DEFAULT '[]',
                last_opened_at TEXT,
                created_at     TEXT NOT NULL,
                -- Soft delete: a profile holding a warmed account must not be
                -- lost to a stray click (docs/12).
                deleted_at     TEXT
            );

            CREATE INDEX IF NOT EXISTS profiles_by_project ON profiles(project_id);

            -- Added after the first release of the schema. SQLite has no
            -- IF NOT EXISTS for columns, so these run through a separate path
            -- below; see add_column().
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Columns added after the first schema shipped. SQLite cannot express
        // "add if absent", and a failed ADD COLUMN on an existing column is the
        // expected case rather than an error worth surfacing.
        //
        // `group_name` was one of these and is gone. The column is deliberately
        // *not* dropped from databases that already have it: removing a column
        // in SQLite means rebuilding the table, and rebuilding a table that
        // holds warmed accounts to reclaim a few unused bytes is a bad trade.
        for stmt in [
            "ALTER TABLE proxies ADD COLUMN rotate_url TEXT",
            "ALTER TABLE proxies ADD COLUMN checker_url TEXT",
            "ALTER TABLE proxies ADD COLUMN last_timezone TEXT",
            // "lat,lng" exactly as the checker reports it. Stored as text rather
            // than two reals because it is only ever handed on whole, and a pair
            // of columns invites one of them being set without the other.
            "ALTER TABLE proxies ADD COLUMN last_location TEXT",
        ] {
            let _ = sqlx::query(stmt).execute(&self.pool).await;
        }

        self.allow_profiles_without_a_project().await?;
        Ok(())
    }

    /// Relax `profiles.project_id` to nullable on databases that predate it.
    ///
    /// SQLite cannot alter a column constraint, so this is the documented
    /// twelve-step rebuild — and it is worth the risk exactly once, because the
    /// alternative is that deleting a project keeps taking its profiles with
    /// it. Columns are named explicitly: a database from before the groups
    /// removal still carries `group_name`, and `INSERT INTO ... SELECT *` would
    /// line the columns up wrongly and quietly move data between fields.
    ///
    /// Idempotent — it inspects the existing schema and returns immediately
    /// once the column is already nullable, which is every start after the
    /// first.
    async fn allow_profiles_without_a_project(&self) -> anyhow::Result<()> {
        let notnull: Option<i64> = sqlx::query_scalar(
            "SELECT \"notnull\" FROM pragma_table_info('profiles') WHERE name = 'project_id'",
        )
        .fetch_optional(&self.pool)
        .await?;
        if notnull != Some(1) {
            return Ok(());
        }

        tracing::info!("migrating: a profile may now live outside a project");
        // Foreign keys off for the swap, and the whole thing in one
        // transaction: a database left holding profiles_new and no profiles is
        // an application that will not start again.
        sqlx::raw_sql("PRAGMA foreign_keys = OFF").execute(&self.pool).await?;
        let result = sqlx::raw_sql(
            r#"
            BEGIN;
            CREATE TABLE profiles_new (
                id             TEXT PRIMARY KEY,
                project_id     TEXT REFERENCES projects(id) ON DELETE SET NULL,
                name           TEXT NOT NULL,
                notes          TEXT NOT NULL DEFAULT '',
                tags           TEXT NOT NULL DEFAULT '[]',
                persona_id     TEXT NOT NULL,
                fp_seed        INTEGER NOT NULL,
                proxy_id       TEXT REFERENCES proxies(id) ON DELETE SET NULL,
                timezone       TEXT,
                languages      TEXT,
                start_urls     TEXT NOT NULL DEFAULT '[]',
                last_opened_at TEXT,
                created_at     TEXT NOT NULL,
                deleted_at     TEXT
            );
            INSERT INTO profiles_new
                (id, project_id, name, notes, tags, persona_id, fp_seed, proxy_id,
                 timezone, languages, start_urls, last_opened_at, created_at, deleted_at)
            SELECT
                 id, project_id, name, notes, tags, persona_id, fp_seed, proxy_id,
                 timezone, languages, start_urls, last_opened_at, created_at, deleted_at
            FROM profiles;
            DROP TABLE profiles;
            ALTER TABLE profiles_new RENAME TO profiles;
            CREATE INDEX IF NOT EXISTS profiles_by_project ON profiles(project_id);
            COMMIT;
            "#,
        )
        .execute(&self.pool)
        .await;
        sqlx::raw_sql("PRAGMA foreign_keys = ON").execute(&self.pool).await?;
        result?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // projects
    // -----------------------------------------------------------------------

    pub async fn projects(&self) -> anyhow::Result<Vec<Project>> {
        let rows = sqlx::query(
            "SELECT p.id, p.name, p.notes,
                    (SELECT count(*) FROM profiles f
                      WHERE f.project_id = p.id AND f.deleted_at IS NULL) AS n
             FROM projects p WHERE p.deleted_at IS NULL ORDER BY p.name",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| Project {
                id: r.get("id"),
                name: r.get("name"),
                notes: r.get("notes"),
                profile_count: r.get("n"),
            })
            .collect())
    }

    pub async fn create_project(&self, name: &str, notes: &str) -> anyhow::Result<String> {
        let id = new_id();
        sqlx::query("INSERT INTO projects (id, name, notes, created_at) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(name)
            .bind(notes)
            .bind(now())
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn rename_project(&self, id: &str, name: &str, notes: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE projects SET name = ?, notes = ? WHERE id = ?")
            .bind(name)
            .bind(notes)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Hide a project. Its profiles stay.
    ///
    /// This deleted them too, once, and that was wrong in the way that only
    /// becomes obvious when you say it out loud: a project is a folder, and
    /// deleting a folder is not a reason to destroy what was filed in it. The
    /// profile is the asset — months of a warmed account — and it survives
    /// every organisational decision made about it. They become profiles in no
    /// project, which the Profiles list shows exactly like the rest.
    pub async fn delete_project(&self, id: &str) -> anyhow::Result<u64> {
        let mut tx = self.pool.begin().await?;
        let detached = sqlx::query(
            "UPDATE profiles SET project_id = NULL WHERE project_id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query("UPDATE projects SET deleted_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(detached)
    }

    /// How many profiles a project would take with it.
    ///
    /// Asked before the confirmation is shown, so the dialog can name the
    /// number instead of saying "and its profiles" — the difference between a
    /// warning someone reads and one they click through.
    pub async fn project_profile_count(&self, id: &str) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM profiles WHERE project_id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// The project a fresh installation starts in, created on demand.
    ///
    /// Without it the first run would open on an empty sidebar and a "create a
    /// project" step that answers a question nobody asked — someone launching an
    /// anti-detect browser wants a profile, not a folder.
    pub async fn default_project(&self) -> anyhow::Result<String> {
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM projects WHERE deleted_at IS NULL ORDER BY created_at LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        match existing {
            Some(id) => Ok(id),
            None => self.create_project("My profiles", "").await,
        }
    }

    // -----------------------------------------------------------------------
    // proxies
    // -----------------------------------------------------------------------

    pub async fn proxies(&self) -> anyhow::Result<Vec<Proxy>> {
        let rows = sqlx::query(
            "SELECT id, name, kind, host, port, username, password, last_country, last_ip,
                    last_timezone,
                    rotate_url, checker_url
             FROM proxies WHERE deleted_at IS NULL ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| self.open_proxy(row_to_proxy(r))).collect())
    }

    pub async fn upsert_proxy(&self, p: &Proxy) -> anyhow::Result<String> {
        let id = if p.id.is_empty() { new_id() } else { p.id.clone() };
        sqlx::query(
            "INSERT INTO proxies
                (id, name, kind, host, port, username, password, rotate_url, checker_url, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name, kind = excluded.kind, host = excluded.host,
                port = excluded.port, username = excluded.username,
                password = excluded.password, rotate_url = excluded.rotate_url,
                checker_url = excluded.checker_url, deleted_at = NULL",
        )
        .bind(&id)
        .bind(&p.name)
        .bind(&p.kind)
        .bind(&p.host)
        .bind(p.port as i64)
        .bind(&p.username)
        // Sealed on the way in. A value already stored as plaintext by an older
        // build is rewritten sealed the first time its proxy is saved — which is
        // why there is no migration step to fail on someone's laptop.
        .bind(p.password.as_deref().map(|v| self.vault.seal(v)))
        .bind(p.rotate_url.as_deref().map(|v| self.vault.seal(v)))
        .bind(&p.checker_url)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Unseal the two secret columns.
    fn open_proxy(&self, mut p: Proxy) -> Proxy {
        p.password = p.password.map(|v| self.vault.open_value(&v));
        p.rotate_url = p.rotate_url.map(|v| self.vault.open_value(&v));
        p
    }

    /// Remember what a proxy's exit looked like.
    ///
    /// Stored so a launch does not have to ask the network what timezone to
    /// claim every single time. A profile that follows its exit needs the
    /// answer before the browser starts, and a round trip in front of every
    /// launch is a round trip an operator waits through fifty times a day.
    pub async fn record_exit(
        &self,
        proxy_id: &str,
        ip: Option<&str>,
        country: Option<&str>,
        timezone: Option<&str>,
        location: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE proxies SET last_ip = ?, last_country = ?, last_timezone = ?, \
                    last_location = ?, checked_at = ? WHERE id = ?",
        )
        .bind(ip)
        .bind(country)
        .bind(timezone)
        .bind(location)
        .bind(now())
        .bind(proxy_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The machine key, shared rather than rebuilt: a second vault would seal
    /// values the first could not open.
    pub fn vault(&self) -> &crate::vault::Vault {
        &self.vault
    }

    /// The vault itself, for the one caller that outlives the borrow: priming
    /// the key hands it to a thread.
    pub fn vault_handle(&self) -> &std::sync::Arc<crate::vault::Vault> {
        &self.vault
    }

    /// Whether secrets are actually being sealed, for the interface to report.
    pub fn vault_available(&self) -> bool {
        self.vault.available()
    }

    pub async fn delete_proxy(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE proxies SET deleted_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // profiles
    // -----------------------------------------------------------------------

    /// Every profile, or one project's.
    ///
    /// `None` is the Profiles view and the default one: the flat list of
    /// everything on this machine, which is what an operator actually works
    /// from. Filtering by project is a narrowing of it, not the other way
    /// round.
    pub async fn profiles(&self, project_id: Option<&str>) -> anyhow::Result<Vec<Profile>> {
        let rows = sqlx::query(
            "SELECT f.id, f.project_id, f.name, f.notes, f.tags, f.persona_id, f.fp_seed,
                    f.timezone, f.languages, f.start_urls, f.last_opened_at,
                    p.name AS project_name,
                    x.id AS px_id, x.name AS px_name, x.kind AS px_kind, x.host AS px_host,
                    x.port AS px_port, x.username AS px_user, x.password AS px_pass,
                    x.last_country AS px_country, x.last_ip AS px_ip,
                    x.last_timezone AS px_tz, x.last_location AS px_loc
             FROM profiles f
             LEFT JOIN projects p ON p.id = f.project_id AND p.deleted_at IS NULL
             LEFT JOIN proxies x ON x.id = f.proxy_id AND x.deleted_at IS NULL
             WHERE (?1 IS NULL OR f.project_id = ?1) AND f.deleted_at IS NULL
             ORDER BY f.name",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_profile).collect())
    }

    /// Put profiles into a project, or take them out of one with `None`.
    ///
    /// Bulk, because the operation an operator actually performs is "these six
    /// go to the German shop", never one at a time.
    pub async fn move_profiles(
        &self,
        ids: &[String],
        project_id: Option<&str>,
    ) -> anyhow::Result<u64> {
        let mut moved = 0;
        let mut tx = self.pool.begin().await?;
        for id in ids {
            moved += sqlx::query(
                "UPDATE profiles SET project_id = ? WHERE id = ? AND deleted_at IS NULL",
            )
            .bind(project_id)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(moved)
    }

    /// One profile, with its proxy password.
    ///
    /// This is the launch path, and it is the reason this is not just a filter
    /// over `profiles()`. That listing deliberately blanks the password — a
    /// secret that is never decrypted cannot leak through a list view — and
    /// routing the launch through it meant the relay was handed a proxy with no
    /// credentials. `Proxy::url()` then produced `socks5://host:port`, the
    /// upstream answered 407, and every authenticated proxy simply did not
    /// work. Which is most of the market.
    ///
    /// So the two paths are separate on purpose: the list shows, this one acts.
    pub async fn profile(&self, id: &str) -> anyhow::Result<Option<Profile>> {
        let row = sqlx::query(
            "SELECT f.id, f.project_id, f.name, f.notes, f.tags, f.persona_id, f.fp_seed,
                    f.timezone, f.languages, f.start_urls, f.last_opened_at,
                    p.name AS project_name,
                    x.id AS px_id, x.name AS px_name, x.kind AS px_kind, x.host AS px_host,
                    x.port AS px_port, x.username AS px_user, x.password AS px_pass,
                    x.last_country AS px_country, x.last_ip AS px_ip,
                    x.last_timezone AS px_tz, x.last_location AS px_loc
             FROM profiles f
             LEFT JOIN projects p ON p.id = f.project_id AND p.deleted_at IS NULL
             LEFT JOIN proxies x ON x.id = f.proxy_id AND x.deleted_at IS NULL
             WHERE f.id = ? AND f.deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let sealed: Option<String> = r.get("px_pass");
            let mut profile = row_to_profile(r);
            if let Some(proxy) = profile.proxy.as_mut() {
                proxy.password = sealed.map(|v| self.vault.open_value(&v));
            }
            profile
        }))
    }

    /// Insert or update. `fp_seed` is only assigned on creation — changing it
    /// would silently give an existing profile a different fingerprint, which is
    /// the one thing a warmed account must never do.
    #[allow(clippy::needless_lifetimes)]
    pub async fn upsert_profile(&self, p: &Profile) -> anyhow::Result<String> {
        let id = if p.id.is_empty() { new_id() } else { p.id.clone() };
        let seed = if p.fp_seed == 0 { random_seed() } else { p.fp_seed };
        sqlx::query(
            "INSERT INTO profiles
                (id, project_id, name, notes, tags, persona_id, fp_seed, proxy_id,
                 timezone, languages, start_urls, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                project_id = excluded.project_id, name = excluded.name,
                notes = excluded.notes, tags = excluded.tags,
                persona_id = excluded.persona_id, proxy_id = excluded.proxy_id,
                timezone = excluded.timezone, languages = excluded.languages,
                start_urls = excluded.start_urls, deleted_at = NULL",
        )
        .bind(&id)
        .bind(&p.project_id)
        .bind(&p.name)
        .bind(&p.notes)
        .bind(to_json_array(&p.tags))
        .bind(&p.persona_id)
        .bind(seed)
        .bind(
            p.proxy_id
                .clone()
                .filter(|id| !id.is_empty())
                .or_else(|| p.proxy.as_ref().map(|x| x.id.clone())),
        )
        .bind(&p.timezone)
        .bind(p.languages.as_ref().map(|l| to_json_array(l)))
        .bind(to_json_array(&p.start_urls))
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn delete_profile(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE profiles SET deleted_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn touch_opened(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE profiles SET last_opened_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// What is in the trash, newest first.
    ///
    /// Across every project rather than per project: someone looking for a
    /// profile they deleted by accident usually remembers the account, not
    /// which folder it was in.
    pub async fn deleted_profiles(&self) -> anyhow::Result<Vec<Profile>> {
        let rows = sqlx::query(
            "SELECT f.id, f.project_id, f.name, f.notes, f.tags, f.persona_id, f.fp_seed,
                    f.timezone, f.languages, f.start_urls, f.deleted_at AS last_opened_at,
                    x.id AS px_id, x.name AS px_name, x.kind AS px_kind, x.host AS px_host,
                    x.port AS px_port, x.username AS px_user, x.password AS px_pass,
                    x.last_country AS px_country, x.last_ip AS px_ip,
                    x.last_timezone AS px_tz, x.last_location AS px_loc
             FROM profiles f
             LEFT JOIN proxies x ON x.id = f.proxy_id AND x.deleted_at IS NULL
             WHERE f.deleted_at IS NOT NULL
             ORDER BY f.deleted_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_profile).collect())
    }

    /// Bring a profile back — and its project, if that went too.
    ///
    /// Without this, restoring a profile whose project was deleted puts it back
    /// into a project nothing lists, which is exactly the invisible state the
    /// trash exists to avoid.
    /// Bring a profile back.
    ///
    /// It used to un-delete the parent project too, because deleting a project
    /// trashed its profiles and restoring one otherwise put it somewhere
    /// invisible. Projects no longer take profiles with them, so that is gone —
    /// and a profile whose project was deleted meanwhile comes back to no
    /// project, which is a place the Profiles list shows.
    pub async fn restore_profile(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE profiles SET deleted_at = NULL,
                    project_id = (SELECT p.id FROM projects p
                                   WHERE p.id = profiles.project_id AND p.deleted_at IS NULL)
             WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Gone for good: the row and the browser data both.
    ///
    /// The directory goes too, because leaving a cookie jar on disk for a
    /// profile the operator believes they destroyed is the opposite of what
    /// "empty the trash" means. Deleted first — a failure there must not leave
    /// a row pointing at nothing, but an orphaned directory with no row is
    /// worse still.
    pub async fn purge_profile(&self, id: &str, dir: &Path) -> anyhow::Result<()> {
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        sqlx::query("DELETE FROM profiles WHERE id = ? AND deleted_at IS NOT NULL")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Rows that a soft delete hid. Used by the trash view and by the test that
    /// checks a delete is recoverable.
    pub async fn deleted_profile_exists(&self, id: &str) -> anyhow::Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM profiles WHERE id = ? AND deleted_at IS NOT NULL",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n == 1)
    }
}

fn row_to_profile(r: sqlx::sqlite::SqliteRow) -> Profile {
    Profile {
        id: r.get("id"),
        project_id: r.get("project_id"),
        project_name: r.try_get("project_name").unwrap_or(None),
        name: r.get("name"),
        notes: r.get("notes"),
        tags: from_json_array(r.get("tags")),
        persona_id: r.get("persona_id"),
        fp_seed: r.get("fp_seed"),
        timezone: r.get("timezone"),
        languages: r.get::<Option<String>, _>("languages").map(from_json_array),
        start_urls: from_json_array(r.get("start_urls")),
        last_opened_at: r.get("last_opened_at"),
        // Reading: the id on its own as well as the whole proxy, so a caller
        // that loads a profile, changes a field and saves it back sends the
        // reference rather than a copy of somebody's credentials.
        proxy_id: r.get::<Option<String>, _>("px_id"),
        proxy: r.get::<Option<String>, _>("px_id").map(|id| Proxy {
            id,
            name: r.get("px_name"),
            kind: r.get("px_kind"),
            host: r.get("px_host"),
            port: r.get::<i64, _>("px_port") as u16,
            username: r.get("px_user"),
            // Left sealed here on purpose: the profile list renders a host and
            // a country, never a password, and a secret that is not decrypted
            // is a secret that cannot leak through a list view.
            password: None,
            last_country: r.get("px_country"),
            last_ip: r.get("px_ip"),
            last_timezone: r.try_get("px_tz").unwrap_or(None),
            last_location: r.try_get("px_loc").unwrap_or(None),
            // Not selected in the profile join: a rotation link usually embeds
            // an API key, and no list view has any use for it.
            rotate_url: None,
            checker_url: None,
        }),
    }
}

fn row_to_proxy(r: &sqlx::sqlite::SqliteRow) -> Proxy {
    Proxy {
        id: r.get("id"),
        name: r.get("name"),
        kind: r.get("kind"),
        host: r.get("host"),
        port: r.get::<i64, _>("port") as u16,
        username: r.get("username"),
        password: r.get("password"),
        last_country: r.get("last_country"),
        last_ip: r.get("last_ip"),
        last_timezone: r.try_get("last_timezone").unwrap_or(None),
        last_location: r.try_get("last_location").unwrap_or(None),
        rotate_url: r.get("rotate_url"),
        checker_url: r.get("checker_url"),
    }
}

fn from_json_array(raw: String) -> Vec<String> {
    serde_json::from_str(&raw).unwrap_or_default()
}

fn to_json_array(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn now() -> String {
    // RFC 3339 in UTC, the same shape the server emits, so a local export and a
    // server response never need different parsing.
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn random_seed() -> i64 {
    use rand::Rng;
    // Positive: SQLite integers are signed and the seed reaches the core as a
    // u64, so a negative value would change meaning on the way.
    rand::thread_rng().gen_range(1..i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store that owns its directory. The guard has to outlive the pool —
    /// SQLite writes its journal next to the database — so the two travel
    /// together and the test sees a plain `Store`.
    struct TestStore {
        store: Store,
        _dir: crate::tmp::TempDir,
    }

    impl std::ops::Deref for TestStore {
        type Target = Store;
        fn deref(&self) -> &Store {
            &self.store
        }
    }

    async fn store() -> TestStore {
        let dir = crate::tmp::TempDir::new("test");
        let store = Store::open_for_tests(&dir.join("t.db")).await.unwrap();
        TestStore { store, _dir: dir }
    }

    fn blank(project: &str, name: &str) -> Profile {
        Profile {
            id: String::new(),
            project_id: Some(project.into()),
            project_name: None,
            name: name.into(),
            notes: String::new(),
            tags: vec![],
            persona_id: "macos-15-m-series-1728x1117".into(),
            fp_seed: 0,
            proxy: None,
            proxy_id: None,
            timezone: None,
            languages: None,
            start_urls: vec![],
            last_opened_at: None,
        }
    }

    #[tokio::test]
    async fn a_fresh_install_lands_in_a_project() {
        let s = store().await;
        let id = s.default_project().await.unwrap();
        // Called again it must not make a second one, or every launch would add
        // a project to the sidebar.
        assert_eq!(id, s.default_project().await.unwrap());
        assert_eq!(s.projects().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_new_profile_gets_a_seed_of_its_own() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        s.upsert_profile(&blank(&project, "a")).await.unwrap();
        s.upsert_profile(&blank(&project, "b")).await.unwrap();

        let all = s.profiles(Some(&project)).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_ne!(all[0].fp_seed, all[1].fp_seed, "two profiles shared a seed");
        assert!(all.iter().all(|p| p.fp_seed > 0));
    }

    #[tokio::test]
    async fn editing_a_profile_does_not_move_its_fingerprint() {
        // The whole point of a warmed account: renaming it, retagging it or
        // changing its proxy must leave the machine it claims to be alone.
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let mut p = blank(&project, "shop");
        p.id = s.upsert_profile(&p).await.unwrap();
        let seed = s.profile(&p.id).await.unwrap().unwrap().fp_seed;

        p.name = "shop renamed".into();
        p.tags = vec!["de".into()];
        s.upsert_profile(&p).await.unwrap();

        assert_eq!(s.profile(&p.id).await.unwrap().unwrap().fp_seed, seed);
    }

    #[tokio::test]
    async fn deleting_a_proxy_keeps_the_profiles_that_used_it() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let proxy_id = s
            .upsert_proxy(&Proxy {
                id: String::new(),
                name: "eu".into(),
                kind: "socks5".into(),
                host: "exit.example".into(),
                port: 1080,
                username: None,
                password: None,
                last_country: None,
                last_ip: None,
                last_timezone: None,
                last_location: None,
                rotate_url: None,
                checker_url: None,
            })
            .await
            .unwrap();

        let mut p = blank(&project, "shop");
        p.proxy = Some(Proxy {
            id: proxy_id.clone(),
            name: String::new(),
            kind: String::new(),
            host: String::new(),
            port: 0,
            username: None,
            password: None,
            last_country: None,
            last_ip: None,
            last_timezone: None,
            last_location: None,
            rotate_url: None,
            checker_url: None,
        });
        p.id = s.upsert_profile(&p).await.unwrap();
        s.delete_proxy(&proxy_id).await.unwrap();

        let after = s.profiles(Some(&project)).await.unwrap();
        assert_eq!(after.len(), 1, "the profile went with the proxy");
        assert!(after[0].proxy.is_none());
    }

    #[tokio::test]
    async fn a_deleted_profile_leaves_the_list_but_not_the_database() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let id = s.upsert_profile(&blank(&project, "shop")).await.unwrap();
        s.delete_profile(&id).await.unwrap();

        assert!(s.profiles(Some(&project)).await.unwrap().is_empty());
        assert!(s.profile(&id).await.unwrap().is_none());
        // Still recoverable — that is what the trash in docs/12 restores from.
        assert!(s.deleted_profile_exists(&id).await.unwrap());
    }

    #[tokio::test]
    async fn deleting_a_project_keeps_its_profiles() {
        // Two wrong answers preceded this one. First the project was hidden and
        // the profiles left pointing at it, so they appeared in no list at all.
        // Then they were trashed along with it — visible, but a folder had
        // become a way to destroy months of warmed accounts. Neither is right:
        // the profile is the asset and it outlives every filing decision.
        let s = store().await;
        let project = s.default_project().await.unwrap();
        s.upsert_profile(&blank(&project, "warmed")).await.unwrap();

        assert_eq!(s.delete_project(&project).await.unwrap(), 1, "nothing was detached");
        assert!(s.projects().await.unwrap().is_empty());
        assert!(s.deleted_profiles().await.unwrap().is_empty(), "the profile went to the trash");

        let all = s.profiles(None).await.unwrap();
        assert_eq!(all.len(), 1, "the profile disappeared with its project");
        assert_eq!(all[0].name, "warmed");
        assert_eq!(all[0].project_id, None, "still filed under a deleted project");
    }

    #[tokio::test]
    async fn a_checked_exit_is_remembered_for_the_launch_that_follows_it() {
        // A profile with no timezone of its own follows its exit, and the
        // launch reads it from here rather than paying for a round trip.
        let s = store().await;
        let id = s
            .upsert_proxy(&Proxy {
                id: String::new(),
                name: "eu".into(),
                kind: "socks5".into(),
                host: "exit.example".into(),
                port: 1080,
                username: None,
                password: None,
                last_country: None,
                last_ip: None,
                last_timezone: None,
                last_location: None,
                rotate_url: None,
                checker_url: None,
            })
            .await
            .unwrap();

        s.record_exit(&id, Some("203.0.113.7"), Some("DE"), Some("Europe/Berlin"), Some("52.52,13.405"))
            .await
            .unwrap();

        let stored = s.proxies().await.unwrap();
        assert_eq!(stored[0].last_timezone.as_deref(), Some("Europe/Berlin"));
        assert_eq!(stored[0].last_country.as_deref(), Some("DE"));

        // And the launch path sees it through the profile join, which is the
        // only place it matters.
        let project = s.default_project().await.unwrap();
        let mut p = blank(&project, "follows its exit");
        p.timezone = None;
        p.proxy = Some(Proxy {
            id: id.clone(),
            name: String::new(),
            kind: String::new(),
            host: String::new(),
            port: 0,
            username: None,
            password: None,
            last_country: None,
            last_ip: None,
            last_timezone: None,
            last_location: None,
            rotate_url: None,
            checker_url: None,
        });
        let pid = s.upsert_profile(&p).await.unwrap();
        let launched = s.profile(&pid).await.unwrap().unwrap();
        assert_eq!(
            launched.proxy.unwrap().last_timezone.as_deref(),
            Some("Europe/Berlin"),
            "the launch path cannot see the exit it is supposed to follow"
        );
    }

    #[tokio::test]
    async fn the_launch_path_gets_the_proxy_password_and_the_list_does_not() {
        // Most residential and mobile proxies are sold with credentials. The
        // list blanks the password by design; the launch path must not, or the
        // relay dials anonymously and the upstream answers 407 — which is to
        // say authenticated proxies do not work at all.
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let proxy_id = s
            .upsert_proxy(&Proxy {
                id: String::new(),
                name: "eu".into(),
                kind: "socks5".into(),
                host: "exit.example".into(),
                port: 1080,
                username: Some("bob".into()),
                password: Some("s3cr3t".into()),
                last_country: None,
                last_ip: None,
                last_timezone: None,
                last_location: None,
                rotate_url: None,
                checker_url: None,
            })
            .await
            .unwrap();

        let mut p = blank(&project, "warmed");
        p.proxy = Some(Proxy {
            id: proxy_id,
            name: String::new(),
            kind: String::new(),
            host: String::new(),
            port: 0,
            username: None,
            password: None,
            last_country: None,
            last_ip: None,
            last_timezone: None,
            last_location: None,
            rotate_url: None,
            checker_url: None,
        });
        let id = s.upsert_profile(&p).await.unwrap();

        let launched = s.profile(&id).await.unwrap().unwrap();
        let proxy = launched.proxy.expect("the profile lost its proxy");
        assert_eq!(proxy.password.as_deref(), Some("s3cr3t"));
        assert_eq!(
            proxy.url(),
            "socks5://bob:s3cr3t@exit.example:1080",
            "the relay would dial without credentials"
        );

        // And the listing still shows nothing it does not need to.
        let listed = s.profiles(None).await.unwrap();
        assert_eq!(listed[0].proxy.as_ref().unwrap().password, None);
    }

    #[tokio::test]
    async fn profiles_lists_every_project_at_once() {
        let s = store().await;
        let a = s.create_project("Shops", "").await.unwrap();
        let b = s.create_project("Ads", "").await.unwrap();
        s.upsert_profile(&blank(&a, "etsy")).await.unwrap();
        s.upsert_profile(&blank(&b, "meta")).await.unwrap();

        let all = s.profiles(None).await.unwrap();
        assert_eq!(all.len(), 2, "the flat list is the master list");
        // And it says where each one is filed, so the view can show it without
        // a request per row.
        let names: Vec<_> = all.iter().map(|p| p.project_name.as_deref()).collect();
        assert!(names.contains(&Some("Shops")) && names.contains(&Some("Ads")));

        assert_eq!(s.profiles(Some(&a)).await.unwrap().len(), 1, "a project narrows it");
    }

    #[tokio::test]
    async fn profiles_move_between_projects_and_out_of_them() {
        let s = store().await;
        let a = s.create_project("Shops", "").await.unwrap();
        let b = s.create_project("Ads", "").await.unwrap();
        let id = s.upsert_profile(&blank(&a, "etsy")).await.unwrap();

        assert_eq!(s.move_profiles(&[id.clone()], Some(&b)).await.unwrap(), 1);
        assert_eq!(s.profile(&id).await.unwrap().unwrap().project_id, Some(b.clone()));

        assert_eq!(s.move_profiles(&[id.clone()], None).await.unwrap(), 1);
        assert_eq!(s.profile(&id).await.unwrap().unwrap().project_id, None);
        // Out of every project is still in the Profiles list.
        assert_eq!(s.profiles(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_restored_profile_whose_project_went_away_lands_in_no_project() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let id = s.upsert_profile(&blank(&project, "warmed")).await.unwrap();

        s.delete_profile(&id).await.unwrap();
        s.delete_project(&project).await.unwrap();
        s.restore_profile(&id).await.unwrap();

        // Not into a project nothing lists, which is the invisible state
        // reached from the other direction.
        let back = s.profile(&id).await.unwrap().unwrap();
        assert_eq!(back.project_id, None);
        assert_eq!(s.profiles(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_trash_gives_a_profile_back_intact() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let mut p = blank(&project, "warmed account");
        p.tags = vec!["de".into()];
        p.id = s.upsert_profile(&p).await.unwrap();
        let seed = s.profile(&p.id).await.unwrap().unwrap().fp_seed;

        s.delete_profile(&p.id).await.unwrap();
        assert_eq!(s.deleted_profiles().await.unwrap().len(), 1);

        s.restore_profile(&p.id).await.unwrap();
        let back = s.profile(&p.id).await.unwrap().unwrap();
        // The point of a trash rather than a delete: what comes back is the
        // same machine, not a new profile with the same name.
        assert_eq!(back.fp_seed, seed);
        assert_eq!(back.tags, vec!["de".to_string()]);
        assert!(s.deleted_profiles().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn purging_takes_the_browser_data_with_it() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let id = s.upsert_profile(&blank(&project, "gone")).await.unwrap();

        let dir = std::env::temp_dir().join(format!("fury-purge-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cookies"), b"secrets").unwrap();

        s.delete_profile(&id).await.unwrap();
        s.purge_profile(&id, &dir).await.unwrap();

        assert!(!s.deleted_profile_exists(&id).await.unwrap());
        // Leaving a cookie jar behind for a profile the operator believes they
        // destroyed is the opposite of what emptying a trash means.
        assert!(!dir.exists(), "the profile directory survived the purge");
    }

    #[tokio::test]
    async fn purge_refuses_a_profile_that_is_not_in_the_trash() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let id = s.upsert_profile(&blank(&project, "live")).await.unwrap();
        let dir = std::env::temp_dir().join(format!("fury-nopurge-{id}"));

        s.purge_profile(&id, &dir).await.unwrap();
        // Still there: purge only ever touches rows a delete already hid, so a
        // stray call cannot destroy something in use.
        assert!(s.profile(&id).await.unwrap().is_some());
    }

    #[test]
    fn the_payload_the_profile_dialog_sends_deserialises() {
        // This is the exact JSON desktop/src/components/ProfileDialog.tsx builds,
        // and until 02.08.2026 it was rejected: `proxy` was a required full
        // Proxy, the dialog sent `{"id": "..."}`, and serde answered
        // `missing field \`name\`` — naming a field the person had filled in on
        // another tab. Creating a profile with a proxy from the dialog had
        // therefore never once worked, and nothing here noticed, because every
        // test built a Profile in Rust where the compiler fills the struct.
        //
        // So this test speaks JSON. It is the only kind that could have caught
        // it.
        let payload = serde_json::json!({
            "id": "",
            "project_id": null,
            "name": "Etsy FR",
            "notes": "",
            "tags": [],
            "persona_id": "win11-iris-xe-1920x1080",
            "fp_seed": 0,
            "proxy_id": "px-1",
            "timezone": null,
            "languages": null,
            "start_urls": [],
            "last_opened_at": null
        });
        let p: Profile = serde_json::from_value(payload).expect("the dialog's payload must parse");
        assert_eq!(p.proxy_id.as_deref(), Some("px-1"));
        assert!(p.proxy.is_none(), "the write path carries a reference, not a copy");
    }

    #[test]
    fn a_profile_read_back_can_be_written_back() {
        // The round trip an edit performs: load, change a field, save. It has to
        // survive serialisation both ways, or editing an existing profile fails
        // the same way creating one did.
        let payload = serde_json::json!({
            "id": "p1", "project_id": null, "name": "n", "notes": "", "tags": [],
            "persona_id": "win11-iris-xe-1920x1080", "fp_seed": 7,
            "proxy_id": "px-9",
            "proxy": {
                "id": "px-9", "name": "px", "kind": "socks5", "host": "h", "port": 1,
                "username": null, "password": null, "last_country": null, "last_ip": null,
                "last_timezone": null, "last_location": null,
                "rotate_url": null, "checker_url": null
            },
            "timezone": null, "languages": null, "start_urls": [], "last_opened_at": null
        });
        let p: Profile = serde_json::from_value(payload).expect("a read-back profile must parse");
        let again: Profile = serde_json::from_value(serde_json::to_value(&p).unwrap())
            .expect("and must survive being sent straight back");
        // The reference wins, so saving cannot rewrite the proxy it points at.
        assert_eq!(again.proxy_id.as_deref(), Some("px-9"));
    }

    #[tokio::test]
    async fn proxy_urls_carry_credentials_when_there_are_any() {
        let anon = Proxy {
            id: String::new(),
            name: String::new(),
            kind: "socks5".into(),
            host: "h".into(),
            port: 1080,
            username: None,
            password: None,
            last_country: None,
            last_ip: None,
            last_timezone: None,
            last_location: None,
            rotate_url: None,
            checker_url: None,
        };
        assert_eq!(anon.url(), "socks5://h:1080");

        let auth = Proxy {
            username: Some("bob".into()),
            password: Some("s3cr3t".into()),
            ..anon
        };
        assert_eq!(auth.url(), "socks5://bob:s3cr3t@h:1080");
    }
}
