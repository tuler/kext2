# Releasing

kext2 ships as:

| registry | package | contents |
|---|---|---|
| crates.io | [`kext2`](https://crates.io/crates/kext2) | library and CLI |
| npm | [`@tuler/kext2`](https://www.npmjs.com/package/@tuler/kext2) | JS loader and types |
| npm | `@tuler/kext2-{darwin-x64,darwin-arm64,linux-x64-gnu,linux-arm64-gnu,win32-x64-msvc}` | native binaries, pulled in as `optionalDependencies` |

All seven always share one version (a Sampo `fixed` group in `.sampo/config.toml`).
The `kext2-node` crate is internal and never published.

The crate's GitHub release (`cargo-kext2-vX.Y.Z`) also carries the standalone
`kext2` CLI, as `kext2-X.Y.Z-<target>.tar.gz` (`.zip` on Windows) for
`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`,
`aarch64-apple-darwin` and `x86_64-pc-windows-msvc`, plus `SHA256SUMS`. The
Linux binaries are fully static (musl), the Windows one links the C runtime
statically, and the macOS ones link only system libraries; the build checks this.

## Routine releases (with Sampo)

Versions and changelogs come from [Sampo](https://github.com/bruits/sampo) changesets.

1. With your change, add a changeset and commit it:

   ```bash
   npx sampo add            # or: cargo install sampo && sampo add
   ```

   Pick `cargo/kext2` (the others follow automatically) and the bump (`patch`,
   `minor`, `major`). This writes `.sampo/changesets/<name>.md`:

   ```markdown
   ---
   cargo/kext2: minor
   ---

   Add a `--foo` option.
   ```

2. Merge to `main`. The **Release** workflow opens (or refreshes) a
   **Release (main)** pull request that bumps the versions and updates the
   changelogs.
3. Merge the release PR. The workflow then builds the native binaries for every
   platform and publishes:
   - the platform packages to npm (`scripts/publish-npm.sh`);
   - then, through Sampo, the crate to crates.io and `@tuler/kext2` to npm;
   - then tags (`cargo-kext2-vX.Y.Z`, `npm-@tuler/kext2-vX.Y.Z`, …) and GitHub
     releases;
   - then attaches the CLI archives to the `cargo-kext2-vX.Y.Z` release
     (`upload-cli` job). Nothing is published unless every CLI build succeeds.

Pull requests opened with the workflow token don't trigger other workflows, so
CI doesn't run on the release PR by itself. Close and reopen it (or push to it)
if you want CI there.

Release PR preparation runs whenever a push to `main` contains pending
changesets, even while publishing is disabled. Native builds are skipped
on that push; they run after the version PR is merged and publishing is
enabled. A manual run builds the native binaries and the CLI archives
(`cli-<target>` artifacts) without creating a PR or publishing.

In **Settings → Actions → General → Workflow permissions**, enable
**Allow GitHub Actions to create and approve pull requests** so Sampo can
create the version PR with `GITHUB_TOKEN`. The default workflow permissions
can stay read-only: the PR job requests `contents: write` and
`pull-requests: write` explicitly. See
[Sampo's action setup](https://github.com/bruits/sampo/blob/main/crates/sampo-github-action/README.md)
and [GitHub's PR permission setting](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository#preventing-github-actions-from-creating-or-approving-pull-requests).

## How publishing stays safe

- **No long-lived tokens.** crates.io and npm use OIDC
  [trusted publishing](https://docs.npmjs.com/trusted-publishers). The registries
  only accept tokens minted for `tuler/kext2`, workflow `release.yml`,
  environment `release`.
- **The `release` environment only accepts the `main` branch.** Workflows from
  other branches or forks can't obtain publish tokens.
- **Least privilege.** Builds run in read-only jobs. Only the `release` job has
  `id-token: write`, and it requests registry tokens only when a release is due.
  The crates.io token expires within 30 minutes and is revoked when the job ends.
- **Pinned code.** Every action in the release job is pinned to a commit SHA.
  Sampo is installed with `cargo install --locked` at a fixed version instead of
  through its action's `curl | bash` installer.
- **Guarded packages.** A platform package can't be published without its binary
  (`prepublishOnly` check). The publish script checks all versions agree before
  publishing anything, and skips versions already on npm, so a failed run can
  be re-run.

## First release (manual)

Trusted publishing can only be configured for packages that already exist, so
0.1.0 is published by hand once. CI publishing stays off until the repository
variable `PUBLISH_ENABLED` is `true`.

You need a crates.io account logged in with `cargo login`, and an npm account with
2FA logged in with `npm login`. You also need npm ≥ 11.15 (`npm --version`) for
`npm trust`.

### 0. Prepare the repository (once)

Make the repository public first. The registries link to it, and on a
private repository npm does not record provenance, while environment
branch rules need a paid GitHub plan.

```bash
gh repo edit tuler/kext2 --visibility public --accept-visibility-change-consequences
```

Create the `release` environment so that only `main` can deploy to it:

```bash
gh api -X PUT repos/tuler/kext2/environments/release --input - <<'JSON'
{"deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}
JSON
gh api -X POST repos/tuler/kext2/environments/release/deployment-branch-policies -f name=main -f type=branch
```

Let workflows open the release PR. The default token can stay read-only,
because the jobs request what they need:

```bash
gh api -X PUT repos/tuler/kext2/actions/permissions/workflow \
  -f default_workflow_permissions=read -F can_approve_pull_request_reviews=true
```

### 1. Build the native binaries

The Linux and Windows binaries are built by CI. Running the workflow manually
only builds; it never publishes.

```bash
gh workflow run release.yml -R tuler/kext2
gh run watch -R tuler/kext2 $(gh run list -R tuler/kext2 -w release.yml -L 1 --json databaseId -q '.[0].databaseId')
rm -rf dist && gh run download -R tuler/kext2 $(gh run list -R tuler/kext2 -w release.yml -L 1 --json databaseId -q '.[0].databaseId') -D dist
ls -R dist   # binding-<platform>/kext2.<platform>.node, js-binding/index.{js,d.ts}, cli-<target>/kext2-*
```

### 2. Publish the crate

From an up-to-date clean checkout of `main`:

```bash
cargo publish -p kext2 --dry-run
cargo publish -p kext2
```

### 3. Publish the npm packages

```bash
NPM_PUBLISH_DRY_RUN=1 scripts/publish-npm.sh dist --main   # check first
scripts/publish-npm.sh dist --main
```

This publishes the five platform packages, then `@tuler/kext2`. npm asks for 2FA
on each publish; in the browser prompt you can allow skipping it for the next
5 minutes.

### 4. Trust the release workflow

npm, for all six packages:

```bash
for p in @tuler/kext2 @tuler/kext2-darwin-x64 @tuler/kext2-darwin-arm64 \
         @tuler/kext2-linux-x64-gnu @tuler/kext2-linux-arm64-gnu @tuler/kext2-win32-x64-msvc; do
  npm trust github "$p" --repo tuler/kext2 --file release.yml --env release --allow-publish --yes
  sleep 2
done
npm trust list @tuler/kext2
```

Then, for extra safety, open each package's settings on npmjs.com and set
**Publishing access** to *Require two-factor authentication and disallow tokens*.
Trusted publishing keeps working, and stolen npm tokens can no longer publish.

crates.io: go to <https://crates.io/crates/kext2/settings> → **Trusted
Publishing** → **Add**, and enter:

| field | value |
|---|---|
| Publisher | GitHub |
| Repository owner | `tuler` |
| Repository name | `kext2` |
| Workflow filename | `release.yml` |
| Environment | `release` |

### 5. Turn on CI publishing

```bash
gh variable set PUBLISH_ENABLED --body true -R tuler/kext2
```

Optionally, tag 0.1.0 the way Sampo tags later releases. Sampo only tags what it
publishes itself, so it won't do this for you:

```bash
git tag cargo-kext2-v0.1.0 && git tag npm-@tuler/kext2-v0.1.0
git push origin cargo-kext2-v0.1.0 npm-@tuler/kext2-v0.1.0
gh release create cargo-kext2-v0.1.0 -R tuler/kext2 --title "kext2 0.1.0" --notes "First release."
```

To attach the CLI binaries to that release, use the `cli-*` artifacts of the
manual run from step 1:

```bash
rm -rf cli && mkdir cli && cp dist/cli-*/kext2-* cli/
(cd cli && shasum -a 256 kext2-* > SHA256SUMS)
gh release upload cargo-kext2-v0.1.0 -R tuler/kext2 cli/*
```

From now on, follow [Routine releases](#routine-releases-with-sampo).
