# Contributing to caddyrs

Thanks for your interest! caddyrs is early-stage, so there is plenty of high-impact work available — see the [roadmap section of the README](README.md#what-it-doesnt-do-yet).

## Development setup

You need stable Rust 1.75+ (`rustup` recommended). Then:

```bash
cargo build
cargo test
```

The test suite needs no root privileges and no external services; integration tests spawn their backends in-process on random ports and finish in about a second.

## Before opening a PR

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

CI enforces all three. A few conventions:

- Keep PRs focused — one fix or feature per PR.
- New behavior needs a test. Bug fixes need a test that fails without the fix.
- If you add a config field, wire it into `Config::validate` so it is either enforced or produces a "not implemented" warning — config fields must never be silently ignored.
- Follow [Conventional Commits](https://www.conventionalcommits.org/) for commit messages (`feat:`, `fix:`, `docs:`, `chore:`).

## Reporting bugs

Use the issue templates. For security issues, **do not open a public issue** — see [SECURITY.md](SECURITY.md).

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed under MIT and Apache-2.0, without any additional terms or conditions.
