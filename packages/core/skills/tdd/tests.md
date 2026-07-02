# Good and Bad Tests

## Good Tests

**Integration-style**: test through real interfaces, not mocks of internal parts.

```typescript
// GOOD: Tests observable behavior
test("user can checkout with valid cart", async () => {
  const cart = createCart();
  cart.add(product);
  const result = await checkout(cart, paymentMethod);
  expect(result.status).toBe("confirmed");
});
```

Characteristics:

- Tests behavior users/callers care about
- Uses public API only
- Survives internal refactors
- Describes WHAT, not HOW
- One logical assertion per test

## Bad Tests

**Implementation-detail tests**: coupled to internal structure.

```typescript
// BAD: Tests implementation details (and uses vi.mock, which is banned in this repo)
test("checkout calls paymentService.process", async () => {
  const mockPayment = vi.mock(paymentService);
  await checkout(cart, payment);
  expect(mockPayment.process).toHaveBeenCalledWith(cart.total);
});
```

Red flags:

- Mocking internal collaborators (and `vi.mock` is banned outright — see [mocking.md](./mocking.md))
- Testing private methods
- Asserting on call counts/order
- Test breaks when refactoring without behavior change
- Test name describes HOW not WHAT
- Verifying through external means instead of the interface

```typescript
// BAD: Bypasses the interface to verify against raw storage
test("createUser saves to database", async () => {
  await createUser({ name: "Alice" });
  const row = await db.select().from(users).where(eq(users.name, "Alice"));
  expect(row).toBeDefined();
});

// GOOD: Verifies through the interface (against a real test DB — see mocking.md)
test("createUser makes user retrievable", async () => {
  const user = await createUser({ name: "Alice" });
  const retrieved = await getUser(user.id);
  expect(retrieved.name).toBe("Alice");
});
```

In this repo, prefer a **real SQLite database** in tests over mocking the data layer — exercising the
real query path catches bugs a mocked chain never would.
