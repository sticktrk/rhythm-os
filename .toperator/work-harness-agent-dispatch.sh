#!/usr/bin/env bash
set -Eeuo pipefail

# Remote-side webhook handoff for Work Harness.
#
# The portal invokes this script over SSH and writes a GitHub webhook envelope to
# stdin. This script spools that envelope, resolves the local repo checkout, and
# launches Claude Code or Codex CLI detached so the SSH call can return quickly.

state_dir="${WORK_HARNESS_AGENT_STATE_DIR:-$HOME/.work-harness/agent-dispatch}"
active_cleanup_source_repo_path=""
active_cleanup_worktree_path=""

log() {
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

sanitize_id() {
  tr -cs 'A-Za-z0-9_.-' '-' | sed -e 's/^-*//' -e 's/-*$//' | cut -c1-120
}

require_command() {
  local command_name="$1"
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Required command not found on PATH: $command_name" >&2
    exit 127
  fi
}

positive_integer_or_default() {
  local value="$1"
  local fallback="$2"
  if [[ "$value" =~ ^[0-9]+$ && "$value" -gt 0 ]]; then
    printf '%s\n' "$value"
  else
    printf '%s\n' "$fallback"
  fi
}

is_safe_git_branch() {
  local branch="$1"
  [[ -n "$branch" ]] || return 1
  [[ "$branch" != -* && "$branch" != /* && "$branch" != */ && "$branch" != *. ]] || return 1
  [[ "$branch" != *..* && "$branch" != *@{* ]] || return 1
  [[ "$branch" =~ ^[A-Za-z0-9._/-]+$ ]]
}

write_dispatch_files() {
  local envelope_path="$1"
  local run_dir="$2"

  require_command node
  node - "$envelope_path" "$run_dir" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const [envelopePath, runDir] = process.argv.slice(2);
const raw = fs.readFileSync(envelopePath, "utf8").trim() || "{}";
let envelope = {};
try {
  envelope = JSON.parse(raw);
} catch (error) {
  fs.writeFileSync(path.join(runDir, "parse-error.txt"), `${error.message}\n`);
}

const portal = envelope.portal && typeof envelope.portal === "object" ? envelope.portal : {};
const payload = envelope.payload && typeof envelope.payload === "object" ? envelope.payload : {};
const repository = payload.repository && typeof payload.repository === "object" ? payload.repository : {};
const sender = payload.sender && typeof payload.sender === "object" ? payload.sender : {};

function value(input) {
  return input == null ? "" : String(input);
}

function write(name, input) {
  fs.writeFileSync(path.join(runDir, name), `${value(input)}\n`);
}

function login(input) {
  return value(input?.login || input?.authorLogin || input?.name);
}

function names(inputs) {
  return Array.isArray(inputs) ? inputs.map((item) => value(item?.name || item?.login || item?.authorLogin || item)).filter(Boolean).join(", ") : "";
}

function fencedBlock(input) {
  const text = value(input).trim();
  if (!text) return ["(empty)"];
  let fence = "```";
  while (text.includes(fence)) fence += "`";
  return [fence, text, fence];
}

function issueLines(issue) {
  if (!issue || typeof issue !== "object" || !issue.number) return [];
  return [
    "## GitHub Issue",
    `Number: #${value(issue.number)}`,
    `Title: ${value(issue.title)}`,
    `State: ${value(issue.state)}`,
    `Author: ${login(issue.user || issue)}`,
    `Created: ${value(issue.created_at || issue.createdAt)}`,
    `Updated: ${value(issue.updated_at || issue.updatedAt)}`,
    `Labels: ${names(issue.labels) || "(none)"}`,
    `Assignees: ${names(issue.assignees) || "(none)"}`,
    `Milestone: ${value(issue.milestone?.title) || "(none)"}`,
    `URL: ${value(issue.html_url || issue.htmlUrl)}`,
    "",
    "### Issue Body",
    ...fencedBlock(issue.body),
    "",
  ];
}

function issueContextLines(context) {
  if (!context || typeof context !== "object") return [];
  if (context.status === "error") {
    return [
      "## GitHub Issue Context",
      `Status: error`,
      `Error: ${value(context.error)}`,
      "",
    ];
  }
  const comments = Array.isArray(context.comments) ? context.comments : [];
  return [
    "## GitHub Issue Context",
    `Source: ${value(context.source || "webhook")}`,
    `Fetched: ${value(context.fetchedAt) || "(not fetched)"}`,
    "",
    ...issueLines(context.issue),
    ...(comments.length
      ? [
          "## Recent GitHub Issue Comments",
          ...comments.slice(0, 20).flatMap((comment, index) => [
            `### Comment ${index + 1}`,
            `Author: ${login(comment.user || comment)}`,
            `Created: ${value(comment.created_at || comment.createdAt)}`,
            `Updated: ${value(comment.updated_at || comment.updatedAt)}`,
            `URL: ${value(comment.html_url || comment.htmlUrl)}`,
            "",
            ...fencedBlock(comment.body),
            "",
          ]),
        ]
      : []),
  ];
}

function pullRequestLines(pullRequest) {
  if (!pullRequest || typeof pullRequest !== "object" || !pullRequest.number) return [];
  return [
    "## GitHub Pull Request",
    `Number: #${value(pullRequest.number)}`,
    `Title: ${value(pullRequest.title)}`,
    `State: ${value(pullRequest.state)}`,
    `Author: ${login(pullRequest.user)}`,
    `Base: ${value(pullRequest.base?.ref)}`,
    `Head: ${value(pullRequest.head?.ref)}`,
    `Draft: ${value(pullRequest.draft)}`,
    `Created: ${value(pullRequest.created_at)}`,
    `Updated: ${value(pullRequest.updated_at)}`,
    `Labels: ${names(pullRequest.labels) || "(none)"}`,
    `Assignees: ${names(pullRequest.assignees) || "(none)"}`,
    `URL: ${value(pullRequest.html_url)}`,
    "",
    "### Pull Request Body",
    ...fencedBlock(pullRequest.body),
    "",
  ];
}

function commentLines(comment) {
  if (!comment || typeof comment !== "object" || !comment.body) return [];
  return [
    "## Triggering Comment",
    `Author: ${login(comment.user)}`,
    `Created: ${value(comment.created_at)}`,
    `Updated: ${value(comment.updated_at)}`,
    `URL: ${value(comment.html_url)}`,
    "",
    "### Comment Body",
    ...fencedBlock(comment.body),
    "",
  ];
}

const owner = portal.owner || process.env.GITHUB_WEBHOOK_OWNER || "";
const repo = portal.repo || process.env.GITHUB_WEBHOOK_REPO || "";
const agentPrompt = portal.agentPrompt || "";
const agentName = portal.agentName || "";
const agentModel = portal.agentModel || "";
const agentMaxTurns = portal.agentMaxTurns || "";
const issueContext = portal.githubIssueContext && typeof portal.githubIssueContext === "object" ? portal.githubIssueContext : null;
const externalUrl = portal.externalUrl || payload.issue?.html_url || payload.pull_request?.html_url || repository.html_url || "";
const issueTitle = issueContext?.issue?.title || payload.issue?.title || payload.pull_request?.title || "";
const targetBranch = portal.targetBranch || "";

write("agent-id.txt", portal.agentId || "");
write("agent-name.txt", agentName);
write("agent-model.txt", agentModel);
write("agent-max-turns.txt", agentMaxTurns);
write("agent-prompt.txt", agentPrompt);
write("delivery-id.txt", envelope.deliveryId || process.env.GITHUB_WEBHOOK_DELIVERY_ID || "");
write("event-kind.txt", portal.eventKind || process.env.GITHUB_WEBHOOK_EVENT_KIND || "");
write("event-type.txt", envelope.eventType || process.env.GITHUB_WEBHOOK_EVENT || "");
write("external-number.txt", portal.externalNumber || process.env.GITHUB_WEBHOOK_EXTERNAL_NUMBER || "");
write("external-url.txt", externalUrl);
write("issue-title.txt", issueTitle);
write("owner.txt", owner);
write("project-id.txt", portal.projectId || process.env.GITHUB_WEBHOOK_PROJECT_ID || "");
write("repo.txt", repo);
write("repo-clone-url.txt", repository.ssh_url || repository.clone_url || "");
write("repo-default-branch.txt", repository.default_branch || "");
write("repo-id.txt", portal.matchedRepoId || "");
write("runner-sha.txt", process.env.WORK_HARNESS_RUNNER_SHA || "");
write("runner-source.txt", process.env.WORK_HARNESS_RUNNER_SOURCE || "");
write("target-branch.txt", targetBranch);

const taskLines = [
  "# Work Harness Remote Agent Dispatch",
  "",
  "You are running on the remote runtime that owns this repository checkout.",
  "Use the bound agent instructions below as your operating prompt.",
  "",
  "## Event",
  `Delivery ID: ${value(envelope.deliveryId || process.env.GITHUB_WEBHOOK_DELIVERY_ID)}`,
  `GitHub event: ${value(envelope.eventType || process.env.GITHUB_WEBHOOK_EVENT)}`,
  `Action: ${value(portal.action || process.env.GITHUB_WEBHOOK_ACTION)}`,
  `Event kind: ${value(portal.eventKind || process.env.GITHUB_WEBHOOK_EVENT_KIND)}`,
  `Repository: ${owner && repo ? `${owner}/${repo}` : value(repository.full_name)}`,
  `External item: ${value(portal.externalNumber || process.env.GITHUB_WEBHOOK_EXTERNAL_NUMBER)}`,
  `External URL: ${value(externalUrl)}`,
  `Target branch: ${value(targetBranch) || "(provider-managed)"}`,
  `Sender: ${value(sender.login || sender.name)}`,
  "",
  ...(issueContext ? issueContextLines(issueContext) : issueLines(payload.issue)),
  ...pullRequestLines(payload.pull_request),
  ...commentLines(payload.comment),
  "## Bound Agent",
  `Name: ${value(agentName)}`,
  `Model hint: ${value(agentModel)}`,
  "",
  "## Bound Agent Instructions",
  agentPrompt || "(No bound agent prompt was supplied by the portal.)",
  "",
  "## Operating Rules",
  "- Inspect the webhook envelope and the local repository before acting.",
  "- Do one bounded, useful slice of work for this event.",
  "- Do not create an infinite loop by reacting to bot-authored follow-up events.",
  "- Use native git commands for repository state, branches, diffs, commits, pushes, and logs.",
  "- Do not rely on the GitHub CLI or gh-based skills; gh may be unavailable in this remote runner.",
  "- Use the webhook envelope and GitHub Issue Context already provided in this prompt before attempting live GitHub API calls.",
  "- Prefer branch/PR/comment workflows already documented in this repository.",
  "- For issue or pull-request comment questions, make your final output only the comment you want posted back. Keep it concise and conversational.",
  "- Do not include dispatch mechanics, runner limitations, gh availability, run directories, worktree paths, or meta-explanations in final output unless the commenter specifically asks for those details.",
  "- Do not use AskUserQuestion; this runner is unattended. If clarification is required, write the question as your final output and stop.",
  "- If there is no safe action to take, write a concise summary and stop.",
  "",
  "## Files",
  `Webhook envelope JSON: ${envelopePath}`,
  `Run directory: ${runDir}`,
  "",
];

const task = taskLines.join("\n");

fs.writeFileSync(path.join(runDir, "task-prompt.md"), `${task}\n`);
NODE
}

resolved_repo_path() {
  local owner="$1"
  local repo="$2"

  if [[ -n "${WORK_HARNESS_REPO_PATH:-}" ]]; then
    printf '%s\n' "$WORK_HARNESS_REPO_PATH"
    return 0
  fi

  if [[ -n "${WORK_HARNESS_REPO_ROOT:-}" && -n "$owner" && -n "$repo" ]]; then
    printf '%s/%s/%s\n' "${WORK_HARNESS_REPO_ROOT%/}" "$owner" "$repo"
    return 0
  fi

  local script_dir
  script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  if [[ ( "$(basename "$script_dir")" == ".toperator" || "$(basename "$script_dir")" == "toperator" ) && -d "$(dirname "$script_dir")/.git" ]]; then
    dirname "$script_dir"
    return 0
  fi

  if git rev-parse --show-toplevel >/dev/null 2>&1; then
    git rev-parse --show-toplevel
    return 0
  fi

  return 1
}

clone_url_for_run() {
  local run_dir="$1"
  local owner
  local repo
  local clone_url

  if [[ -n "${WORK_HARNESS_GIT_CLONE_URL:-}" ]]; then
    printf '%s\n' "$WORK_HARNESS_GIT_CLONE_URL"
    return 0
  fi

  clone_url="$(tr -d '\r\n' < "$run_dir/repo-clone-url.txt" 2>/dev/null || true)"
  if [[ -n "$clone_url" ]]; then
    printf '%s\n' "$clone_url"
    return 0
  fi

  owner="$(tr -d '\r\n' < "$run_dir/owner.txt" 2>/dev/null || true)"
  repo="$(tr -d '\r\n' < "$run_dir/repo.txt" 2>/dev/null || true)"
  if [[ -n "$owner" && -n "$repo" ]]; then
    printf 'git@github.com:%s/%s.git\n' "$owner" "$repo"
    return 0
  fi

  return 1
}

ensure_repo_checkout() {
  local source_repo_path="$1"
  local run_dir="$2"
  local clone_url

  if [[ -d "$source_repo_path/.git" ]]; then
    return 0
  fi

  if [[ "${WORK_HARNESS_AUTO_CLONE:-false}" != "true" ]]; then
    return 0
  fi

  require_command git
  if [[ -e "$source_repo_path" && -n "$(find "$source_repo_path" -mindepth 1 -maxdepth 1 2>/dev/null | head -n 1)" ]]; then
    log "repo path exists but is not an empty directory or git checkout: $source_repo_path"
    return 2
  fi
  if ! clone_url="$(clone_url_for_run "$run_dir")"; then
    log "could not determine clone URL for missing repo checkout: $source_repo_path"
    return 2
  fi

  log "cloning missing repo checkout path=$source_repo_path"
  mkdir -p "$(dirname "$source_repo_path")"
  git clone "$clone_url" "$source_repo_path" >&2
}

ensure_repo_remote_url() {
  local source_repo_path="$1"
  local clone_url="${WORK_HARNESS_GIT_CLONE_URL:-}"
  local current_url=""

  if [[ -z "$clone_url" || ! -d "$source_repo_path/.git" ]]; then
    return 0
  fi

  require_command git
  current_url="$(git -C "$source_repo_path" remote get-url origin 2>/dev/null || true)"
  if [[ -z "$current_url" ]]; then
    git -C "$source_repo_path" remote add origin "$clone_url" >&2
    return 0
  fi
  if [[ "$current_url" != "$clone_url" ]]; then
    git -C "$source_repo_path" remote set-url origin "$clone_url" >&2
  fi
}

create_delivery_worktree() {
  local source_repo_path="$1"
  local run_dir="$2"
  local delivery_id
  local base_ref
  local target_branch
  local worktree_root
  local worktree_path

  require_command git

  delivery_id="$(basename "$run_dir")"
  base_ref="${WORK_HARNESS_WORKTREE_BASE_REF:-HEAD}"
  target_branch="$(tr -d '\r\n' < "$run_dir/target-branch.txt" 2>/dev/null || true)"
  if [[ -n "$target_branch" ]]; then
    if ! is_safe_git_branch "$target_branch"; then
      log "target branch is not a safe Git ref name: $target_branch"
      exit 2
    fi
    if git -C "$source_repo_path" rev-parse --verify --quiet "refs/remotes/origin/$target_branch" >/dev/null; then
      base_ref="origin/$target_branch"
    fi
  fi
  worktree_root="${WORK_HARNESS_WORKTREE_ROOT:-$state_dir/worktrees}"
  mkdir -p "$worktree_root"
  worktree_path="$worktree_root/$delivery_id"

  if [[ -e "$worktree_path" ]]; then
    git -C "$source_repo_path" worktree remove --force "$worktree_path" >/dev/null 2>&1 || rm -rf "$worktree_path"
  fi

  git -C "$source_repo_path" worktree prune >/dev/null 2>&1 || true
  git -C "$source_repo_path" worktree add --detach "$worktree_path" "$base_ref" >&2

  printf '%s\n' "$worktree_path" > "$run_dir/worktree-path.txt"
  printf '%s\n' "$base_ref" > "$run_dir/worktree-base-ref.txt"
  printf '%s\n' "$worktree_root" > "$run_dir/worktree-root.txt"
  printf '%s\n' "$source_repo_path" > "$run_dir/source-repo-path.txt"
  printf '%s\n' "$worktree_path"
}

cleanup_delivery_worktree() {
  local source_repo_path="$1"
  local worktree_path="$2"

  if [[ -z "$worktree_path" || ! -e "$worktree_path" ]]; then
    return 0
  fi

  if [[ "${WORK_HARNESS_DELETE_WORKTREE_ON_EXIT:-false}" != "true" ]]; then
    log "keeping delivery worktree path=$worktree_path"
    return 0
  fi

  log "removing delivery worktree path=$worktree_path"
  if ! git -C "$source_repo_path" worktree remove --force "$worktree_path"; then
    log "git worktree remove failed; deleting worktree directory path=$worktree_path"
    rm -rf "$worktree_path"
    git -C "$source_repo_path" worktree prune >/dev/null 2>&1 || true
  fi
}

select_provider() {
  local model="$1"
  local configured="${WORK_HARNESS_AGENT_PROVIDER:-auto}"

  if [[ "$configured" != "auto" ]]; then
    printf '%s\n' "$configured"
    return 0
  fi

  if [[ "$model" == claude-* ]] && command -v claude >/dev/null 2>&1; then
    printf 'claude\n'
    return 0
  fi

  if command -v codex >/dev/null 2>&1; then
    printf 'codex\n'
    return 0
  fi

  if command -v claude >/dev/null 2>&1; then
    printf 'claude\n'
    return 0
  fi

  echo "No supported agent CLI found. Install/authenticate codex or claude." >&2
  exit 127
}

run_codex() {
  local run_dir="$1"
  local repo_path="$2"
  local model="$3"
  local prompt_file="$run_dir/task-prompt.md"
  local sandbox="${WORK_HARNESS_CODEX_SANDBOX:-workspace-write}"
  local reasoning_effort="${WORK_HARNESS_CODEX_REASONING_EFFORT:-}"
  local instruction="Follow the Work Harness dispatch prompt provided on stdin."

  require_command codex

  local args=(codex exec --cd "$repo_path" --sandbox "$sandbox" --ask-for-approval never --json --output-last-message "$run_dir/final.md")
  local codex_model="$model"
  if [[ -n "${WORK_HARNESS_CODEX_MODEL:-}" ]]; then
    codex_model="$WORK_HARNESS_CODEX_MODEL"
  elif [[ "$codex_model" =~ ^(.+):(minimal|low|medium|high|xhigh)$ ]]; then
    codex_model="${BASH_REMATCH[1]}"
    reasoning_effort="${reasoning_effort:-${BASH_REMATCH[2]}}"
  fi
  if [[ -n "$codex_model" && "$codex_model" != claude-* && "$codex_model" != "inherit" ]]; then
    args+=(--model "$codex_model")
  fi
  if [[ -n "$reasoning_effort" ]]; then
    args+=(-c "model_reasoning_effort=\"$reasoning_effort\"")
  fi

  "${args[@]}" "$instruction" < "$prompt_file" > "$run_dir/codex.jsonl" 2> "$run_dir/codex.stderr.log"
}

run_claude() {
  local run_dir="$1"
  local repo_path="$2"
  local model="$3"
  local agent_name="$4"
  local prompt_file="$run_dir/task-prompt.md"
  local permission_mode="${WORK_HARNESS_CLAUDE_PERMISSION_MODE:-bypassPermissions}"
  local effort="${WORK_HARNESS_CLAUDE_EFFORT:-}"
  local max_turns="${WORK_HARNESS_CLAUDE_MAX_TURNS:-}"
  local instruction="Follow the Work Harness dispatch prompt provided on stdin."

  require_command claude

  if [[ -z "$max_turns" && -s "$run_dir/agent-max-turns.txt" ]]; then
    max_turns="$(tr -d '\r\n' < "$run_dir/agent-max-turns.txt" 2>/dev/null || true)"
  fi

  local args=(claude -p --output-format stream-json --verbose --permission-mode "$permission_mode")
  local claude_model="$model"
  if [[ "$claude_model" =~ ^(.+):(low|medium|high|xhigh|max)$ ]]; then
    claude_model="${BASH_REMATCH[1]}"
    effort="${effort:-${BASH_REMATCH[2]}}"
  fi
  if [[ -n "${WORK_HARNESS_CLAUDE_MODEL:-}" ]]; then
    args+=(--model "$WORK_HARNESS_CLAUDE_MODEL")
  elif [[ -n "$claude_model" && "$claude_model" == claude-* ]]; then
    args+=(--model "$claude_model")
  fi
  if [[ -n "$effort" ]]; then
    args+=(--effort "$effort")
  fi
  if [[ "$max_turns" =~ ^[0-9]+$ && "$max_turns" -gt 0 ]]; then
    args+=(--max-turns "$max_turns")
  fi
  if [[ "${WORK_HARNESS_CLAUDE_USE_AGENT_FLAG:-false}" == "true" && -n "$agent_name" ]]; then
    args+=(--agent "$agent_name")
  fi
  if [[ -n "${WORK_HARNESS_CLAUDE_MAX_BUDGET_USD:-}" ]]; then
    args+=(--max-budget-usd "$WORK_HARNESS_CLAUDE_MAX_BUDGET_USD")
  fi

  (cd "$repo_path" && "${args[@]}" "$instruction" < "$prompt_file" > "$run_dir/claude.stream.jsonl" 2> "$run_dir/claude.stderr.log")
}

extract_claude_final() {
  local run_dir="$1"
  local stream_path="$run_dir/claude.stream.jsonl"
  local final_path="$run_dir/final.md"

  if [[ ! -s "$stream_path" || -s "$final_path" ]]; then
    return 0
  fi

  require_command node
  node - "$stream_path" "$final_path" <<'NODE' || true
const fs = require("node:fs");

const [streamPath, finalPath] = process.argv.slice(2);
let finalText = "";
for (const line of fs.readFileSync(streamPath, "utf8").split(/\r?\n/)) {
  if (!line.trim()) continue;
  let entry;
  try {
    entry = JSON.parse(line);
  } catch {
    continue;
  }
  if (entry.type === "result" && typeof entry.result === "string" && entry.result.trim()) {
    finalText = entry.result.trim();
    continue;
  }
  const content = entry.message?.content;
  if (entry.type === "assistant" && Array.isArray(content)) {
    const text = content
      .filter((item) => item && item.type === "text" && typeof item.text === "string")
      .map((item) => item.text)
      .join("\n")
      .trim();
    if (text) finalText = text;
  }
}
if (finalText) fs.writeFileSync(finalPath, `${finalText}\n`);
NODE
}

write_completion_payload() {
  local run_dir="$1"
  local repo_path="$2"
  local exit_code="$3"
  local branch=""
  local head_sha=""
  local target_branch=""

  target_branch="$(tr -d '\r\n' < "$run_dir/target-branch.txt" 2>/dev/null || true)"
  branch="$(git -C "$repo_path" branch --show-current 2>/dev/null || true)"
  if [[ -z "$branch" && -n "$target_branch" ]]; then
    branch="$target_branch"
  fi
  head_sha="$(git -C "$repo_path" rev-parse HEAD 2>/dev/null || true)"
  printf '%s\n' "$branch" > "$run_dir/branch.txt"
  printf '%s\n' "$head_sha" > "$run_dir/head-sha.txt"

  require_command node
  node - "$run_dir" "$exit_code" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const [runDir, exitCodeRaw] = process.argv.slice(2);
const read = (name) => {
  try {
    return fs.readFileSync(path.join(runDir, name), "utf8").trim();
  } catch {
    return "";
  }
};
const finalMarkdown = read("final.md");
const summary = finalMarkdown.split(/\r?\n/).map((line) => line.trim()).find(Boolean) || "";
const exitCode = Number(exitCodeRaw);
  const externalNumber = Number(read("external-number.txt"));
  const branch = read("branch.txt");
const baseBranch = process.env.WORK_HARNESS_PR_BASE_REF || read("repo-default-branch.txt") || "main";
const requestedActions = [];

if (Number.isInteger(exitCode) && exitCode === 0 && branch && branch !== baseBranch && branch !== "HEAD") {
  requestedActions.push({
    type: "open_pull_request",
    base: baseBranch,
    head: branch,
    title: read("issue-title.txt") || `Remote agent follow-up ${read("delivery-id.txt")}`,
  });
}
if (Number.isInteger(externalNumber) && externalNumber > 0) {
  requestedActions.push({
    type: "comment_issue",
    issueNumber: externalNumber,
  });
}

const payload = {
  agentId: read("agent-id.txt"),
  agentName: read("agent-name.txt"),
  baseBranch,
  branch,
  deliveryId: read("delivery-id.txt") || path.basename(runDir),
  eventKind: read("event-kind.txt"),
  eventType: read("event-type.txt"),
  exitCode: Number.isInteger(exitCode) ? exitCode : null,
  externalNumber: Number.isInteger(externalNumber) && externalNumber > 0 ? externalNumber : null,
  externalUrl: read("external-url.txt"),
  finalMarkdown,
  headSha: read("head-sha.txt"),
  issueTitle: read("issue-title.txt"),
  owner: read("owner.txt"),
  projectId: read("project-id.txt"),
  provider: read("provider.txt"),
  repo: read("repo.txt"),
  repoId: read("repo-id.txt"),
  requestedActions,
  runDir,
  runnerSha: read("runner-sha.txt"),
  runnerSource: read("runner-source.txt"),
  sourceRepoPath: read("source-repo-path.txt"),
  status: Number.isInteger(exitCode) && exitCode === 0 ? "succeeded" : "failed",
  summary,
  worktreePath: read("worktree-path.txt"),
};

fs.writeFileSync(path.join(runDir, "completion.json"), `${JSON.stringify(payload, null, 2)}\n`);
NODE
}

push_target_branch() {
  local run_dir="$1"
  local repo_path="$2"
  local target_branch

  target_branch="$(tr -d '\r\n' < "$run_dir/target-branch.txt" 2>/dev/null || true)"
  if [[ -z "$target_branch" ]]; then
    return 0
  fi
  if ! is_safe_git_branch "$target_branch"; then
    log "target branch is not a safe Git ref name: $target_branch"
    return 2
  fi

  log "pushing target branch target=$target_branch"
  git -C "$repo_path" push origin "HEAD:refs/heads/$target_branch"
}

post_completion_callback() {
  local run_dir="$1"
  local payload_path="$run_dir/completion.json"

  if [[ -z "${WORK_HARNESS_COMPLETION_URL:-}" ]]; then
    log "completion callback skipped; WORK_HARNESS_COMPLETION_URL is not set"
    return 0
  fi
  if [[ -z "${WORK_HARNESS_COMPLETION_SECRET:-}" ]]; then
    log "completion callback skipped; WORK_HARNESS_COMPLETION_SECRET is not set"
    return 0
  fi
  if [[ ! -s "$payload_path" ]]; then
    log "completion callback skipped; completion payload is missing"
    return 0
  fi

  require_command node
  WORK_HARNESS_COMPLETION_PAYLOAD="$payload_path" node <<'NODE' > "$run_dir/completion-callback.log" 2>&1
import { createHmac } from "node:crypto";
import { readFileSync } from "node:fs";

const url = process.env.WORK_HARNESS_COMPLETION_URL;
const secret = process.env.WORK_HARNESS_COMPLETION_SECRET;
const payloadPath = process.env.WORK_HARNESS_COMPLETION_PAYLOAD;
const body = readFileSync(payloadPath);
const signature = createHmac("sha256", secret).update(body).digest("hex");

const response = await fetch(url, {
  body,
  headers: {
    "Content-Type": "application/json",
    "X-Work-Harness-Signature": `sha256=${signature}`,
  },
  method: "POST",
});
const text = await response.text();
console.log(`status=${response.status}`);
console.log(text.slice(0, 8000));
if (!response.ok) process.exit(1);
NODE
}

run_agent_unlocked() {
  local run_dir="$1"
  local source_repo_path="$2"
  local worktree_path=""
  local agent_repo_path=""
  local model
  local agent_name
  local provider

  model="$(tr -d '\r\n' < "$run_dir/agent-model.txt" 2>/dev/null || true)"
  agent_name="$(tr -d '\r\n' < "$run_dir/agent-name.txt" 2>/dev/null || true)"
  provider="$(select_provider "$model")"

  log "agent run started provider=$provider source_repo_path=$source_repo_path run_dir=$run_dir"
  printf '%s\n' "$provider" > "$run_dir/provider.txt"
  printf '%s\n' "$source_repo_path" > "$run_dir/repo-path.txt"

  ensure_repo_checkout "$source_repo_path" "$run_dir" || return 2
  if [[ ! -d "$source_repo_path/.git" ]]; then
    log "repo path is not a git checkout: $source_repo_path"
    return 2
  fi
  ensure_repo_remote_url "$source_repo_path" || return 2

  if [[ "${WORK_HARNESS_GIT_FETCH:-true}" != "false" ]]; then
    if ! git -C "$source_repo_path" fetch --all --prune; then
      log "git fetch failed; continuing with existing checkout"
    fi
  fi

  worktree_path="$(create_delivery_worktree "$source_repo_path" "$run_dir")"
  agent_repo_path="$worktree_path"
  active_cleanup_source_repo_path="$source_repo_path"
  active_cleanup_worktree_path="$worktree_path"
  trap 'cleanup_delivery_worktree "$active_cleanup_source_repo_path" "$active_cleanup_worktree_path"' EXIT
  printf '%s\n' "$agent_repo_path" > "$run_dir/agent-repo-path.txt"
  log "delivery worktree ready path=$agent_repo_path"

  local exit_code=0
  if [[ "$provider" == "claude" ]]; then
    run_claude "$run_dir" "$agent_repo_path" "$model" "$agent_name" || exit_code=$?
    extract_claude_final "$run_dir"
  elif [[ "$provider" == "codex" ]]; then
    run_codex "$run_dir" "$agent_repo_path" "$model" || exit_code=$?
  else
    log "unsupported provider: $provider"
    return 2
  fi

  if [[ "$exit_code" -eq 0 ]]; then
    push_target_branch "$run_dir" "$agent_repo_path" || exit_code=$?
  fi

  printf '%s\n' "$exit_code" > "$run_dir/exit-code.txt"
  write_completion_payload "$run_dir" "$agent_repo_path" "$exit_code" || log "failed to write completion payload"
  post_completion_callback "$run_dir" || log "completion callback failed"
  if [[ "$exit_code" -eq 0 ]]; then
    log "agent run completed provider=$provider"
  else
    log "agent run failed provider=$provider exit_code=$exit_code"
  fi
  return "$exit_code"
}

run_agent() {
  local run_dir="$1"
  local source_repo_path="$2"
  local max_concurrent
  local lock_dir
  local poll_seconds

  max_concurrent="$(positive_integer_or_default "${WORK_HARNESS_AGENT_MAX_CONCURRENT:-1}" 1)"
  lock_dir="${WORK_HARNESS_AGENT_LOCK_DIR:-$state_dir/locks}"
  poll_seconds="$(positive_integer_or_default "${WORK_HARNESS_AGENT_LOCK_POLL_SECONDS:-15}" 15)"
  mkdir -p "$lock_dir"

  if ! command -v flock >/dev/null 2>&1; then
    log "flock is not available; running without cross-dispatch concurrency enforcement"
    local unlocked_status=0
    run_agent_unlocked "$run_dir" "$source_repo_path" || unlocked_status=$?
    return "$unlocked_status"
  fi

  local waiting_logged=0
  while true; do
    local slot
    for ((slot = 1; slot <= max_concurrent; slot += 1)); do
      local lock_file="$lock_dir/agent-slot-$slot.lock"
      local lock_fd
      exec {lock_fd}>"$lock_file"
      if flock -n "$lock_fd"; then
        printf '%s\n' "$slot" > "$run_dir/agent-lock-slot.txt"
        printf '%s\n' "$max_concurrent" > "$run_dir/agent-max-concurrent.txt"
        log "agent concurrency slot acquired slot=$slot max=$max_concurrent"
        local status=0
        run_agent_unlocked "$run_dir" "$source_repo_path" || status=$?
        flock -u "$lock_fd" || true
        eval "exec ${lock_fd}>&-"
        return "$status"
      fi
      eval "exec ${lock_fd}>&-"
    done
    if [[ "$waiting_logged" == "0" ]]; then
      log "agent concurrency full max=$max_concurrent; waiting for a slot"
      waiting_logged=1
    fi
    sleep "$poll_seconds"
  done
}

queue_agent_run() {
  mkdir -p "$state_dir/runs"
  umask 077

  local delivery_id="${GITHUB_WEBHOOK_DELIVERY_ID:-manual-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
  local safe_delivery_id
  safe_delivery_id="$(printf '%s' "$delivery_id" | sanitize_id)"
  local run_dir="$state_dir/runs/${safe_delivery_id:-manual-$$}"
  mkdir -p "$run_dir"

  local envelope_path="$run_dir/envelope.json"
  cat > "$envelope_path"
  write_dispatch_files "$envelope_path" "$run_dir"

  local owner
  local repo
  owner="$(tr -d '\r\n' < "$run_dir/owner.txt" 2>/dev/null || true)"
  repo="$(tr -d '\r\n' < "$run_dir/repo.txt" 2>/dev/null || true)"

  local repo_path
  if ! repo_path="$(resolved_repo_path "$owner" "$repo")"; then
    log "could not resolve repo path; set WORK_HARNESS_REPO_PATH or WORK_HARNESS_REPO_ROOT" > "$run_dir/queue-error.log"
    exit 2
  fi

  local script_path="$0"
  if command -v realpath >/dev/null 2>&1; then
    script_path="$(realpath "$0")"
  fi

  if command -v setsid >/dev/null 2>&1; then
    setsid "$script_path" --run-agent "$run_dir" "$repo_path" > "$run_dir/runner.log" 2>&1 < /dev/null &
  else
    nohup "$script_path" --run-agent "$run_dir" "$repo_path" > "$run_dir/runner.log" 2>&1 < /dev/null &
  fi
  printf '%s\n' "$!" > "$run_dir/runner.pid"
  log "queued remote agent run run_dir=$run_dir repo_path=$repo_path pid=$(cat "$run_dir/runner.pid")"
}

if [[ "${1:-}" == "--run-agent" ]]; then
  if [[ $# -ne 3 ]]; then
    echo "Usage: $0 --run-agent RUN_DIR REPO_PATH" >&2
    exit 2
  fi
  run_agent "$2" "$3"
else
  queue_agent_run
fi
