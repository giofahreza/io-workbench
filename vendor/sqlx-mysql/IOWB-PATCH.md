# Narrow SQLx 0.8.0 MySQL type-info backport

Upstream: https://github.com/launchbadge/sqlx, package `sqlx-mysql` 0.8.0.
This directory was copied from the already locked crates.io package (SHA-256
`5afe4c38a9b417b6a9a5eeffe7235d0a106716495536e7727d1c7f4b1ff3eba6`).
The packaged `.cargo_vcs_info.json` is retained. The two packaged license-path
placeholders have been replaced with full MIT and Apache-2.0 license texts from
the matching `sqlx` 0.8.0 package. No dependency major/minor version is changed.

Only `src/type_info.rs` contains a functional source change. MySQL's BINARY flag
also marks textual collations such as `utf8mb4_bin`; it does not by itself mean
that the value is binary. The protocol already supplies a character-set number,
but SQLx0.8 discards it when constructing TypeInfo. The backport clears that
overloaded flag for text/string/blob-family TypeInfo only when the wire charset
is not binary63. Existing type naming and String/byte compatibility then work
correctly across arbitrary SQL, table reads, transfers and locked row snapshots.
Original protocol/column flags, database collation, and numeric/JSON/spatial
types are unchanged. Actual binary63 values remain binary even when valid UTF-8.

Two driver unit regressions cover binary63 versus45/46/255 textual collations,
String compatibility, type names and preservation of unrelated flags. Server
integration regressions additionally exercise actual MySQL/MariaDB textual
columns, JSON aliases, native JSON, JSON-looking BLOB bytes and transfers.

This local patch can be removed when the repository deliberately migrates to a
driver release that preserves equivalent charset-aware decoding. Do not remove
it solely because the generic BINARY-flag path compiles or basic ASCII tests pass.
