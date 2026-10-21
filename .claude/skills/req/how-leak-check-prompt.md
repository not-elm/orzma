# How-leak check

Use this prompt when dispatching the check subagent. Fill in the document path.

```
You are checking a requirements document for the orzma terminal emulator
before a human approves it. The document must state only what problem exists
and what solution is required. The human designs the code afterwards, so
the document must leave every structural decision open.

Document: <path>

You may read the repository to tell an internal name from a public surface.
Do not edit any file.

Classify every sentence in these sections: Problem, Required solution,
Non-goals, Alternatives considered, Open questions.

DESIGN — the sentence decides, names, or describes code structure:
- types, traits, fields, enum variants, function or method signatures
- modules, files, crates, or where code goes
- threads, processes, or which component does the work
- systems, observers, events, messages, resources, plugins, or schedules
  inside orzma
- algorithms, data structures, caching, or the internal order in which
  mechanisms are tried
- test cases, test procedures, or acceptance tests
- task breakdowns or implementation phases
Also report as DESIGN two or more sentences that are each fine alone but
together fix one internal structure; quote all of them in one finding.

OK — leave these unreported:
- behavior a user can observe
- public surfaces, existing or new: config keys and values, command-line
  flags, key bindings, UI text, control sequences (OSC, CSI, …), webview
  or socket protocol messages, the SDK's public API
- measurable outcomes: latency, startup time, memory, build time
- keeping an existing public surface compatible
- platform scope and other scope decisions
A sentence that mixes both is DESIGN; quote only the design part.

UNSURE — you cannot tell. Use this rather than guessing. A tie-breaker:
could two structurally different mock implementations both satisfy the
sentence? If yes, it is OK. If it forces one structure, it is DESIGN.

Carry-over memo for Step 2: do not classify its content; it is allowed to
hold implementation ideas. Report an entry only if it is not a verbatim
quote attributed to the human.

Report, most important first:

FINDING <n>
Section: <section>
Text: "<exact quote>"
Verdict: DESIGN | UNSURE | MEMO
Reason: <one sentence>

If there is nothing to report, reply exactly: NO FINDINGS
```
