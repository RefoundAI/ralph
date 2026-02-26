#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RALPH_BIN="${RALPH_BIN:-$ROOT_DIR/target/debug/ralph}"
MOCK_AGENT_BIN="${MOCK_AGENT_BIN:-$ROOT_DIR/target/debug/examples/mock-agent}"
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

SMOKE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ralph-non-tty-smoke-XXXXXX")"
cleanup() {
  rm -rf "$SMOKE_DIR"
}
trap cleanup EXIT

echo "[non-tty-smoke] workspace: $SMOKE_DIR"

(
  cd "$SMOKE_DIR"
  "$RALPH_BIN" --no-ui init >"$ARTIFACT_DIR/non-tty-init.log" 2>&1

  run_task_id="$("$RALPH_BIN" --no-ui task add "Non-TTY smoke run task")"
  run_agent_cmd="env MOCK_RESPONSE='<task-done>${run_task_id}</task-done>' '$MOCK_AGENT_BIN'"

  "$RALPH_BIN" run "$run_task_id" --no-verify --agent "$run_agent_cmd" \
    >"$ARTIFACT_DIR/non-tty-run-stdout.log" 2>"$ARTIFACT_DIR/non-tty-run-stderr.log"

  "$RALPH_BIN" --no-ui task list --all >"$ARTIFACT_DIR/non-tty-task-list.log" 2>&1
)

if LC_ALL=C grep -Fq $'\x1b[?1049h' "$ARTIFACT_DIR/non-tty-run-stdout.log"; then
  echo "alternate-screen escape sequence found in non-tty stdout" >&2
  exit 1
fi

if ! grep -q "Tasks complete." "$ARTIFACT_DIR/non-tty-run-stdout.log"; then
  echo "expected completion line missing from non-tty stdout" >&2
  exit 1
fi

# Strip ANSI escape sequences for pattern matching
sed 's/\x1b\[[0-9;]*m//g' "$ARTIFACT_DIR/non-tty-run-stderr.log" \
  >"$ARTIFACT_DIR/non-tty-run-stderr-plain.log"

if ! grep -q '\[task\]' "$ARTIFACT_DIR/non-tty-run-stderr-plain.log"; then
  echo "expected [task] event missing from non-tty stderr" >&2
  exit 1
fi

if ! grep -q '\[dag\]' "$ARTIFACT_DIR/non-tty-run-stderr-plain.log"; then
  echo "expected [dag] event missing from non-tty stderr" >&2
  exit 1
fi

if ! grep -q "t-" "$ARTIFACT_DIR/non-tty-task-list.log"; then
  echo "expected plain task list output missing in non-tty mode" >&2
  exit 1
fi

echo "[non-tty-smoke] complete"
