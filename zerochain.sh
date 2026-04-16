#!/usr/bin/env bash
# zerochain.sh — proof of concept: multi-agent orchestration via mkdir
set -euo pipefail

BLUE='\033[0;34m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

WORKSPACE="${ZEROCHAIN_WORKSPACE:-./workspace}"

usage() {
  cat <<EOF
Usage: $(basename "$0") <command> [options]

Commands:
  init <name> [--stages "s1 s2 ..."]   Create a workflow with numbered stages
  run <name> [--stage <stage>]         Execute the next (or specified) stage
  status <name>                        Show workflow stage states
  list                                 List all workflows
  approve <name> <stage>               Remove human gate from a stage
  reject <name> <stage> [reason]       Mark stage as error

Environment:
  ZEROCHAIN_WORKSPACE   Workspace root (default: ./workspace)
EOF
}

die() { echo -e "${RED}Error: $*${NC}" >&2; exit 1; }

ensure_workspace() { mkdir -p "$WORKSPACE"; }

# --- helpers ---

get_stages() {
  local dir="$1"
  local -n _stages="$2"
  _stages=()
  [[ -d "$dir" ]] || return 0
  while IFS= read -r d; do
    [[ -d "$d" ]] || continue
    local base; base=$(basename "$d")
    [[ "$base" =~ ^[0-9]{2}[_a-z] ]] || continue
    _stages+=("$d")
  done < <(printf '%s\n' "$dir"/*/ | sort)
}

stage_state() {
  local d="$1"
  [[ -e "$d/.complete" ]]  && echo "complete"  && return
  [[ -e "$d/.error" ]]     && echo "error"     && return
  [[ -e "$d/.executing" ]] && echo "executing" && return
  [[ -e "$d/.human_gate" ]] && echo "waiting"  && return
  echo "pending"
}

stage_order() {
  local d="$1"
  basename "$d" | sed 's/^0*//' | sed 's/[^0-9].*//'
}

stage_name() {
  basename "$1" | sed 's/^[0-9]*[_a-z]*_//'
}

fmt_state() {
  case "$1" in
    complete)  echo -e "${GREEN}complete${NC}" ;;
    error)     echo -e "${RED}error${NC}" ;;
    executing) echo -e "${YELLOW}executing${NC}" ;;
    waiting)   echo -e "${BLUE}waiting for approval${NC}" ;;
    *)         echo -e "${BLUE}pending${NC}" ;;
  esac
}

deps_met() {
  local wf="$1" target="$2"
  local order
  order=$(stage_order "$target")
  [[ "$order" =~ ^[0-9]+$ ]] || return 0
  for prev in "$wf"/[0-9][0-9]*_/; do
    [[ -d "$prev" ]] || continue
    local po; po=$(stage_order "$prev")
    (( 10#$po < 10#$order )) || continue
    [[ -e "$prev/.complete" ]] || return 1
  done
  return 0
}

# --- commands ---

cmd_init() {
  local name="$1"; shift
  local stages=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --stages) read -ra stages <<< "$2"; shift 2 ;;
      *) shift ;;
    esac
  done
  ((${#stages[@]} > 0)) || stages=(research design implement review)

  ensure_workspace
  local wf="$WORKSPACE/$name"
  [[ ! -d "$wf" ]] || die "Workflow '$name' already exists"
  mkdir -p "$wf"

  local i=1
  for s in "${stages[@]}"; do
    local norm; norm=$(echo "$s" | tr '[:upper:]' '[:lower:]' | tr ' ' '_')
    local prefix; prefix=$(printf "%02d" "$i")
    local sd="$wf/${prefix}_${norm}"
    mkdir -p "$sd/input" "$sd/output"
    cat > "$sd/CONTEXT.md" <<STAGE
---
name: $s
order: $prefix
---

# $s

Describe what this stage does here.
STAGE
    i=$((i + 1))
  done

  echo -e "${GREEN}Initialized${NC} workflow '$name' (${#stages[@]} stages)"
}

cmd_run() {
  local name="$1"; shift
  local target=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --stage) target="$2"; shift 2 ;;
      *) shift ;;
    esac
  done

  ensure_workspace
  local wf="$WORKSPACE/$name"
  [[ -d "$wf" ]] || die "Workflow '$name' not found"

  local stages=()
  get_stages "$wf" stages

  if [[ -n "$target" ]]; then
    # Find exact or partial match
    for s in "${stages[@]}"; do
      if [[ "$(basename "$s")" == *"$target"* ]]; then
        target="$s"; break
      fi
    done
    [[ -d "$target" ]] || die "Stage '$target' not found"
  else
    # Find next pending stage
    for s in "${stages[@]}"; do
      local st; st=$(stage_state "$s")
      [[ "$st" != "complete" && "$st" != "error" ]] && target="$s" && break
    done
    [[ -n "$target" ]] || die "All stages complete"
  fi

  deps_met "$wf" "$target" || die "Dependencies not met for $(basename "$target")"

  if [[ -e "$target/.human_gate" ]]; then
    die "Stage '$(basename "$target")' requires approval — run: $(basename "$0") approve $name $(basename "$target")"
  fi

  # --- execution loop (LLM would plug in here) ---
  mkdir -p "$target/output"
  touch "$target/.executing"
  echo -e "${YELLOW}Executing${NC} $(basename "$target")..."
  sleep 0.1  # placeholder: actual work happens here
  # --- end execution loop ---

  rm -f "$target/.executing"
  touch "$target/.complete"
  echo -e "${GREEN}Complete${NC}: $(basename "$target")"
}

cmd_status() {
  local name="$1"
  local wf="$WORKSPACE/$name"
  [[ -d "$wf" ]] || die "Workflow '$name' not found"

  echo "Workflow: $name"
  local stages=()
  get_stages "$wf" stages

  local complete=0 error=0 total=${#stages[@]}
  for s in "${stages[@]}"; do
    local st; st=$(stage_state "$s")
    [[ "$st" == "complete" ]] && ((complete++))
    [[ "$st" == "error" ]]    && ((error++))
    printf "  %-20s %s\n" "$(basename "$s")" "$(fmt_state "$st")"
  done

  local pending=$((total - complete - error))
  echo -e "  ${NC}---"
  echo "  $complete complete, $pending pending, $error error"
}

cmd_list() {
  ensure_workspace
  for wf in "$WORKSPACE"/*/; do
    [[ -d "$wf" ]] || continue
    local name; name=$(basename "$wf")
    local stages=() complete=0
    get_stages "$wf" stages
    for s in "${stages[@]}"; do
      [[ -e "$s/.complete" ]] && ((complete++))
    done
    echo -e "$name ${GREEN}$complete${NC}/${#stages[@]} stages"
  done
}

cmd_approve() {
  local name="$1" stage="$2"
  local wf="$WORKSPACE/$name"
  [[ -d "$wf" ]] || die "Workflow '$name' not found"
  [[ -d "$wf/$stage" ]] || die "Stage '$stage' not found"
  rm -f "$wf/$stage/.human_gate"
  echo -e "${GREEN}Approved${NC} $stage"
}

cmd_reject() {
  local name="$1" stage="$2"; shift 2
  local reason="${*:-}"
  local wf="$WORKSPACE/$name"
  [[ -d "$wf" ]] || die "Workflow '$name' not found"
  [[ -d "$wf/$stage" ]] || die "Stage '$stage' not found"
  rm -f "$wf/$stage/.executing" "$wf/$stage/.human_gate"
  [[ -n "$reason" ]] && echo "$reason" > "$wf/$stage/.error" || touch "$wf/$stage/.error"
  echo -e "${RED}Rejected${NC} $stage"
}

# --- main ---

(($# < 1)) && usage && exit 1

case "$1" in
  init)    (($# < 2)) && usage && exit 1; cmd_init "${@:2}" ;;
  run)     (($# < 2)) && usage && exit 1; cmd_run "${@:2}" ;;
  status)  (($# < 2)) && usage && exit 1; cmd_status "$2" ;;
  list)    cmd_list ;;
  approve) (($# < 3)) && usage && exit 1; cmd_approve "$2" "$3" ;;
  reject)  (($# < 3)) && usage && exit 1; cmd_reject "${@:2}" ;;
  *)       usage && exit 1 ;;
esac
