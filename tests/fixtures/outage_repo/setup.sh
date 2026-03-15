#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
BENCH_HISTORY_COMMITS="${WHY_BENCH_HISTORY_COMMITS:-0}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src docs
cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    token.starts_with("secure-")
}
EOF
cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 1
}
EOF
GIT_AUTHOR_DATE='2024-01-01T12:00:00Z' GIT_COMMITTER_DATE='2024-01-01T12:00:00Z' git add src/auth.rs src/util.rs
GIT_AUTHOR_DATE='2024-01-01T12:00:00Z' GIT_COMMITTER_DATE='2024-01-01T12:00:00Z' git commit -m 'feat: add auth and util helpers' >/dev/null

cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 2
}
EOF
GIT_AUTHOR_DATE='2024-01-02T12:00:00Z' GIT_COMMITTER_DATE='2024-01-02T12:00:00Z' git add src/util.rs
GIT_AUTHOR_DATE='2024-01-02T12:00:00Z' GIT_COMMITTER_DATE='2024-01-02T12:00:00Z' git commit -m 'fix: adjust util helper' >/dev/null

cat > src/auth.rs <<'EOF'
pub fn verify_token(token: &str) -> bool {
    // security: outage rollback guard
    token.starts_with("secure-") && token.len() > 3
}
EOF
cat > src/util.rs <<'EOF'
pub fn helper(value: i32) -> i32 {
    value + 3
}
EOF
GIT_AUTHOR_DATE='2024-01-03T12:00:00Z' GIT_COMMITTER_DATE='2024-01-03T12:00:00Z' git add src/auth.rs src/util.rs
GIT_AUTHOR_DATE='2024-01-03T12:00:00Z' GIT_COMMITTER_DATE='2024-01-03T12:00:00Z' git commit -m 'hotfix: rollback auth guard after outage (#42)' >/dev/null

cat > docs/runbook.md <<'EOF'
incident notes
EOF
GIT_AUTHOR_DATE='2024-01-04T12:00:00Z' GIT_COMMITTER_DATE='2024-01-04T12:00:00Z' git add docs/runbook.md
GIT_AUTHOR_DATE='2024-01-04T12:00:00Z' GIT_COMMITTER_DATE='2024-01-04T12:00:00Z' git commit -m 'docs: add runbook notes' >/dev/null

for i in $(seq 1 "$BENCH_HISTORY_COMMITS"); do
  cat > src/auth.rs <<EOF
pub fn verify_token(token: &str) -> bool {
    // security: outage rollback guard
    // benchmark history marker ${i}
    token.starts_with("secure-") && token.len() > 3 && token.len() >= ${i}
}
EOF
  GIT_AUTHOR_DATE='2024-01-05T12:00:00Z' GIT_COMMITTER_DATE='2024-01-05T12:00:00Z' git add src/auth.rs
  GIT_AUTHOR_DATE='2024-01-05T12:00:00Z' GIT_COMMITTER_DATE='2024-01-05T12:00:00Z' git commit -m "maintenance: refine outage auth guard ${i}" >/dev/null
done
