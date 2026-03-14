#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/legacy.rs <<'EOF'
pub fn process_legacy_format(data: &[u8]) -> Vec<u8> {
    // TODO(2020-01-15): remove after v3 migration is complete
    // HACK: workaround for old client format, should be cleaned up
    if data.starts_with(b"LEGACY:") {
        convert_legacy_format(data)
    } else {
        data.to_vec()
    }
}
EOF
cat > src/rollout.rs <<'EOF'
pub fn staged_rollout_enabled() -> bool {
    // remove after 2020-02-01 once the staged rollout is complete
    true
}
EOF
git add src/legacy.rs src/rollout.rs
git commit -m "feat: add legacy format support and rollout gate" >/dev/null
