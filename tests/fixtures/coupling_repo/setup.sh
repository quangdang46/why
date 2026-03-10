#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
for i in 1 2 3 4 5; do
  cat > src/schema.rs <<EOF
pub fn update_schema_v${i}() {
    execute_migration(SCHEMA_V${i});
}
EOF
  cat > src/data.rs <<EOF
pub fn migrate_data_v${i}() {
    transform_records(MIGRATION_V${i});
}
EOF
  git add src/schema.rs src/data.rs
  git commit -m "migration: schema v${i} + data migration" >/dev/null
done
