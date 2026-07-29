---
name: explain
description: Explain code with analogies and diagrams. Use when the user asks how something works, wants to understand a codebase, or needs a walkthrough.
when_to_use: "explain", "how does", "what does", "walk me through", "understand", "how it works"
allowed_tools:
  - read
  - grep
  - graph_query
  - graph_find
  - graph_callers
---

You are explaining code. Make it clear, visual, and relatable.

## Target

$ARGUMENTS

## Steps

1. **Start with an analogy**: Compare the code pattern to something from everyday life.
   - Example: "A message queue is like a post office mailbox — messages go in, recipients pick them up later."

2. **Draw a diagram**: Use ASCII art to show the flow.
   - Show the main components and how they connect.
   - Keep it simple — don't draw every detail.

3. **Walk through the code**: Explain step-by-step what happens.
   - Use line references: "At line 42, we..."
   - Explain WHY, not just WHAT.
   - Call out key decisions and trade-offs.

4. **Highlight a gotcha**: What's a common mistake or misunderstanding?
   - Off-by-one errors, null handling, race conditions, etc.

5. **Summarize**: End with a one-sentence summary of what the code does.

## Guardrails

- Don't explain obvious syntax. Assume the reader knows the language.
- Don't read the entire file aloud. Focus on the interesting parts.
- If the code is complex, break the explanation into layers (high-level → detail).
- Use the code graph tools to understand relationships before explaining.

## Verification

- [ ] Analogy provided
- [ ] Diagram included
- [ ] Key lines referenced
- [ ] Gotcha highlighted
- [ ] One-sentence summary at the end
