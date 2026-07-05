# Coding Standards

These standards describe how to design and write code in this codebase — Rust
(`crates/`, `crates-cli/`, `crates-tauri/`) and TypeScript (`apps/client`).
They are especially intended for agents: before adding patterns, libraries,
adapters, or abstractions, read the existing code and prefer the local
convention unless it conflicts with the safety/correctness principles below.

Adapted from [dmmulroy's TypeScript coding standards gist](https://gist.github.com/dmmulroy/9c80f1f499b031aa0b6525b5d9ae25f0),
generalized to cover Rust alongside TypeScript. Most of these rules are
already the idiomatic default in Rust (errors as values via `Result`, no
`null`, ownership-enforced immutability) — the Rust callouts below mostly
point at *where* the equivalent lives rather than introducing new machinery.

## Decision priority

When rules pull in different directions, use this order:

1. Preserve correctness, safety, and debuggability.
2. Follow established project architecture and conventions.
3. Improve the local design toward these standards.
4. Avoid broad migrations unless explicitly requested.
5. Document meaningful trade-offs with comments or ADRs.

New code paths, modules, adapters, and services should generally follow these
standards, but do not force a whole-project migration for an unrelated
change.

## Core principles

- Prefer **errors as values** over `throw` / rejected promises (TS) or
  `panic!`/`unwrap()` (Rust) for expected failures.
- Parse early. Do not merely validate and throw away the information learned.
- Make illegal states unrepresentable where practical.
- Prefer correct-by-construction APIs over convention-based invariants.
- Use branded/refined/domain types liberally for meaningful primitives.
- Prefer composition over inheritance.
- Prefer imperative shell / functional core.
- Design deep, cohesive modules with low caller burden.
- Test behavior through real seams; avoid module mocks and spy-driven tests.
- Keep code discoverable for humans and agents.

## Adapting to existing codebases

Before adding a new pattern or library, inspect the repo for existing choices
around:

- error handling
- schema parsing / deserialization
- dependency injection
- testing
- observability
- adapters/services
- module layout

Prefer consistency inside the codebase. At boundaries, translate between
local typed errors and whatever the framework or existing code expects (e.g.
flattening `thiserror` errors to CLI exit codes in `tt-cli`, per this repo's
[CLAUDE.md](../CLAUDE.md) conventions).

## Errors and failures

### Expected failures are values

Expected failures include domain, parsing, authorization, integration, I/O,
persistence, and workflow failures. They should appear in the return type.

**Rust:** this is the default — return `Result<T, E>` with a `thiserror`
enum for `E`. Library crates under `crates/` define their own error enums;
`tt-cli` flattens them to exit codes at the boundary. Never `unwrap()` or
`expect()` outside tests for a failure mode a caller could actually hit.

**TypeScript:** prefer a small local tagged union when the codebase doesn't
already have one:

```ts
type Result<T, E extends Error> =
  | { readonly _tag: "ok"; readonly value: T }
  | { readonly _tag: "err"; readonly error: E };
```

Prefer `Promise<Result<User, UserLookupError>>`, not `Promise<User>` that
rejects for ordinary lookup/storage failures. Promise rejection is
equivalent to throwing — treat it as acceptable only for unrecoverable
defects or unclassified third-party behavior at a boundary.

### Unrecoverable defects may throw/panic

Acceptable for panic-style failures:

- violated internal invariants
- impossible branches
- startup misconfiguration
- temporary not-yet-implemented paths
- catastrophic runtime conditions

**Rust:** `unreachable!()` for impossible branches, `panic!()` for startup
misconfiguration, `todo!()` for temporary stubs. Match exhaustively on enums
instead of adding a catch-all arm — let the compiler enforce it.

**TypeScript:** use shared helpers from `prelude.ts` where available
(`casesHandled` for exhaustive union handling, `shouldNeverHappen`,
`notYetImplemented`). Avoid one-off `assertNever` helpers when the project
already has these.

### Custom errors

Expected failures should use custom tagged errors with a stable tag, a
useful message, structured contextual fields, safe telemetry fields, and an
optional cause chain.

**Rust:**

```rust
#[derive(Debug, thiserror::Error)]
pub enum UserStoreError {
    #[error("user store unavailable during {operation}")]
    Unavailable {
        operation: &'static str,
        #[source]
        cause: std::io::Error,
    },
}
```

**TypeScript:**

```ts
export class UserStoreUnavailable extends Error {
  readonly _tag = "UserStoreUnavailable";

  constructor(
    readonly operation: "findActiveByEmail",
    readonly provider: "postgres",
    readonly cause: unknown,
  ) {
    super(`User store unavailable during ${operation}`);
  }
}
```

Keep error unions/enums precise at module boundaries. Avoid broad
`AppError`-style catch-all types except near entrypoints, orchestration,
logging, and rendering layers.

## Sensitive data, telemetry, and debugging

Tracing/logging should make failures diagnosable with safe fields: domain
IDs, operation names, dependency/provider names, state tags, retry counts,
typed error tags, safe summaries.

Do not put secrets in errors, traces, logs, or snapshots. Wrap sensitive
values (tokens, API keys, passwords, raw credentials) in a newtype/wrapper
that redacts on `Debug`/`Display` (Rust: a `Redacted<T>` tuple struct with a
hand-written `Debug` impl; TypeScript: a local `Redacted<T>` in
`prelude.ts`), and unwrap only where the raw value is needed — usually
inside an adapter making an external call.

## Parse, don't validate

Boundary code should turn unknown or less-structured input into domain
types as early as practical.

Prefer:

```
unknown -> HttpBodyDto -> CreateUserInput -> EmailAddress/UserId/etc.
```

not passing a loosely-typed blob (`serde_json::Value` / `z.infer<...>`)
throughout the app.

Use names that preserve meaning:

- `parse_x(input) -> Result<X, ParseXError>` (Rust) / `parseX(input): Result<X, ParseXError>` (TS) for untrusted or less-structured input
- `X::new(...)` / `make_x(...)` (Rust) / `makeX(...)` / `createX(...)` (TS) for smart constructors from already-typed pieces
- `is_x(value) -> bool` / `isX(value): boolean` for true predicates
- `assert_x`/`assertX` rarely, mostly at tests/framework boundaries

Avoid `validate_x`/`validateX` when the function returns a refined value —
it parsed something.

### Schemas

Use schema libraries as boundary parsers, not as ad-hoc validators sprinkled
through core logic.

**Rust:** `serde` with `#[serde(deny_unknown_fields)]` at strict boundaries,
`#[serde(default)]`/tolerant parsing at shared-format boundaries (e.g.
`tt-config`'s settings file, which is shared with the TypeScript CLI and
must NOT use `deny_unknown_fields`). Fallible `TryFrom`/smart constructors
for turning a deserialized DTO into a domain type.

**TypeScript:** prefer Zod 4 (or the repo's established schema library) as
the boundary parser, producing refined/domain types and typed custom errors.

## Branded types and correct construction

Use branded/refined types for meaningful primitives:

- IDs: `UserId`, `OrgId`, `WorkflowId`
- parsed strings: `EmailAddress`, `NonEmptyString`, `Url`
- constrained numbers: `PositiveInt`, `Cents`, `Percentage`
- units: `Milliseconds`, `Bytes`, `UsdCents`

**Rust:** the newtype pattern (`pub struct UserId(Uuid)`) with a private
field and a `parse`/`new`/`TryFrom` constructor is the branded type — invalid
instances are unconstructable by design.

**TypeScript:** `type EmailAddress = Brand<string, "EmailAddress">`,
constructed only through `parse`/smart constructors.

Avoid optional/null/undefined values in functions that require a value.
Push optionality outward — branch or parse before calling. Avoid
`Partial<T>` (TS) or `Option<T>` fields sprinkled through a struct (Rust) as
an application/domain input unless partiality is the real domain concept;
prefer explicit input types for each operation.

## State machines and boolean blindness

When an entity has meaningful lifecycle states, model them with tagged
unions/enums, not a bag of booleans and optional timestamps.

**Rust** (this is the natural shape of an enum):

```rust
enum Invoice {
    Draft { id: InvoiceId, lines: Vec<LineItem> },
    Sent { id: InvoiceId, sent_at: Instant },
    Paid { id: InvoiceId, paid_at: Instant },
}
```

**TypeScript:**

```ts
type Invoice =
  | { readonly _tag: "Draft"; readonly id: InvoiceId; readonly lines: NonEmptyArray<LineItem> }
  | { readonly _tag: "Sent"; readonly id: InvoiceId; readonly sentAt: Instant }
  | { readonly _tag: "Paid"; readonly id: InvoiceId; readonly paidAt: Instant };
```

Avoid the `isSent: boolean; isPaid: boolean; sentAt?: Date; paidAt?: Date`
shape, and avoid boolean parameters that control behavior
(`createUser(input, true)`) — prefer named options or domain types
(`createUser(input, { emailVerification: "skip" })` / a Rust options struct
or enum). Booleans are fine as clear predicate return values
(`is_expired(token) -> bool`, `has_permission(user, permission) -> bool`).

## Modules and abstractions

### Deep modules

A deep module hides substantial behavior/invariants behind a cohesive,
low-burden interface. Low-burden does not necessarily mean few
functions/methods — a domain module may expose many cohesive combinators
around one concept and still be deep.

Avoid shallow abstractions that merely forward calls, mirror tables, or
expose implementation steps.

Use the deletion test:

- if deleting the module makes complexity disappear, it was probably
  pass-through waste
- if deleting it spreads complexity across callers, it was probably earning
  its keep

### Domain modules

Prefer OCaml-style domain modules for core concepts. A domain module
centers on one primary type or tightly related type family and exposes
parsers, smart constructors, combinators, predicates, and formatting
helpers for that concept. In Rust this maps directly onto a module
(`mod email_address`) with a type plus free functions; in TypeScript it maps
onto a file with a namespace-style import.

**Rust** (`email_address.rs`):

```rust
/// A parsed, normalized email address.
pub struct EmailAddress(String);

impl EmailAddress {
    /// Parse an email address from untrusted input.
    pub fn parse(input: &str) -> Result<Self, InvalidEmailAddress> { .. }
}

impl std::fmt::Display for EmailAddress { .. }
```

**TypeScript** (`email-address.ts`):

```ts
/** A parsed, normalized email address. */
export type EmailAddress = Brand<string, "EmailAddress">;

/** Parse an email address from untrusted input. */
export function parse(input: string): Result<EmailAddress, InvalidEmailAddress>;

/** Render an email address as a string. */
export function toString(email: EmailAddress): string;
```

If using classes/structs for domain values:

- construct through `parse` / `make` / smart constructors
- make invalid instances unconstructable
- keep fields private/readonly from callers
- keep methods cohesive over that value
- do not hide dependencies or I/O inside domain value types
- avoid inheritance for domain behavior

### Application/service modules

Application modules own real capabilities or operations — e.g. this repo's
collectors in `tt-collect`, or `tt-store`'s todo/issue/PR operations. They
coordinate domain modules, persistence, external calls, authorization,
workflows, and telemetry.

Prefer a struct with constructed-in dependencies (Rust) or a class with
constructor injection (TypeScript) when the module has dependencies,
stateful resources, configuration, or multiple cohesive operations.

Avoid dependency bags (a `deps` struct/object threaded into every function)
unless the framework demands it.

No arbitrary method limit. Split when methods are unrelated, change for
different reasons, require unrelated dependencies, or create an accidental
grab bag. Avoid vague names like `Manager`, `Processor`, `Helper`, or a
generic `UserService` unless established by the framework/project.

## Dependency interfaces and adapters

Depend on the smallest meaningful shape a module actually uses. Let
concrete adapters be wider.

**Rust:** define a narrow trait for what the caller needs
(`trait UsersForPasswordReset { fn find_active_by_email(&self, email: &EmailAddress) -> Result<ActiveUser, UserLookupError>; }`)
and let a wider concrete type (e.g. a `PostgresUsers` struct with many
methods) implement it.

**TypeScript**, structural typing gives the same effect for free:

```ts
type UsersForPasswordReset = {
  findActiveByEmail(email: EmailAddress): Promise<Result<ActiveUser, UserLookupError>>;
};
```

This avoids both mega-repositories and one-method adapter sprawl.

### Adapter reuse audit

Before creating a new adapter or service, audit existing adapters/services.
Prefer, in order:

1. Reuse an existing adapter as-is through a narrow dependency type/trait.
2. Extend an existing adapter if the new method fits its existing cohesive
   capability and changes for the same reason.
3. Create a new adapter only when reuse/extension would create bad coupling
   or an accidental interface.

When a meaningful new adapter/service is still created after the audit,
note in the PR/commit description:

- what existing adapters/services were checked
- why reuse did not fit
- why extension did not fit
- why the new adapter is a separate cohesive capability

Skip this for tiny local test adapters, obvious in-memory fakes, or trivial
framework glue.

### Repositories and persistence

Avoid repository-per-table by default. Repository-like adapters (e.g.
`tt-store`) are acceptable when they represent a cohesive domain
persistence capability — they should expose meaningful domain operations
and return parsed domain types / typed errors, not raw rows/ORM errors.

Treat raw database rows (`rusqlite::Row`, an ORM model) as infrastructure
DTOs. Parse them into domain types before application/core logic. Keep
SQL details inside the persistence module.

## Functional core, imperative shell, and entrypoints

Keep domain/application behavior reusable across the CLI (`tt-cli`), the
Tauri app (`tt-app`), and the MCP server (`tt-mcp`) — this is the reason
shared logic lives in Tauri-free `crates/` libraries at all.

The functional core contains: domain logic, parsers, state transitions,
combinators, decision functions. It avoids: I/O, hidden dependencies,
ambient time/randomness (see `now_ms` passed into `tt-store` rather than
reading the clock), thrown/panicking expected failures, framework-specific
concerns.

The imperative shell: parses untrusted input, sequences effects, calls the
core with refined values, classifies external failures into typed errors,
handles I/O, persistence, HTTP, queues, telemetry, time, randomness.

Entrypoint adapters (CLI commands, Tauri commands, MCP handlers) should be
thin protocol translation layers — parse protocol-specific input, invoke
shared modules, render protocol-specific output. Do not duplicate business
rules in CLI handlers/Tauri commands/MCP handlers.

## Workflows, transactions, and idempotency

Use ordinary function calls or database transactions for simple
single-boundary operations.

Use a saga/durable workflow when the process needs: retries, compensation,
idempotency, resumability, timers, human approval, cross-service
coordination, or multiple transaction boundaries.

Do not hold database transactions open across network calls or
long-running operations.

Any command, job, or workflow step that may be retried needs an explicit
idempotency strategy: idempotency key, natural unique constraint,
deduplication record, state-machine transition guard, or transactional
outbox/inbox. Retrying should not rely on "probably safe" side effects.

## Testing

Prefer confidence-oriented tests:

1. e2e for critical user flows
2. integration tests through real seams
3. focused/property tests for pure domain modules
4. unit tests when they test meaningful behavior, not implementation details

**Rust:** black-box CLI tests with `assert_cmd` (per this repo's
[CLAUDE.md](../CLAUDE.md)); unit tests alongside logic. Prefer real
seams — a temp SQLite DB for `tt-store`, a real filesystem in a tempdir for
`tt-journal` — over hand-rolled mock traits.

**TypeScript:** never use `vi.mock`/`jest.mock` for module mocking. Use real
seams: constructor-injected interfaces/classes, local database substitutes
such as SQLite, in-memory adapters when behavior is simple, fake external
adapters when needed.

In both languages, prefer tests that assert observable input/output
behavior (returned value/error, persisted state, emitted event/message,
rendered response) over spy-driven tests
(`expect(sendEmail).toHaveBeenCalledWith(...)`) unless the interaction
itself is the only observable behavior.

### Property tests and arbitraries

Use property tests where properties are clearer than examples, especially
for parsers/smart constructors, branded/refined types, state machines,
serialization roundtrips, normalization/idempotence, and lawful
combinators.

**Rust:** `proptest` or `quickcheck`. **TypeScript:** `fast-check`. Prefer
exporting arbitraries/generators near the domain module they support
(e.g. `invoice_number.rs` + a `#[cfg(test)] mod proptest_support` or
`invoice-number.ts` + `invoice-number.arbitrary.ts`).

Tests should not bypass parsers, smart constructors, or invariants.

## Style and safety

**Rust:** `cargo clippy --all -- -D warnings` and `cargo fmt --check` are
non-negotiable (per this repo's CLAUDE.md). Avoid `unwrap()`/`expect()`
outside tests and quick prototypes; avoid `as` casts where a `TryFrom`/checked
conversion exists; avoid `.clone()` as a substitute for thinking about
ownership when a borrow would do. A `// SAFETY:` comment is required on any
`unsafe` block, explaining the invariant the compiler can't check.

**TypeScript:** use strict settings where practical (`strict: true`,
`noUncheckedIndexedAccess: true`, `exactOptionalPropertyTypes: true`,
`noImplicitOverride: true`, `noFallthroughCasesInSwitch: true`). Prefer
immutable values (`readonly` fields, `ReadonlyArray`). Mutation is
acceptable inside localized imperative shell code, performance-sensitive
internals, builders, or adapters when hidden behind a precise interface.

Avoid `any`, non-null assertions (`!`), and `as Type` casts (`as const` is
fine). Any non-`as const` cast requires a Rust-like safety comment:

```ts
// SAFETY: TypeScript cannot express the brand. parseEmailAddress checked the normalized string before branding. Callers cannot construct EmailAddress except through this parser.
return normalized as EmailAddress;
```

Do not use `!` — branch, parse, or refine instead.

## Imports, exports, and files

Prefer direct imports from the file/module that owns the abstraction.
Avoid barrel files / `index.ts` re-export layers, and avoid Rust `pub use`
re-export sprawl, by default.

For domain modules, namespace-style imports often preserve the module
shape (`import * as EmailAddress from "./email-address"` in TS; `use
crate::email_address;` + `email_address::parse(..)` in Rust). Use named
imports for classes, prelude helpers, and focused shared helpers.

Export/`pub` only what callers should use. Keep internal helpers
unexported/private unless intentionally shared. Do not widen visibility
just for tests — use `#[cfg(test)]` submodules (Rust) or test-only exports
sparingly and deliberately (TS).

Avoid vague files: `utils.ts`, `helpers.ts`, `common.ts`, `misc.rs`. Use
precise names: `email_address.rs`, `billing_period.ts`, `string_case.rs`.

No arbitrary file-size limits. Prefer cohesion and discoverability over
small files for their own sake. Split when a file has multiple unrelated
reasons to change or callers must understand unrelated concepts.

## Comments and docs

Comments should explain invariants, trade-offs, non-obvious domain rules,
and safety justifications. Avoid comments that narrate obvious code — this
repo's CLAUDE.md already says "default to no comments; only add one when
the WHY is non-obvious."

Every exported/public function, type, and constant should have a doc
comment: `///` doc comments in Rust (so `cargo doc` and hover-docs pick them
up), JSDoc in TypeScript.

```rust
/// Parse an email address from untrusted input.
///
/// Returns `InvalidEmailAddress` when the input is malformed.
pub fn parse(input: &str) -> Result<EmailAddress, InvalidEmailAddress> { .. }
```

```ts
/**
 * Parse an email address from untrusted input.
 *
 * @param input - The untrusted string to parse.
 * @returns A parsed email address, or `InvalidEmailAddress` when the input is invalid.
 */
export function parse(input: string): Result<EmailAddress, InvalidEmailAddress>;
```

Document panics (`# Panics` in Rust doc comments) or `@throws` (TS) only for
unrecoverable defects, framework-required behavior, or temporary
not-yet-implemented paths. Do not document expected typed errors as panics
or throws.

## Configuration and resources

Parse environment/config at startup or the earliest boundary into typed
config with redacted values where appropriate (this repo: `tt-config`'s
settings are loaded once and passed down, not re-read ad hoc). Missing/
invalid config is a startup failure with useful context.

Avoid top-level side effects except in true entrypoint/bootstrap files (CLI
`main`, Tauri `main`, MCP server entry). Modules should not start servers,
open connections, read env, register handlers, or perform I/O at import/
load time.

Avoid mutable singletons/global state. Constants and pure lookup tables are
fine. Inject a clock/random source into dependency-bearing modules (this
repo already does this: `tt-store` takes `now_ms` as a parameter — never
reads the clock in logic). Pure domain functions may accept explicit `now`/
random values.

## Quick agent checklist

Before coding:

- Read existing conventions for errors, schemas, tests, adapters,
  telemetry, and module layout.
- Look for existing domain modules/types before creating new ones.
- Look for existing adapters/services before creating a new one.
- Parse inputs at the edge and use domain types internally.
- Avoid raw DTOs, raw IDs, nullable bags, and `Partial<T>`/loose `Option`
  fields in core/application logic.
- Prefer typed errors as values (`Result<T, E>`) for new expected failures.
- Preserve existing observability/error mechanics.
- Test through public interfaces and real seams — no `vi.mock`/`jest.mock`,
  no hand-rolled Rust mock traits when a real seam (tempdir, temp SQLite)
  is available.
- Use `proptest`/`fast-check` for generated test data when practical.
- Add `///`/JSDoc for exported/public symbols.
- Note the adapter reuse audit in the PR description for meaningful new
  adapters/services.
