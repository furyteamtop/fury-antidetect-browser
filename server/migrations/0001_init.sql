-- Fury server — initial schema
-- PostgreSQL 16+. Requires pgcrypto for gen_random_uuid().
-- UUIDv7 is generated application-side (uuid crate) so IDs sort by creation time.

CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS citext;

-- ---------------------------------------------------------------------------
-- Organizations & users
-- ---------------------------------------------------------------------------

CREATE TABLE organizations (
    id              UUID PRIMARY KEY,
    name            TEXT        NOT NULL,
    -- Org Root Key is never stored in the clear. Each member holds a wrapped copy
    -- in org_members.wrapped_ork. This column only tracks rotation generation so
    -- clients can detect they are holding a stale wrap.
    ork_generation  INT         NOT NULL DEFAULT 1,
    -- When true, offline profile launch is refused by the agent.
    require_online  BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ
);

CREATE TABLE users (
    id              UUID PRIMARY KEY,
    email           CITEXT      NOT NULL UNIQUE,
    -- Argon2id hash of the login password. Distinct from the key-derivation
    -- password material used client-side to unwrap the user key.
    password_hash   TEXT        NOT NULL,
    -- X25519 public key. Used to wrap the ORK for this user.
    public_key      BYTEA       NOT NULL,
    -- Private key, encrypted client-side with the user key. Server cannot read it.
    wrapped_private_key BYTEA   NOT NULL,
    kdf_salt        BYTEA       NOT NULL,
    totp_secret_enc BYTEA,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at     TIMESTAMPTZ
);

CREATE TYPE org_role AS ENUM ('owner', 'admin', 'manager', 'member');

CREATE TABLE org_members (
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            org_role    NOT NULL,
    -- ORK encrypted to this member's X25519 public key.
    wrapped_ork     BYTEA       NOT NULL,
    ork_generation  INT         NOT NULL,
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);

CREATE INDEX ON org_members (user_id);

-- ---------------------------------------------------------------------------
-- Projects & access grants
-- ---------------------------------------------------------------------------

CREATE TABLE projects (
    id              UUID PRIMARY KEY,
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name            TEXT        NOT NULL,
    description     TEXT        NOT NULL DEFAULT '',
    color           TEXT,
    created_by      UUID        NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ
);

CREATE INDEX ON projects (org_id) WHERE deleted_at IS NULL;

-- Permission bits. Mirrored in shared-rs/src/rbac.rs — keep in sync.
--   1 view              32  reveal_secrets
--   2 launch            64  export_cookies
--   4 edit_profile     128  create_profile
--   8 edit_fingerprint 256  delete_profile
--  16 edit_proxy       512  manage_access
CREATE TABLE project_grants (
    project_id      UUID        NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    permissions     BIGINT      NOT NULL,
    granted_by      UUID        NOT NULL REFERENCES users(id),
    granted_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ,
    PRIMARY KEY (project_id, user_id)
);

CREATE INDEX ON project_grants (user_id);

-- ---------------------------------------------------------------------------
-- Proxies
-- ---------------------------------------------------------------------------

CREATE TYPE proxy_kind AS ENUM ('http', 'https', 'socks5', 'ssh');

CREATE TABLE proxies (
    id              UUID PRIMARY KEY,
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- NULL = available org-wide; otherwise scoped to one project.
    project_id      UUID        REFERENCES projects(id) ON DELETE SET NULL,
    name            TEXT        NOT NULL,
    kind            proxy_kind  NOT NULL,
    host            TEXT        NOT NULL,
    port            INT         NOT NULL CHECK (port BETWEEN 1 AND 65535),
    -- Credentials encrypted with a DEK wrapped by the ORK. Server never sees plaintext.
    credentials_enc BYTEA,
    wrapped_dek     BYTEA,
    -- Rotation link, also encrypted (it usually embeds an API key).
    rotate_url_enc  BYTEA,
    -- Last successful check. Denormalised for list views.
    last_ip         INET,
    last_country    TEXT,
    last_timezone   TEXT,
    last_asn        TEXT,
    last_kind_detected TEXT,        -- datacenter | residential | mobile | hosting
    last_checked_at TIMESTAMPTZ,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ
);

CREATE INDEX ON proxies (org_id) WHERE deleted_at IS NULL;

-- ---------------------------------------------------------------------------
-- Profiles
-- ---------------------------------------------------------------------------

CREATE TABLE profiles (
    id              UUID PRIMARY KEY,
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id      UUID        NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name            TEXT        NOT NULL,
    notes           TEXT        NOT NULL DEFAULT '',
    tags            TEXT[]      NOT NULL DEFAULT '{}',

    -- Fingerprint definition. The full config is derived deterministically from
    -- (persona_id, seed) plus explicit overrides; we never store the derived blob.
    persona_id      TEXT        NOT NULL,
    fp_seed         BYTEA       NOT NULL,
    fp_overrides    JSONB       NOT NULL DEFAULT '{}',
    -- Derived from proxy IP at launch when true.
    auto_timezone   BOOLEAN     NOT NULL DEFAULT TRUE,
    auto_geo        BOOLEAN     NOT NULL DEFAULT TRUE,
    auto_locale     BOOLEAN     NOT NULL DEFAULT TRUE,

    proxy_id        UUID        REFERENCES proxies(id) ON DELETE SET NULL,

    start_urls      TEXT[]      NOT NULL DEFAULT '{}',
    -- Extra hardening applied when the launching user lacks export_cookies.
    lock_devtools   BOOLEAN     NOT NULL DEFAULT FALSE,

    current_version INT         NOT NULL DEFAULT 0,
    created_by      UUID        NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ
);

CREATE INDEX ON profiles (project_id) WHERE deleted_at IS NULL;
CREATE INDEX ON profiles (org_id) WHERE deleted_at IS NULL;
CREATE INDEX ON profiles USING GIN (tags);
CREATE INDEX ON profiles (proxy_id) WHERE deleted_at IS NULL;

-- Optional per-profile grant. Exception to the project-level model; used when a
-- single profile must be shared outside its project.
CREATE TABLE profile_grants (
    profile_id      UUID        NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    permissions     BIGINT      NOT NULL,
    granted_by      UUID        NOT NULL REFERENCES users(id),
    granted_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ,
    PRIMARY KEY (profile_id, user_id)
);

-- ---------------------------------------------------------------------------
-- Bundles (encrypted user-data-dir snapshots)
-- ---------------------------------------------------------------------------

CREATE TYPE bundle_kind AS ENUM ('snapshot', 'delta');

CREATE TABLE bundles (
    id              UUID PRIMARY KEY,
    profile_id      UUID        NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    version         INT         NOT NULL,
    kind            bundle_kind NOT NULL,
    -- For deltas: which version this patches from.
    base_version    INT,
    s3_key          TEXT        NOT NULL,
    size_bytes      BIGINT      NOT NULL,
    sha256          BYTEA       NOT NULL,
    -- Profile DEK wrapped by the ORK.
    wrapped_dek     BYTEA       NOT NULL,
    uploaded_by     UUID        NOT NULL REFERENCES users(id),
    uploaded_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (profile_id, version)
);

CREATE INDEX ON bundles (profile_id, version DESC);

-- ---------------------------------------------------------------------------
-- Credentials
-- ---------------------------------------------------------------------------

CREATE TABLE credentials (
    id              UUID PRIMARY KEY,
    profile_id      UUID        NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    label           TEXT        NOT NULL,
    site            TEXT        NOT NULL DEFAULT '',
    -- {username, password, totp_secret, recovery_codes[]} encrypted as one blob.
    payload_enc     BYTEA       NOT NULL,
    wrapped_dek     BYTEA       NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON credentials (profile_id);

-- One-shot tokens let an agent fetch a credential for autofill on behalf of a
-- user who does NOT hold reveal_secrets. Single use, short TTL, audited.
CREATE TABLE autofill_tokens (
    token_hash      BYTEA PRIMARY KEY,
    credential_id   UUID        NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
    profile_id      UUID        NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    issued_to       UUID        NOT NULL REFERENCES users(id),
    field           TEXT        NOT NULL,   -- username | password | totp
    expires_at      TIMESTAMPTZ NOT NULL,
    consumed_at     TIMESTAMPTZ
);

CREATE INDEX ON autofill_tokens (expires_at) WHERE consumed_at IS NULL;

-- ---------------------------------------------------------------------------
-- Locks
-- ---------------------------------------------------------------------------

CREATE TABLE profile_locks (
    profile_id      UUID PRIMARY KEY REFERENCES profiles(id) ON DELETE CASCADE,
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Hash of the lock token. The agent must present the token to upload a bundle,
    -- which is what stops a force-unlocked holder from clobbering newer state.
    token_hash      BYTEA       NOT NULL,
    machine_name    TEXT        NOT NULL,
    machine_id      TEXT        NOT NULL,
    acquired_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL
);

CREATE INDEX ON profile_locks (expires_at);

-- ---------------------------------------------------------------------------
-- Audit — append only
-- ---------------------------------------------------------------------------

CREATE TABLE audit_events (
    id              BIGSERIAL PRIMARY KEY,
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    actor_user_id   UUID        REFERENCES users(id),
    action          TEXT        NOT NULL,
    target_kind     TEXT,
    target_id       UUID,
    detail          JSONB       NOT NULL DEFAULT '{}',
    ip              INET,
    user_agent      TEXT,
    request_id      UUID,
    at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON audit_events (org_id, at DESC);
CREATE INDEX ON audit_events (target_id, at DESC);

-- ---------------------------------------------------------------------------
-- Row-level security: second line of defence under application RBAC.
-- The app connects as `fury_app` and sets `app.user_id` per request.
-- ---------------------------------------------------------------------------

ALTER TABLE projects        ENABLE ROW LEVEL SECURITY;
ALTER TABLE profiles        ENABLE ROW LEVEL SECURITY;
ALTER TABLE proxies         ENABLE ROW LEVEL SECURITY;
ALTER TABLE bundles         ENABLE ROW LEVEL SECURITY;
ALTER TABLE credentials     ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_events    ENABLE ROW LEVEL SECURITY;

CREATE OR REPLACE FUNCTION current_app_user() RETURNS UUID
LANGUAGE sql STABLE AS $$
    SELECT NULLIF(current_setting('app.user_id', TRUE), '')::UUID
$$;

CREATE OR REPLACE FUNCTION user_in_org(target_org UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM org_members m
        WHERE m.org_id = target_org AND m.user_id = current_app_user()
    )
$$;

CREATE POLICY org_isolation ON projects
    USING (user_in_org(org_id));
CREATE POLICY org_isolation ON profiles
    USING (user_in_org(org_id));
CREATE POLICY org_isolation ON proxies
    USING (user_in_org(org_id));
CREATE POLICY org_isolation ON audit_events
    USING (user_in_org(org_id));
CREATE POLICY org_isolation ON bundles
    USING (EXISTS (SELECT 1 FROM profiles p
                   WHERE p.id = bundles.profile_id AND user_in_org(p.org_id)));
CREATE POLICY org_isolation ON credentials
    USING (EXISTS (SELECT 1 FROM profiles p
                   WHERE p.id = credentials.profile_id AND user_in_org(p.org_id)));

-- Audit must not be rewritable, not even by an org owner going through the app.
-- Run as superuser after creating the application role:
--   REVOKE UPDATE, DELETE ON audit_events FROM fury_app;
