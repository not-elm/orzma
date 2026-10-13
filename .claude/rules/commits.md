# Commit Rules

Commit messages follow the Conventional Commits style of the history, as
`CONTRIBUTING.md` describes (`feat(webview): …`, `fix(input): …`,
`docs: …`). The rules below add what the history alone does not show.

## Commits that apply code-review findings

A commit that applies the findings of a code review — `/code-review`,
including its `--fix` mode, or the review step of a pipeline such as
`forte:autopilot` — is titled `fix(code-review): <summary>`, so the log
shows which changes came out of a review rather than out of the original
work.

Required:

| Pattern                          | Example                                                          |
| -------------------------------- | ---------------------------------------------------------------- |
| `fix(code-review): <summary>`    | `fix(code-review): address the review of the line-gap baseline` |

- The type is `fix` and the scope is `code-review`, whatever mix of bug
  fixes, doc corrections and cleanups the findings hold.
- The summary names the work that was reviewed, not the area the scope
  would otherwise carry: `address the review of the line-gap baseline`,
  not `fix(input): …`.
- The findings are applied in a commit of their own, on top of the
  reviewed commits, rather than amended into them.

Forbidden:

| Pattern                                               | Example                                            | Why                                                    |
| ----------------------------------------------------- | -------------------------------------------------- | ------------------------------------------------------ |
| A review-fix commit titled like ordinary work         | `fix: address the review of the line-gap baseline` | The log no longer shows that the change came from a review |
| Review findings folded into the commits they reviewed | `git commit --amend` with the review fixes         | Same                                                   |
