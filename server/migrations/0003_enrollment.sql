-- Enrolment: how a person comes to have an account.
--
-- There is no open registration, for the reason docs/13 gives: a server holding
-- live working accounts should not have a door anyone can walk through. But the
-- alternative that was documented — hand-written INSERTs — could not work. The
-- users table requires `public_key`, `wrapped_private_key` and `kdf_salt`, and
-- org_members requires `wrapped_ork`; all four are produced by client-side
-- cryptography from a password the server must never see. No amount of SQL at a
-- psql prompt can produce them.
--
-- So enrolment is an invitation the server issues and the client completes. The
-- server says "this address may join, as this role"; the client generates its
-- keys, wraps its own private key under its own password, and posts back only
-- the wrapped material. The server ends up storing exactly what it can do
-- nothing with.

CREATE TABLE enrollments (
    id              UUID PRIMARY KEY,
    -- Only the hash. An invitation code is a bearer credential for the few
    -- minutes it lives, and a database dump should not yield working ones —
    -- the same reasoning as sessions.token_hash.
    code_hash       BYTEA       NOT NULL UNIQUE,
    email           CITEXT      NOT NULL,

    -- NULL means "create the organisation on enrolment": the first person on a
    -- fresh server has nobody to invite them, so the code that bootstraps an
    -- owner carries the name of the org to create.
    org_id          UUID        REFERENCES organizations(id) ON DELETE CASCADE,
    org_name        TEXT,
    role            org_role    NOT NULL,

    -- The ORK, wrapped to the invitee's public key. NULL for an owner, who
    -- generates the ORK, and for an invitation issued before the invitee has
    -- published a public key — see `enrollment_pending_key` below.
    wrapped_ork     BYTEA,
    ork_generation  INT,

    -- Short by design. An invitation that sits in an inbox for a month is a
    -- credential nobody is watching.
    expires_at      TIMESTAMPTZ NOT NULL,
    used_at         TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Either it joins an existing org or it creates one, never both and never
    -- neither. Enforced here because a code that means nothing would only be
    -- discovered by the person trying to use it.
    CONSTRAINT enrollment_targets_one_org
        CHECK ((org_id IS NULL) <> (org_name IS NULL)),
    -- An owner brings the ORK into being; anyone else must be handed one. A
    -- member invitation with no wrapped ORK is legal only while it waits for
    -- the inviter to wrap it — never once it has been used.
    CONSTRAINT enrollment_owner_has_no_wrapped_ork
        CHECK (role <> 'owner' OR wrapped_ork IS NULL)
);

CREATE INDEX ON enrollments (email) WHERE used_at IS NULL;

-- A member can exist before they hold the organisation key.
--
-- The ORK is wrapped to a member's public key, and that key does not exist
-- until they have enrolled — so requiring the wrapping at the moment of joining
-- made the two steps circular. NULL now means "joined, waiting to be handed the
-- key": they can sign in and see the shape of the organisation, and decrypt
-- nothing, until an owner or admin wraps the ORK to the public key they
-- published. Which is also the honest model of an invitation: someone asked to
-- join, and someone still has to let them in.
ALTER TABLE org_members ALTER COLUMN wrapped_ork DROP NOT NULL;
ALTER TABLE org_members ALTER COLUMN ork_generation DROP NOT NULL;
