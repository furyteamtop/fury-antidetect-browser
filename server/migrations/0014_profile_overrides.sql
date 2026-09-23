-- Machine fields pinned by hand over the persona: screen, cores, memory, GPU,
-- UI locale, position. One JSON object rather than six columns because the
-- server never reads inside it -- it validates the whole against the persona
-- with the shared crate and hands it on in the launch spec, and the agent
-- applies it. '{}' is "as the persona has it", which is every profile that
-- existed before this.
ALTER TABLE profiles ADD COLUMN overrides JSONB NOT NULL DEFAULT '{}'::jsonb;
