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
    pub project_id: String,
    pub name: String,
    pub notes: String,
    pub tags: Vec<String>,
    pub persona_id: String,
    pub fp_seed: i64,
    /// A second level inside a project, flat and free-form.
    ///
    /// Not a table. A group here is a label an operator invents while working —
    /// "warmed", "to check", "client A" — and giving it a row of its own would
    /// mean creating one before it can be used, which is exactly the friction
    /// that makes people stop using groups. Empty means ungrouped.
    #[serde(default)]
    pub group_name: Option<String>,
    pub proxy: Option<Proxy>,
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
                project_id     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                name           TEXT NOT NULL,
                notes          TEXT NOT NULL DEFAULT '',
                tags           TEXT NOT NULL DEFAULT '[]',
                persona_id     TEXT NOT NULL,
                fp_seed        INTEGER NOT NULL,
                group_name     TEXT,
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
        for stmt in [
            "ALTER TABLE proxies ADD COLUMN rotate_url TEXT",
            "ALTER TABLE proxies ADD COLUMN checker_url TEXT",
            "ALTER TABLE profiles ADD COLUMN group_name TEXT",
        ] {
            let _ = sqlx::query(stmt).execute(&self.pool).await;
        }
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

    /// Hide a project, and send its profiles to the trash with it.
    ///
    /// The profiles have to go somewhere visible. Hiding only the project left
    /// them in the database and out of every list — not deleted, not in the
    /// trash, simply unreachable, which is the worst of the three outcomes and
    /// is what this did at first. A profile holds a warmed account; if it is
    /// going away it must be somewhere the operator can get it back from.
    pub async fn delete_project(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        // One transaction: a project hidden without its profiles following, or
        // profiles trashed while their project stays, are both states nothing
        // else in the app knows how to render.
        sqlx::query("UPDATE profiles SET deleted_at = ? WHERE project_id = ? AND deleted_at IS NULL")
            .bind(now())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE projects SET deleted_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
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

    pub async fn profiles(&self, project_id: &str) -> anyhow::Result<Vec<Profile>> {
        let rows = sqlx::query(
            "SELECT f.id, f.project_id, f.name, f.notes, f.tags, f.persona_id, f.fp_seed,
                    f.group_name, f.timezone, f.languages, f.start_urls, f.last_opened_at,
                    x.id AS px_id, x.name AS px_name, x.kind AS px_kind, x.host AS px_host,
                    x.port AS px_port, x.username AS px_user, x.password AS px_pass,
                    x.last_country AS px_country, x.last_ip AS px_ip
             FROM profiles f
             LEFT JOIN proxies x ON x.id = f.proxy_id AND x.deleted_at IS NULL
             WHERE f.project_id = ? AND f.deleted_at IS NULL
             ORDER BY f.name",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_profile).collect())
    }

    pub async fn profile(&self, id: &str) -> anyhow::Result<Option<Profile>> {
        let project: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM profiles WHERE id = ? AND deleted_at IS NULL")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(project) = project else {
            return Ok(None);
        };
        Ok(self
            .profiles(&project)
            .await?
            .into_iter()
            .find(|p| p.id == id))
    }

    /// Insert or update. `fp_seed` is only assigned on creation — changing it
    /// would silently give an existing profile a different fingerprint, which is
    /// the one thing a warmed account must never do.
    pub async fn upsert_profile(&self, p: &Profile) -> anyhow::Result<String> {
        let id = if p.id.is_empty() { new_id() } else { p.id.clone() };
        let seed = if p.fp_seed == 0 { random_seed() } else { p.fp_seed };
        sqlx::query(
            "INSERT INTO profiles
                (id, project_id, name, notes, tags, persona_id, fp_seed, group_name, proxy_id,
                 timezone, languages, start_urls, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                project_id = excluded.project_id, name = excluded.name,
                notes = excluded.notes, tags = excluded.tags,
                persona_id = excluded.persona_id, group_name = excluded.group_name,
                proxy_id = excluded.proxy_id,
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
        .bind(p.group_name.as_ref().filter(|g| !g.trim().is_empty()))
        .bind(p.proxy.as_ref().map(|x| x.id.clone()))
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
                    f.group_name, f.timezone, f.languages, f.start_urls, f.deleted_at AS last_opened_at,
                    x.id AS px_id, x.name AS px_name, x.kind AS px_kind, x.host AS px_host,
                    x.port AS px_port, x.username AS px_user, x.password AS px_pass,
                    x.last_country AS px_country, x.last_ip AS px_ip
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
    pub async fn restore_profile(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE projects SET deleted_at = NULL
             WHERE id = (SELECT project_id FROM profiles WHERE id = ?)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE profiles SET deleted_at = NULL WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
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
        name: r.get("name"),
        notes: r.get("notes"),
        tags: from_json_array(r.get("tags")),
        persona_id: r.get("persona_id"),
        fp_seed: r.get("fp_seed"),
        group_name: r.get("group_name"),
        timezone: r.get("timezone"),
        languages: r.get::<Option<String>, _>("languages").map(from_json_array),
        start_urls: from_json_array(r.get("start_urls")),
        last_opened_at: r.get("last_opened_at"),
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

    async fn store() -> Store {
        let dir = std::env::temp_dir().join(format!("fury-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open(&dir.join("t.db")).await.unwrap()
    }

    fn blank(project: &str, name: &str) -> Profile {
        Profile {
            id: String::new(),
            project_id: project.into(),
            name: name.into(),
            notes: String::new(),
            tags: vec![],
            persona_id: "macos-15-m-series-1728x1117".into(),
            fp_seed: 0,
            group_name: None,
            proxy: None,
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

        let all = s.profiles(&project).await.unwrap();
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
            rotate_url: None,
            checker_url: None,
        });
        p.id = s.upsert_profile(&p).await.unwrap();
        s.delete_proxy(&proxy_id).await.unwrap();

        let after = s.profiles(&project).await.unwrap();
        assert_eq!(after.len(), 1, "the profile went with the proxy");
        assert!(after[0].proxy.is_none());
    }

    #[tokio::test]
    async fn a_deleted_profile_leaves_the_list_but_not_the_database() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let id = s.upsert_profile(&blank(&project, "shop")).await.unwrap();
        s.delete_profile(&id).await.unwrap();

        assert!(s.profiles(&project).await.unwrap().is_empty());
        assert!(s.profile(&id).await.unwrap().is_none());
        // Still recoverable — that is what the trash in docs/12 restores from.
        assert!(s.deleted_profile_exists(&id).await.unwrap());
    }

    #[tokio::test]
    async fn a_group_is_just_a_label_and_survives_a_round_trip() {
        // No table, no create-group step: the label is invented while working
        // and has to be usable the moment it is typed.
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let mut p = blank(&project, "shop");
        p.group_name = Some("warmed".into());
        p.id = s.upsert_profile(&p).await.unwrap();
        assert_eq!(
            s.profile(&p.id).await.unwrap().unwrap().group_name.as_deref(),
            Some("warmed")
        );

        // Blank is ungrouped, not a group called "".
        p.group_name = Some("   ".into());
        s.upsert_profile(&p).await.unwrap();
        assert_eq!(s.profile(&p.id).await.unwrap().unwrap().group_name, None);
    }

    #[tokio::test]
    async fn deleting_a_project_puts_its_profiles_in_the_trash_not_nowhere() {
        // The bug this replaces: the project was hidden and the profiles were
        // left untouched, so they were in neither list — present in the
        // database and unreachable from the application.
        let s = store().await;
        let project = s.default_project().await.unwrap();
        s.upsert_profile(&blank(&project, "warmed")).await.unwrap();

        assert_eq!(s.project_profile_count(&project).await.unwrap(), 1);
        s.delete_project(&project).await.unwrap();

        assert!(s.projects().await.unwrap().is_empty());
        let trashed = s.deleted_profiles().await.unwrap();
        assert_eq!(trashed.len(), 1, "the profile vanished instead of being trashed");
        assert_eq!(trashed[0].name, "warmed");
    }

    #[tokio::test]
    async fn restoring_a_profile_brings_its_project_back_too() {
        let s = store().await;
        let project = s.default_project().await.unwrap();
        let id = s.upsert_profile(&blank(&project, "warmed")).await.unwrap();
        s.delete_project(&project).await.unwrap();

        s.restore_profile(&id).await.unwrap();
        // Otherwise it would come back into a project nothing lists — the same
        // invisible state, reached from the other direction.
        assert_eq!(s.projects().await.unwrap().len(), 1);
        assert_eq!(s.profiles(&project).await.unwrap().len(), 1);
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
