-- Session tokens.
--
-- Opaque and random, not JWTs: revocation has to be immediate for a tool whose
-- selling point is that an operator's access ends the moment they leave, and a
-- JWT can only be revoked by keeping the denylist it was meant to avoid.
--
-- Only the SHA-256 of the token is stored, so a dump of this table grants
-- nothing. docs/01 assumes the server itself may be compromised.

CREATE TABLE sessions (
    token_hash   BYTEA PRIMARY KEY,
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Sliding: refreshed on each authenticated request, so a full shift is not
    -- interrupted while an abandoned token still dies on schedule.
    expires_at   TIMESTAMPTZ NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- For the audit trail: which machine a session was opened from.
    machine_name TEXT        NOT NULL DEFAULT '',
    ip           INET
);

CREATE INDEX ON sessions (user_id);
-- Expired rows are swept by a periodic job; the index keeps that cheap.
CREATE INDEX ON sessions (expires_at);
