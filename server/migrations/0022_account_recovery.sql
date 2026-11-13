-- Account recovery and owner hand-over (docs/25).
--
-- A recovery key is a second lock on the private key, made on the client from
-- 256 random bits the person writes down. The server keeps the private key
-- wrapped under it, the salt, and an Argon2id hash of a separate value derived
-- from it (the "auth" half, shared-rs recovery.rs): enough to tell a right key
-- from a wrong one, nothing to open anything with.

ALTER TABLE users ADD COLUMN recovery_salt        BYTEA;
ALTER TABLE users ADD COLUMN recovery_wrapped_key BYTEA;
ALTER TABLE users ADD COLUMN recovery_auth_hash   TEXT;
ALTER TABLE users ADD COLUMN recovery_created_at  TIMESTAMPTZ;

-- One attempt to recover an account, from the moment the key was accepted to
-- the moment a new password was set (or it was cancelled, or it lapsed).
--
-- Like login_challenges, outside row-level security: the person using it is
-- by definition not signed in. What a client presents is `secret`, of which
-- only the hash is kept, so a dump of this table resumes nothing.
CREATE TABLE recovery_requests (
    id              UUID        PRIMARY KEY,
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    secret_hash     BYTEA       NOT NULL UNIQUE,
    -- totp | email | wait: the second step this attempt must pass, chosen when
    -- it starts and not changeable after.
    factor          TEXT        NOT NULL CHECK (factor IN ('totp', 'email', 'wait')),
    -- The emailed code's hash, for factor 'email'.
    code_hash       BYTEA,
    attempts        INT         NOT NULL DEFAULT 0,
    -- When the second step was passed, or for 'wait' when the 24 hours end.
    ready_at        TIMESTAMPTZ,
    -- The hash of the nonce sealed to the account's public key, answered on
    -- finish: the proof that whoever finishes holds the private key.
    challenge_hash  BYTEA,
    ip              INET,
    machine_name    TEXT        NOT NULL DEFAULT '',
    machine_id      TEXT,
    user_agent      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL,
    cancelled_at    TIMESTAMPTZ,
    cancelled_by    UUID        REFERENCES users(id) ON DELETE SET NULL,
    completed_at    TIMESTAMPTZ
);
CREATE INDEX ON recovery_requests (user_id);
CREATE INDEX ON recovery_requests (expires_at);

-- Wrong recovery keys, by the address they were tried against, for the limit
-- of five an hour (recovery.rs). Its own table rather than a count over
-- login_events: that journal is under FORCE ROW LEVEL SECURITY, an unbound
-- connection counts nothing there, and a limit that silently counts zero is
-- no limit -- which is what the first version of this was, caught by
-- recovery_tests.rs. Unknown addresses are counted too, so the limit does not
-- tell an existing account from a missing one.
CREATE TABLE recovery_failures (
    email   CITEXT      NOT NULL,
    ip      INET,
    at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON recovery_failures (email, at);
