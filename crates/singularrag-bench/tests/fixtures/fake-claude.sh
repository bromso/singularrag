#!/bin/sh
# Stand-in for `claude` in tests. Replays $FAKE_CLAUDE_FIXTURE to stdout.
# $FAKE_CLAUDE_STDERR, when set, is written to stderr. $FAKE_CLAUDE_EXIT overrides the exit code.
# $FAKE_CLAUDE_TOUCH, when set, creates that file in the working directory (dirty-tree test).
# $FAKE_CLAUDE_ARGS_FILE, when set, receives the argument list one per line.
if [ "$1" = "--version" ]; then echo "9.9.9 (Claude Code)"; exit 0; fi
if [ -n "$FAKE_CLAUDE_ARGS_FILE" ]; then printf '%s\n' "$@" > "$FAKE_CLAUDE_ARGS_FILE"; fi
if [ -n "$FAKE_CLAUDE_TOUCH" ]; then : > "$FAKE_CLAUDE_TOUCH"; fi
if [ -n "$FAKE_CLAUDE_STDERR" ]; then echo "$FAKE_CLAUDE_STDERR" >&2; fi
if [ -n "$FAKE_CLAUDE_FIXTURE" ]; then cat "$FAKE_CLAUDE_FIXTURE"; fi
exit "${FAKE_CLAUDE_EXIT:-0}"
