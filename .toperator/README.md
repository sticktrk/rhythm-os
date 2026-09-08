# .toperator

This directory is maintained by the DT Concepts Work Harness portal for sticktrk/rhythm-os.

## Files

- `work-harness-agent-dispatch.sh`: remote SSH webhook handoff script. It reads the portal webhook envelope from stdin, spools a run directory, resolves this repository checkout, creates an isolated per-delivery Git worktree, and launches Claude Code or Codex CLI detached in that worktree.
- `install-agent-dispatch.sh`: optional helper to install the dispatch script as `/usr/local/bin/work-harness-agent-dispatch` on the runtime host.

Each run writes `task-prompt.md` with event routing plus inline GitHub Issue, Pull Request, and triggering comment details when those objects are present in the webhook payload. The full raw webhook envelope is also saved as `envelope.json`. The source checkout path, agent worktree path, and worktree base ref are written into the run directory for debugging and follow-up.
For plain repo-path dispatch commands, the portal sends the current runner script inline over SSH on every webhook delivery, stages it under `~/.work-harness/agent-dispatch/bin/work-harness-agent-dispatch-<sha>.sh`, and executes that staged copy. This keeps live dispatch from depending on a previously synced repo-local script. The synced `.toperator/` copy remains useful for transparency, manual tests, and custom command overrides.
When `PORTAL_AGENT_DISPATCH_COMPLETION_SECRET` is configured on the portal, SSH dispatch injects `WORK_HARNESS_COMPLETION_URL` and `WORK_HARNESS_COMPLETION_SECRET` into the staged runner environment. The script then posts a signed `completion.json` callback to the portal after the provider exits. The portal-side orchestrator uses the Project's editable Dispatch Orchestrator Prompt plus GitHub App credentials to open or reuse a Pull Request for a pushed branch, add it to the repo GitHub Project, and comment back on the triggering issue. The remote host should not receive GitHub App keys or rely on `gh`.

## Repo command

When the portal SSH target lands directly on this checkout, the per-Repo Connections command can be:

```txt
/srv/client-repos/OWNER/REPO
```

The portal treats a plain path as the remote repository checkout directory and runs the dispatch script from
the current portal-shipped runner staged under the SSH user's Work Harness state directory. To override the command completely and use the synced repo script directly, provide the full command:

```txt
WORK_HARNESS_REPO_PATH=/srv/client-repos/OWNER/REPO /srv/client-repos/OWNER/REPO/.toperator/work-harness-agent-dispatch.sh
```

Project Settings can force this repo onto the shared worker with `ssh_dispatch_mode=shared_ssh`, force the Repo/Project
runtime target with `repo_ssh`, or follow the portal default with `env_default`. Shared-worker dispatch sends agent work
to `PORTAL_AGENT_WORKER_SSH_TARGET`. The default shared worker layout is `/srv/work-harness/repos/OWNER/REPO` on the
`toperator-agent-worker` runtime user. The runner can auto-clone a missing checkout from a repo-specific SSH host alias
such as `git@work-harness-OWNER-REPO:OWNER/REPO.git` when `WORK_HARNESS_AUTO_CLONE=true`. The portal generates one
deploy key per connected repository whose Project effectively uses the shared worker, stores the private key on the
non-root worker user, and adds that public key to GitHub with the App's repository Administration permission.

For manual/local script tests, include these environment variables:

```txt
WORK_HARNESS_COMPLETION_URL=https://portal.example.com/api/agent-dispatch/completions
WORK_HARNESS_COMPLETION_SECRET=shared-hmac-secret
WORK_HARNESS_AGENT_MAX_CONCURRENT=1
WORK_HARNESS_AUTO_CLONE=true
```

The runtime host must have `node` plus authenticated `codex` and/or `claude` available on the SSH user's PATH.
It must also have `git`; agent execution always runs in a retained per-delivery worktree under `~/.work-harness/agent-dispatch/worktrees/` unless `WORK_HARNESS_WORKTREE_ROOT` is set.
The runner enforces `WORK_HARNESS_AGENT_MAX_CONCURRENT` with `flock`; keep it at `1` for the current single-agent portal-wide execution mode.
Codex dispatch supports encoded model choices such as `gpt-5.5:xhigh`; the runner passes `gpt-5.5` as the model and `xhigh` as `model_reasoning_effort`. Override the effort with `WORK_HARNESS_CODEX_REASONING_EFFORT`.
Claude dispatch supports encoded model choices such as `claude-opus-4-8:xhigh`; the runner passes `claude-opus-4-8` as the model and `xhigh` as `--effort`. Override the effort with `WORK_HARNESS_CLAUDE_EFFORT`.
Claude dispatch defaults to `WORK_HARNESS_CLAUDE_PERMISSION_MODE=bypassPermissions` and no `--max-turns` cap because the run is non-interactive. Set `WORK_HARNESS_CLAUDE_MAX_TURNS` or the portal Agent Max turns field only when a finite cap is desired. Scope the SSH user, checkout, worktree root, and credentials for automation. The wrapper prompt tells Claude not to use `AskUserQuestion`; clarification requests should be written as final output.
The wrapper prompt also tells Claude to use native `git` commands and the provided webhook/GitHub Issue context instead of relying on `gh` or gh-based skills, which may be unavailable in remote runs.
