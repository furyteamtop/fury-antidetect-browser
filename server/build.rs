// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Rebuild when a migration is added or edited.
//!
//! `sqlx::migrate!("./migrations")` reads the directory at COMPILE time and
//! bakes the SQL into the binary. Cargo does not know that, so a new .sql file
//! is not a reason to rebuild anything — and the result is a deployment that
//! copies the migration, builds nothing, restarts, and runs the old set.
//!
//! It happened, on the first server this was deployed to. The file was in
//! /opt/fury/src/server/migrations, the binary was fresh by every timestamp
//! cargo looks at, and the schema was one migration behind with no error
//! anywhere: `_sqlx_migrations` simply stopped at 6. Nothing in the build, the
//! service log or the deploy output said so, because from every one of their
//! points of view nothing had gone wrong.
//!
//! One line fixes it, and it is the kind of line that only gets written after
//! the afternoon it costs.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
