# Releasing Sakshi

A release is cut by pushing a version tag. CI does the building and packaging;
the Homebrew tap and Scoop bucket are updated automatically once the release
exists (see section 3). Only winget is left to a maintainer, because it is a
pull request to Microsoft's repository.

## 1. Prepare

1. Pick the version, e.g. `0.2.0`.
2. Set `version` under `[workspace.package]` in `Cargo.toml` to it, and refresh
   the lockfile: `cargo update --workspace`.
3. Move the `Unreleased` notes in `CHANGELOG.md` under the new version.
4. Make sure `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
   and `cargo test` pass, then merge to `main` through a pull request.

## 2. Tag

```sh
git switch main && git pull
git tag v0.2.0
git push origin v0.2.0
```

The [release workflow](../.github/workflows/release.yml):

1. checks the tag matches the version in `Cargo.toml` (it refuses to publish if
   they differ);
2. builds `--release --locked` for macOS (ARM and Intel), Linux (x86-64 and
   ARM64) and Windows (x86-64 and ARM64) and runs `sakshi --version` on each build that
   can run on its runner;
3. packages `sakshi-vX.Y.Z-<target>.tar.gz` (a folder holding `sakshi`,
   `LICENSE`, `README.md`) or `.zip` on Windows (the same files, unfolded),
   each with a SHA-256;
4. renders the Homebrew, Scoop and winget manifests from the templates in
   `packaging/` with the real checksums;
5. creates the GitHub release with the archives, `SHA256SUMS`,
   `package-manifests.zip` and generated notes.

Pull requests that touch the workflow or `packaging/` run the build and
package steps without publishing, so changes there are tested first. The
manifest renderer has its own tests: `python3 -m unittest discover -s packaging`.

If a run fails after the tag is pushed, delete the tag (and any partial
release), fix the problem, and tag again.

## 3. Publish to package managers

### Automatic: Homebrew tap and Scoop bucket

After the release is created, the `update tap and bucket` job in the release
workflow downloads `package-manifests.zip` from it and pushes
`homebrew/sakshi.rb` to `maxsignalhq/homebrew-tap` (`Formula/sakshi.rb`) and
`scoop/sakshi.json` to `maxsignalhq/scoop-bucket` (`bucket/sakshi.json`). It
is a separate job, so if it fails the release is unaffected: re-run the job.

**One-time setup (done for 0.1.0).** Each repository has a write-enabled
deploy key ("sakshi release workflow") under its Settings, Deploy keys. The
private half of each is an Actions secret on the Sakshi repository:
`TAP_DEPLOY_KEY` for `homebrew-tap` and `BUCKET_DEPLOY_KEY` for
`scoop-bucket`. A deploy key can push to its one repository and nothing else,
and it does not expire. Without the secrets the job reports a notice and
succeeds without pushing anything, and you publish by hand as below.

To rotate a key: `ssh-keygen -t ed25519 -N "" -f key`, remove the old deploy key
on the repository and add `key.pub` with write access
(`gh repo deploy-key add key.pub -R maxsignalhq/<repo> -w`), then
`gh secret set <NAME> -R maxsignalhq/Sakshi < key` and delete the key file.

### By hand

Use this if the job is skipped or fails. Download `package-manifests.zip` from
the release. Each of these is a separate repository, so each needs someone with
access to it.

**Homebrew.** Put `homebrew/sakshi.rb` at `Formula/sakshi.rb` in a tap
repository (conventionally `maxsignalhq/homebrew-tap`). Users then run
`brew install maxsignalhq/tap/sakshi`. Check it first with
`brew audit --strict --new maxsignalhq/tap/sakshi` and `brew test sakshi`.

**Scoop.** Put `scoop/sakshi.json` at `bucket/sakshi.json` in a bucket
repository (e.g. `maxsignalhq/scoop-bucket`). Users run
`scoop bucket add maxsignalhq https://github.com/maxsignalhq/scoop-bucket`
then `scoop install sakshi`. The manifest has `checkver` and `autoupdate`, so
Scoop's own tooling can keep the bucket current afterwards.

**winget.** Open a pull request to
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) adding the
three `winget/*.yaml` files under
`manifests/m/MaxSignalHQ/Sakshi/<version>/`. Validate locally with
`winget validate --manifest <folder>` first. The package is a portable zip, so
users get a `sakshi` command with `winget install MaxSignalHQ.Sakshi`.

## What is not automated, and why

Pushing the tag publishes a release, so a person does it on purpose. The tap
and bucket update follows from the tag automatically, using the deploy keys
above. The winget pull request stays manual.
