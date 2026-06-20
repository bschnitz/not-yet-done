#!/usr/bin/env bash
#
# C6 — one-shot migration of the task domain from the legacy single
# database (`nyd.db`) into the task adapter's own database (`tasks.db`).
#
# Background: until the DB split (phase C), tasks, trackings, projects and
# tags lived in the same SQLite file as the TUI app-shell tables
# (link / settings / saved_query / query_shortcut). C2 taught the schema
# sync to populate a fresh `tasks.db` (the local adapter's default DSN),
# but that file starts EMPTY. This script copies the existing task-domain
# rows from `nyd.db` into `tasks.db` so the adapter-backed Tasks/Trackings
# tabs show the real history.
#
# What it does NOT touch:
#   * `nyd.db` is opened READ-ONLY and left completely intact — it stays
#     as the safety net (and the TUI/CLI app-shell still reads it).
#   * The app-shell tables (link/settings/saved_query/query_shortcut) and
#     adapter caches (jira_*, etc.) are NOT migrated — they belong to
#     `nyd.db`.
#
# Safety:
#   * Refuses to run if the target task tables are non-empty (prevents a
#     double-migration / accidental merge). Wipe `tasks.db` or restore the
#     pre-migration backup to re-run.
#   * Backs up the target `tasks.db` (consistent `.backup` copy) before
#     writing.
#   * Verifies row counts match per table afterwards; aborts on mismatch.
#
# ⚠ Before running: stop any in-progress tracking and close the TUI so
#   nothing writes to `tasks.db` concurrently.
#
# Usage:
#   migrate-from-legacy-db.sh [SRC_DB] [DST_DB]
# Defaults:
#   SRC_DB = $XDG_DATA_HOME/not_yet_done/nyd.db   (~/.local/share/...)
#   DST_DB = $XDG_DATA_HOME/not_yet_done/tasks.db

set -euo pipefail

data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/not_yet_done"
SRC="${1:-$data_dir/nyd.db}"
DST="${2:-$data_dir/tasks.db}"

# Task-domain tables, ordered so referenced rows are inserted before the
# rows that reference them (foreign-key-safe even when enforcement is on).
TABLES=(
  project
  global_tag
  project_tag
  task
  tracking
  task_global_tag
  task_project
  task_project_tag
)

die() { echo "migrate: ERROR: $*" >&2; exit 1; }

command -v sqlite3 >/dev/null || die "sqlite3 not found in PATH"
[ -f "$SRC" ] || die "source database not found: $SRC"
[ -f "$DST" ] || die "target database not found: $DST (open the adapter once to create it, or run the TUI)"

echo "migrate: source (read-only): $SRC"
echo "migrate: target:             $DST"
echo

# --- Guard: target task tables must be empty -------------------------------
existing=0
for t in "${TABLES[@]}"; do
  n=$(sqlite3 "$DST" "SELECT count(*) FROM $t;" 2>/dev/null || echo 0)
  existing=$((existing + n))
done
[ "$existing" -eq 0 ] || die "target task tables already hold $existing rows — refusing to merge. Restore the pre-migration backup or wipe tasks.db to re-run."

# --- Backup the target before writing --------------------------------------
ts=$(date +%Y%m%d-%H%M%S)
backup="$DST.pre-c6-$ts.bak"
sqlite3 "$DST" ".backup '$backup'"
echo "migrate: backed up target → $backup"
echo

# --- Copy each table with an explicit column list --------------------------
# Explicit columns (not SELECT *) so a future column-order change can't
# silently scramble values; columns are read from the target schema.
for t in "${TABLES[@]}"; do
  cols=$(sqlite3 "$DST" "SELECT group_concat(name, ', ') FROM pragma_table_info('$t');")
  [ -n "$cols" ] || die "target table '$t' has no columns (schema not synced?)"
  sqlite3 "$DST" <<SQL
PRAGMA foreign_keys = OFF;
ATTACH DATABASE 'file:$SRC?mode=ro' AS legacy;
INSERT INTO main.$t ($cols) SELECT $cols FROM legacy.$t;
DETACH DATABASE legacy;
SQL
  src_n=$(sqlite3 "$SRC" "SELECT count(*) FROM $t;")
  dst_n=$(sqlite3 "$DST" "SELECT count(*) FROM $t;")
  printf "migrate: %-20s %5s rows\n" "$t" "$dst_n"
  [ "$src_n" -eq "$dst_n" ] || die "row-count mismatch for '$t': source=$src_n target=$dst_n (target backup at $backup)"
done

echo
echo "migrate: done. nyd.db left untouched as a safety net."
echo "migrate: target backup (pre-migration, empty): $backup"
