#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
BENCH_HISTORY_COMMITS="${WHY_BENCH_HISTORY_COMMITS:-5}"
BENCH_EXTRA_FILES="${WHY_BENCH_EXTRA_FILES:-0}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
if [ "$BENCH_EXTRA_FILES" -gt 0 ]; then
  for i in $(seq 1 "$BENCH_EXTRA_FILES"); do
    cat > "src/helper_${i}.rs" <<EOF
pub fn helper_${i}(value: i32) -> i32 {
    value + ${i}
}
EOF
  done
  git add src
  git commit -m "feat: add helper modules" >/dev/null
fi

for i in $(seq 1 "$BENCH_HISTORY_COMMITS"); do
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

  if [ "$i" -eq 1 ]; then
    cat > src/metrics.rs <<'EOF'
pub fn record_migration_metric() {
    emit_metric("migration-started");
}
EOF
  elif [ "$i" -eq 3 ]; then
    cat > src/metrics.rs <<'EOF'
pub fn record_migration_metric() {
    emit_metric("migration-midpoint");
}
EOF
  fi

  if [ "$i" -eq 1 ] || [ "$i" -eq 3 ]; then
    git add src/schema.rs src/data.rs src/metrics.rs
    git commit -m "migration: schema v${i} + data migration + metrics" >/dev/null
  else
    git add src/schema.rs src/data.rs
    git commit -m "migration: schema v${i} + data migration" >/dev/null
  fi

  if [ "$BENCH_EXTRA_FILES" -gt 0 ]; then
    helper_index=$(( (i - 1) % BENCH_EXTRA_FILES + 1 ))
    cat > "src/helper_${helper_index}.rs" <<EOF
pub fn helper_${helper_index}(value: i32) -> i32 {
    value + ${helper_index} + ${i}
}
EOF
    git add "src/helper_${helper_index}.rs"
    git commit -m "refactor: refresh helper ${helper_index} (${i})" >/dev/null
  fi
done
