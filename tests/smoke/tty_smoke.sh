#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RALPH_BIN="${RALPH_BIN:-$ROOT_DIR/target/debug/ralph}"
MOCK_AGENT_BIN="${MOCK_AGENT_BIN:-$ROOT_DIR/target/debug/examples/mock-agent}"
EXPECT_BIN="${EXPECT_BIN:-expect}"
ARTIFACT_DIR="${SMOKE_ARTIFACT_DIR:-$ROOT_DIR/tests/smoke/artifacts}"

mkdir -p "$ARTIFACT_DIR"

if [[ ! -x "$RALPH_BIN" ]]; then
  echo "missing executable: $RALPH_BIN" >&2
  exit 1
fi
if [[ ! -x "$MOCK_AGENT_BIN" ]]; then
  echo "missing executable: $MOCK_AGENT_BIN" >&2
  exit 1
fi
if ! command -v "$EXPECT_BIN" >/dev/null 2>&1; then
  echo "missing expect binary: $EXPECT_BIN" >&2
  exit 1
fi

SMOKE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ralph-tty-smoke-XXXXXX")"
cleanup() {
  rm -rf "$SMOKE_DIR"
}
trap cleanup EXIT

run_pty() {
  local cmd="$1"
  EXPECT_CMD="$cmd" "$EXPECT_BIN" <<'EOF'
set timeout 45
set cmd $env(EXPECT_CMD)
spawn -noecho /usr/bin/env bash -lc $cmd
expect eof
set status [lindex [wait] 3]
exit $status
EOF
}

run_pty_with_q() {
  local cmd="$1"
  EXPECT_CMD="$cmd" "$EXPECT_BIN" <<'EOF'
set timeout 45
set cmd $env(EXPECT_CMD)
spawn -noecho /usr/bin/env bash -lc $cmd
after 1500
send -- "q"
expect eof
set status [lindex [wait] 3]
exit $status
EOF
}

echo "[tty-smoke] workspace: $SMOKE_DIR"

(
  cd "$SMOKE_DIR"
  "$RALPH_BIN" --no-ui init >"$ARTIFACT_DIR/tty-init.log" 2>&1

  run_task_id="$("$RALPH_BIN" --no-ui task add "TTY smoke run task")"
  run_agent_cmd="env MOCK_RESPONSE='<task-done>${run_task_id}</task-done>' '$MOCK_AGENT_BIN'"
  run_pty "$RALPH_BIN run $run_task_id --no-verify --agent \"$run_agent_cmd\"" \
    >"$ARTIFACT_DIR/tty-run.log" 2>&1

  "$EXPECT_BIN" "$ROOT_DIR/tests/smoke/interactive_task_create.expect" \
    "$RALPH_BIN" "$MOCK_AGENT_BIN" "$SMOKE_DIR" \
    >"$ARTIFACT_DIR/tty-task-create.log" 2>&1

  "$EXPECT_BIN" "$ROOT_DIR/tests/smoke/interactive_feature_create.expect" \
    "$RALPH_BIN" "$MOCK_AGENT_BIN" "$SMOKE_DIR" \
    >"$ARTIFACT_DIR/tty-feature-create.log" 2>&1

  tree_root_id="$("$RALPH_BIN" --no-ui task add "TTY smoke tree root")"
  tree_child_id="$("$RALPH_BIN" --no-ui task add "TTY smoke tree child" --parent "$tree_root_id")"
  dep_source_id="$("$RALPH_BIN" --no-ui task add "TTY smoke dep source")"
  dep_target_id="$("$RALPH_BIN" --no-ui task add "TTY smoke dep target")"
  "$RALPH_BIN" --no-ui task deps add "$dep_source_id" "$dep_target_id" \
    >"$ARTIFACT_DIR/tty-task-deps-add.log" 2>&1

  run_pty_with_q "$RALPH_BIN feature list" >"$ARTIFACT_DIR/tty-feature-list.log" 2>&1
  run_pty_with_q "$RALPH_BIN task list --all" >"$ARTIFACT_DIR/tty-task-list.log" 2>&1
  run_pty_with_q "$RALPH_BIN task show $tree_root_id" \
    >"$ARTIFACT_DIR/tty-task-show.log" 2>&1
  run_pty_with_q "$RALPH_BIN task tree $tree_root_id" \
    >"$ARTIFACT_DIR/tty-task-tree.log" 2>&1
  run_pty_with_q "$RALPH_BIN task deps list $dep_target_id" \
    >"$ARTIFACT_DIR/tty-task-deps-list.log" 2>&1

  echo "$tree_child_id" >"$ARTIFACT_DIR/tty-tree-child-id.txt"
)

echo "[tty-smoke] complete"
