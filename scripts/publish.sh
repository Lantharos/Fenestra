#!/usr/bin/env bash
set -euo pipefail

version="${1:-}"
if [[ -z "$version" ]]; then
  echo "usage: scripts/publish.sh <version>" >&2
  exit 2
fi

tag="v${version#v}"
workspace_version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
public_version="${workspace_version%.0}"
if [[ "$tag" != "v$public_version" ]]; then
  echo "tag $tag does not match public workspace version $public_version" >&2
  exit 1
fi
if [[ "$(git branch --show-current)" != "main" ]]; then
  echo "publishing requires the main branch" >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "publishing requires a clean worktree" >&2
  exit 1
fi

cargo fmt --all --check
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
if command -v actionlint >/dev/null 2>&1; then
  actionlint
fi
if command -v gh >/dev/null 2>&1; then
  if ! gh secret list --repo Lantharos/Sabine | grep -q '^SABINE_UPDATE_SIGNING_KEY[[:space:]]'; then
    echo "GitHub secret SABINE_UPDATE_SIGNING_KEY is not configured" >&2
    exit 1
  fi
  immutable="$(gh api repos/Lantharos/Sabine/immutable-releases -H 'X-GitHub-Api-Version: 2026-03-10' --jq .enabled)"
  if [[ "$immutable" != "true" ]]; then
    echo "immutable GitHub Releases must be enabled" >&2
    exit 1
  fi
fi
git fetch origin main --tags
if [[ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]]; then
  echo "local main must exactly match origin/main" >&2
  exit 1
fi
if git rev-parse "$tag" >/dev/null 2>&1; then
  echo "$tag already exists" >&2
  exit 1
fi

git tag -s "$tag" -m "Sabine $public_version"
git push origin "$tag"
