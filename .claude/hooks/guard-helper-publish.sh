#!/usr/bin/env bash
# PreToolUse(Bash) guard (issue #347): only the main session and the agent
# types that implement work may commit, push or publish a PR. A helper - a
# fork, Explore, Plan, a reviewer - is refused, whatever its prompt says.
# Claude Code sets agent_type only for subagent calls; the main thread has none.
set -euo pipefail

input=$(cat)
agent_type=$(jq -r '.agent_type // empty' <<<"$input")
command=$(jq -r '.tool_input.command // empty' <<<"$input")

[[ -z "$agent_type" ]] && exit 0
case "$agent_type" in
  senior-software-engineer | frontend-engineer | testing-expert | devops-expert | tech-writer) exit 0 ;;
esac

publish='(^|[;&|(`[:space:]])(git([[:space:]]+-C[[:space:]]+[^[:space:]]+)?[[:space:]]+(commit|push|merge|rebase|reset|tag)|gh[[:space:]]+(pr|release|repo)[[:space:]]+(create|merge|close|edit|ready|review|comment|delete))([[:space:]]|$)'
if grep -Eq "$publish" <<<"$command"; then
  jq -n --arg t "$agent_type" '{hookSpecificOutput: {
    hookEventName: "PreToolUse",
    permissionDecision: "deny",
    permissionDecisionReason: ("Refused: a \($t) helper may not commit, push or publish (issue #347). Report your findings to the implementing agent; it makes every edit, commit, push and PR itself.")
  }}'
fi
exit 0
