# Decision Issues (not ADRs)

Architecture decisions are captured as **GitHub issues**, not in-repo `docs/adr/` files. In-repo ADRs
drift out of sync with the code; an issue gives a searchable history tied to the actual outcome — the
PR that implemented it, the discussion, the close state.

## How to record one

Create an issue with the `decision` label (create the label if it doesn't exist), then close it as the
record — or keep it open if the decision is still being implemented. Reference it from the relevant PR.

```bash
gh label create decision --color 5319e7 --description "Architecture decision record" 2>/dev/null || true
gh issue create \
  --label decision \
  --title "Decision: <short title of the decision>" \
  --body "$(cat <<'EOF'
## Decision

<1-3 sentences: what's the context, what we decided, and why.>

## Trade-off

<The genuine alternatives considered and why this one won.>

## Consequences

<Only if there are non-obvious downstream effects worth flagging.>
EOF
)"
```

A decision issue can be a single paragraph. The value is recording _that_ a decision was made and
_why_ — not filling out sections. Drop **Consequences** when nothing non-obvious follows.

## When to offer a decision issue

Offer one only when **all three** are true:

1. **Hard to reverse** — the cost of changing your mind later is meaningful.
2. **Surprising without context** — a future reader will look at the code and wonder "why on earth did
   they do it this way?"
3. **The result of a real trade-off** — there were genuine alternatives and you picked one for specific
   reasons.

If a decision is easy to reverse, skip it — you'll just reverse it. If it's not surprising, nobody will
wonder why. If there was no real alternative, there's nothing to record beyond "we did the obvious
thing."

### What qualifies

- **Architectural shape.** "We're using a monorepo." "The write model is event-sourced, the read model
  is projected into Postgres."
- **Integration patterns between contexts.** "Ordering and Billing communicate via domain events, not
  synchronous HTTP."
- **Technology choices that carry lock-in.** Database, message bus, auth provider, deployment target —
  the ones that would take a quarter to swap out, not every library.
- **Boundary and scope decisions.** "Customer data is owned by the Customer context; other contexts
  reference it by ID only." The explicit no-s are as valuable as the yes-s.
- **Deliberate deviations from the obvious path.** "Manual SQL instead of an ORM because X." Anything a
  reasonable reader would assume the opposite of — it stops the next engineer from "fixing" something
  deliberate.
- **Constraints not visible in the code.** "We can't use AWS because of compliance." "Response times
  must be under 200ms because of the partner API contract."
- **Rejected alternatives when the rejection is non-obvious.** Considered GraphQL, picked REST for
  subtle reasons → record it, or someone suggests GraphQL again in six months.
