# Contributing to Orbyn Forge

Thanks for helping. Forge is developed in the open at
[`orbyntech/forge`](https://github.com/orbyntech/forge). All changes arrive as
pull requests from forks. Nobody pushes to the main repository directly.

## Workflow

1. **Fork** `orbyntech/forge` on GitHub (the **Fork** button).
2. **Clone your fork** and add the main repository as `upstream`:
   ```sh
   git clone https://github.com/<your-user>/forge.git
   cd forge
   git remote add upstream https://github.com/orbyntech/forge.git
   ```
3. **Branch from `master`**:
   ```sh
   git fetch upstream
   git switch -c <topic> upstream/master
   ```
4. **Make the change, with docs.** Read [`CLAUDE.md`](CLAUDE.md) for the
   architecture, security and code-quality rules. Reviews apply them.
5. **Run the checks**:
   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo deny check
   docker run --rm -v "$PWD":/repo zricethezav/gitleaks:v8.21.2 dir /repo --no-banner
   (cd src/ui && npm ci && npm run build)
   (cd docs && npm ci && npm run build)
   ```
6. **Push to your fork** and open a pull request into `orbyntech/forge:master`.
   Fill in the PR template.

The first time you contribute, CI on your PR waits for a maintainer to approve it.
`master` is the only long-lived branch. Pull requests into it are
squash-merged, and CI runs on every pull request and every push to `master`.
Releases are tags on `master` (`vX.Y.Z`, or `vX.Y.Z-rc-NN` for a release
candidate); pushing a tag runs the release pipeline.

## Guidelines

- Keep PRs focused. Write small commits with imperative subjects.
- Every behaviour, config, API, CLI or UI change updates `docs/` in the same PR.
- New dependencies must be MIT, Apache-2.0, BSD or another license `deny.toml`
  allows. No OpenSSL. Open an issue first for anything large.
- Don't commit real hostnames, IP addresses, usernames or secrets. Use
  placeholders, `lab-a` … `lab-z` for test machines, and `192.0.2.x` for
  example addresses.

## Reporting bugs and ideas

Use [GitHub Issues](https://github.com/orbyntech/forge/issues). For security
problems, follow [`SECURITY.md`](SECURITY.md) instead.

## License

By contributing, you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
