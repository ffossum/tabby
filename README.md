# tabby

A pager for psql.

## Install

Needs Rust 1.85 or newer (edition 2024).

```sh
cargo install --path .     # or: cargo build --release
```

## Usage

Set the environment variable `PSQL_PAGER` to the path of the tabby binary, e.g. in
`.bashrc` or `.zshrc`:

```sh
export PSQL_PAGER=tabby
```

