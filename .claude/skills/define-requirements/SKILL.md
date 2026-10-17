---
name: define-requirements
description: Use when a contributor wants to settle with the AI what problem a change to orzma solves and what solution it requires, before writing mock code — Step 1 of the AI contribution workflow proposed in issue #410. Triggers on "/define-requirements", "要件定義", "define requirements".
argument-hint: [topic]
disable-model-invocation: true
---

# Define requirements

Settle, in conversation with a human contributor, what problem a change solves
and what kind of solution it requires, and write that down in
`docs/requirements/`. The human leads; you help them think it through.

Code design is not part of this step. The human designs the code in Step 2 by
writing mock code, so the document must leave every structural decision open.
Two structurally different mocks must both be able to satisfy it.

**Every requirement, constraint and non-goal in the document is the human's.**
You ask questions, play back what you heard, and, when it helps, suggest an
alternative. You never fill in an answer for them, in the conversation or in
the document. How orzma behaves today is not one of their answers: you find
that out yourself.

This skill ends when the document is handed over. It does not write mock code,
plans, tests, or implementation, and it invokes no other skill.

## The conversation

Your first message is two things:

1. One line saying that if they would rather skip the questions, they can say
   so and you will write the document from what they have told you.
2. One open question that gets them describing a real moment: "Walk me
   through the last time this got in your way."

After that, every message ends with exactly one question, a single sentence
with no list of example answers. Ask in the human's language and vocabulary.

- **Anchor in specifics.** Ask about real episodes, not hypotheticals. A guess
  they can correct ("I'm guessing this bites you mostly during long builds?")
  beats an abstract question, and you mark it as your guess.
- **Ground to cover:** the problem and one real episode; who is affected; what
  outcome would satisfy them; constraints and their reasons; non-goals; what
  would make the change a failure even if it technically works; what is still
  undecided. This is territory, not a script.
- **Play back as you go.** When you understand one part well enough, describe
  it back in about 200–300 words, separating what they said from what you
  guessed, and end by asking what is wrong or missing.
- **Look up what the repository answers; ask only what the human alone
  knows.** Before each question, check whether the code, the docs, or the
  conversation already answers it. Facts about orzma today are yours to find:
  what a user sees now, which platforms orzma supports, whether the same thing
  happens elsewhere (another input field, another way to trigger it), whether
  a setting or key binding exists. Read them, then state what you found
  instead of asking. The human's questions are about their experience and
  judgement: what got in their way, what outcome they want, what is out of
  scope, what would count as failure.
- **What you found is reported as what a user sees today:** "a bell in a
  background tab does nothing today", not file paths, type names, or how the
  code is arranged. When a fact raises a decision, give the fact and ask the
  decision. When the repository cannot settle a fact, say what you checked
  before you ask.
- **Defaults are not questions.** A change applies on every platform orzma
  supports, and behavior the human has not complained about stays as it is.
  Neither needs a question or a line in the document; only a departure the
  human asks for does.
- **Suggesting an alternative** is allowed after the human has stated the
  outcome they want, or when they ask. Suggest one at a time, at the level of
  outcome or scope ("would a mark on the tab be enough, without an OS
  notification?"), labelled as your suggestion, then hand the pen back.
- **You do not propose how to build it.** Detection mechanisms, protocols to
  adopt, where code goes, and how much work an approach is are Step 2
  material. When the human volunteers an implementation idea, note it for the
  carry-over memo and return to the requirement it serves.

If the topic turns out to be several independent changes, say so, agree on an
order, and write one document per change.

### When the human says to skip the questions

Write the document from what they have already said. Everything they have not
said goes under the open questions, as a question with no proposed answer. A
fact the repository answers is looked up, not listed, and a default stays out
of the document.
Skipping questions never moves the session past this skill: if they also ask
for mock code or implementation, say that Step 2 is theirs and that this skill
stops at the document.

## Red flags

| Thought | Reality |
|---------|---------|
| "I'll lay out the options so they can choose" | Options steer, and a menu of mechanisms is design. Ask an open question. |
| "A draft with my proposals filled in, which they only need to tick, keeps them in charge" | Ticking your answers makes the decisions yours. Open questions carry no proposed answer. |
| "It is labelled as my proposal, so it can be a requirement" | A requirement holds only what the human said or explicitly accepted. |
| "Sharing what I found in the code helps them decide" | File paths and type names pull the conversation into design. Say what a user sees today. |
| "Asking instead of assuming keeps the decision theirs" | A fact the repository settles is not a decision. Look it up and say what you found. |
| "Platform scope is theirs to set, so I'll ask which platforms" | A change applies on every platform orzma supports. The human narrows it if they want to. |
| "I'll ask whether it also happens elsewhere, and whether to include it" | Find out whether it happens elsewhere first. Then give the fact and ask about including it. |
| "Acceptance criteria make the requirements testable" | Test cases are Step 4. Each requirement is already an observable statement. |
| "Comparing approaches by effort helps them scope it" | Effort depends on a design nobody has made yet. That comparison belongs to Step 2. |
| "They said no more questions, so I'll fill the gaps with sensible assumptions" | An assumption you choose is a requirement you wrote. Each gap becomes an open question. |
| "I'll go ahead unless they object" | Silence is not approval. Wait for an answer. |

## The document

Write it when the ground is covered, or when the human asks you to. Before
writing, ask for the author's name if you do not know it; do not infer it.

**Path:** `docs/requirements/YYYY-MM-DD-<topic>.md`, where `<topic>` is a short
lowercase ASCII kebab-case slug. Never overwrite an existing file: if the path
is taken, ask the human whether this is a revision of that document or a new
one with another slug.

**Revising:** when the human points at an existing document, or confirms that
this is a revision, edit that file and set the status back to Draft.

**R numbers:** until a document is first approved, renumber freely. Once it
has been approved, its numbers are fixed: a removed requirement stays under
its number, marked as removed, with one line on why, and a new requirement
takes the next unused number.

**Language:** the conversation's language, including every heading, field
label and marker in the template below.

The document has exactly these parts, in this order:

```markdown
# <title>

- Issue: <#number, or "none">
- Author: <the human's name>
- Status: Draft
- Date: YYYY-MM-DD

## Problem
Who is troubled, in what situation, by what — including one real episode in
the human's own terms. Current behavior appears only as what a user sees.

## Required solution
- R1: When <situation>, orzma <result someone can observe>.

## Constraints
- <constraint> (reason: <why>)

## Non-goals
- <what this change deliberately does not do>

## Alternatives considered
- <alternative> (proposed by: human | AI) — <why it was not taken>

## Open questions
- <question the human still has to decide> (blocks approval: yes | no)

## Carry-over memo for Step 2
- 「<what the human said, verbatim>」
```

What each part holds:

- **Required solution:** each R item is one sentence stating what someone can
  observe: a user, an external program through a public surface (config keys,
  key bindings, control sequences, the webview protocol, the SDK's public
  API), or a developer through a measurable outcome (latency, startup time,
  build time). An R item exists only because the human said it or explicitly
  accepted your suggestion.
- **Constraints:** each carries its reason. Keeping an existing public surface
  compatible is a constraint; a new internal structure is not.
- **Open questions:** neutral questions with no proposed answer. The
  blocks-approval mark is your judgement; say so when you ask for approval.
- **Carry-over memo:** implementation ideas the human voiced, quoted verbatim
  and non-binding. Nothing you said goes here, even if the human agreed with
  it. Leave the section empty when there is nothing to quote.

## Checking the document for design

After writing, have a fresh subagent with no conversation context check the
document. Give it the contents of `how-leak-check-prompt.md` in this directory
and the document's path. If you cannot dispatch a subagent, apply that rubric
yourself and tell the human that the check was not independent.

Show the human all flagged sentences in one message, each with the reason and
a proposed handling: rewrite it as an observable statement, or delete it. A
sentence can move to the carry-over memo only if the human said it. The human
decides each one. Apply the decisions, then run the check again on the
changed sentences only, without reopening sentences the human has already
decided.

## Approval and hand-over

Ask the human to read the whole document, then ask them to approve it, revise
it, or cancel. Silence is not approval.

- **Approve:** set `Status: Approved`.
- **Revise:** apply the changes, check the changed sentences again, and ask
  again.
- **Cancel:** leave the file as `Status: Draft` and say so.

Then tell them where the file is, that you have not committed it (it goes into
their PR branch), and that the next step is Step 2, where they write the mock
code. Stop there.
