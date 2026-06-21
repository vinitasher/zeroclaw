#!/usr/bin/env bash
# agent-preflight.sh — pre-PR validation gate for automated (and human) contributors.
#
# Mirrors CI's fmt / lint / check / test command lines locally before a pull
# request is opened, so agentic coding pipelines catch the same class of
# failures before pushing. Local toolchains and system packages can still differ
# from the CI runner. Idempotent: safe to run repeatedly (it auto-applies
# rustfmt, everything else is read-only).
#
#   scripts/agent-preflight.sh ["<proposed PR title>"]
#
# Exit 0 = local CI-parity gates passed. Non-zero = fix the reported failures first.
# Honors CARGO_BUILD_JOBS / RUSTFLAGS from the environment.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)" || exit 2
fail=0
run() { printf '\n\033[1m==> %s\033[0m\n' "$*"; "$@" || { echo "::error::FAILED: $*"; fail=1; }; }

# 1. Format — auto-apply then verify (idempotent). Format gates all of CI.
printf '\n\033[1m==> cargo fmt --all (auto-apply)\033[0m\n'; cargo fmt --all || fail=1
run cargo fmt --all -- --check

# 2. Repo quality gate (clippy -D warnings + provider-dispatch SSOT gate).
run ./scripts/ci/rust_quality_gate.sh --strict

# 2b. CI's Lint job uses the curated ci-all feature set, not --all-features.
run cargo clippy --workspace --exclude zeroclaw-desktop --all-targets --features ci-all -- -D warnings

# 3. CI's Check matrix.
run cargo check --locked --features ci-all
run cargo check --locked --no-default-features

# 4. CI's Test job, with a cargo-nextest fallback for local machines.
if cargo nextest --version >/dev/null 2>&1; then
  run cargo nextest run --locked --workspace --exclude zeroclaw-desktop
else
  printf '\n\033[33m(note) cargo-nextest not found; falling back to cargo test with CI workspace/exclude flags.\033[0m\n'
  run cargo test --locked --workspace --exclude zeroclaw-desktop
fi

# 4b. Architecture guards from CI's Lint job.
run cargo test --test architecture tests_that_persist_config_isolate_the_path
run cargo test --test architecture user_facing_strings_route_through_fluent

# 5. PR title — Conventional Commits with scope (the `main` CI check).
if [ "${1-}" != "" ]; then
  run ./scripts/check-pr-title.sh "$1"
else
  printf '\n\033[33m(note) pass your proposed PR title as $1 to validate it: scripts/agent-preflight.sh "fix(scope): ..."\033[0m\n'
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "================================================================"
  echo " PREFLIGHT FAILED — do NOT open a PR until the above are fixed."
  echo " (Automated pipelines: treat a non-zero exit as a hard gate.)"
  echo "================================================================"
  exit 1
fi
echo "PREFLIGHT PASSED — local CI-parity gates passed; ready to open a PR."
