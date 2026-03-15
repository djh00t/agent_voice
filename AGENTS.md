# AGENTS.md

## Repo Rules

- Always use atomic commits.
- Each commit must touch exactly one file.
- Each commit must represent exactly one change.
- Always commit and push all completed changes.
- Always use conventional commit messages with a proper scope.
- Always use a multiline commit format:

```text
type(scope): short summary

Why:
- reason

What:
- change

Refs:
- issue: ISSUE-123
- task: TASK-456
- story: STORY-789
```

- If an issue, task, or story number is not available, omit that footer line rather than inventing one.
- Run the repo validation gates before pushing changes that affect behavior.
- Do not batch unrelated files into a single commit.
