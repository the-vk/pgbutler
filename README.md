# pgbutler

`pgbutler` is a small agent to help you with PostgreSQL database maintenance.

[![Rust](https://github.com/the-vk/pgbutler/actions/workflows/rust.yml/badge.svg)](https://github.com/the-vk/pgbutler/actions/)
[![Crates.io](https://img.shields.io/crates/v/pgbutler.svg)](https://crates.io/crates/pgbutler)
## Local data

Connection profiles live in `~/.pgbutler/connections/<name>.toml`. Data pgbutler
tracks per server connection is stored next to them in a SQLite database,
`~/.pgbutler/connections/<name>.sqlite`.

The schema is managed with [SQLx](https://github.com/launchbadge/sqlx)
migrations from `migrations/`. They are embedded into the binary at compile
time and applied automatically whenever a connection is established, so
pgbutler ships as a single executable.
