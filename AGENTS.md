# Agent instructions

## Commit messages

Create project commits only when the user asks. Use Conventional Commits:

```text
type(scope): short description
```

Use a lowercase type, an optional scope, and a concise description of the change.

| Change | Commit example | Release |
| --- | --- | --- |
| Bug fix | `fix(links): preserve URL fragments` | Patch |
| New feature | `feat(cli): add a preview command` | Minor |
| Breaking change | `feat(cli)!: remove the implicit build command` | Major |
| Documentation, tests, maintenance, refactoring, or performance | `docs: clarify folder titles` | Patch |

Mark every incompatible behavior change with `!` immediately before the colon, regardless of commit type. Explain the incompatibility in the body. A `BREAKING CHANGE:` or `BREAKING-CHANGE:` footer also triggers a major release:

```text
refactor(routes)!: change generated page URLs

BREAKING CHANGE: links to the previous URL format must be updated.
```

Use `fix`, `feat`, `docs`, `test`, `chore`, `refactor`, `perf`, `ci`, or `build` as appropriate. Do not label a feature as maintenance to avoid a minor release. For squash merges, keep the PR title in this format and preserve breaking-change markers in the resulting commit.

## Automatic releases

- Every push to `main` is eligible for a release. CI examines all commits since the previous stable release and applies one bump: major takes precedence over minor, which takes precedence over patch.
- `feat:` triggers minor. `!` or a breaking-change footer triggers major, including before version `1.0.0`. All other commits default to patch, so documentation and maintenance also produce releases.
- Before the first release tag, the version in `Cargo.toml` is the baseline. Rerunning a workflow at an existing release commit resumes that version instead of bumping it again.
- CI updates `Cargo.toml` and `Cargo.lock`, tests the image, commits and tags the version, and publishes the GHCR images and GitHub release with Linux AMD64/ARM64 binaries and checksums. Binaries come from the same builds as the images. Do not manually bump versions, create release tags, or publish unless explicitly instructed by the user.
- Reserve `chore(release): vX.Y.Z [skip ci]` for CI-generated version commits. Do not add skip-CI markers to normal commits.
- When changing release automation, run `python3 -m unittest discover -s scripts -p 'test_release.py'` and validate the workflow YAML and shell syntax.
