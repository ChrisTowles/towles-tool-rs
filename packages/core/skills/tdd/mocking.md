# When to Mock

In this repo, **`vi.mock` is banned** (it's an oxlint error). Achieve substitution through
**constructor / parameter dependency injection** instead, and only at system boundaries.

Substitute at **system boundaries** only:

- External APIs (payment, email, etc.) — inject a fake implementation of the boundary interface
- Databases — **prefer a real test DB (SQLite).** Do NOT mock Drizzle query chains; exercise the real
  query path
- Time / randomness — inject a clock or seed
- File system (sometimes) — inject a path or fs-like interface

Don't substitute:

- Your own classes/modules
- Internal collaborators
- Anything you control

## Designing for substitutability

**1. Use dependency injection — pass dependencies in, don't create them internally**

```typescript
// Easy to substitute (inject a fake paymentClient in tests)
function processPayment(order, paymentClient) {
  return paymentClient.charge(order.total);
}

// Hard to substitute (constructs its own client)
function processPayment(order) {
  const client = new StripeClient(process.env.STRIPE_KEY);
  return client.charge(order.total);
}
```

Inject a hand-written fake that implements the boundary interface — no `vi.mock`, no auto-mocking.

**2. Prefer SDK-style interfaces over generic fetchers**

Specific functions per external operation beat one generic function with conditional logic:

```typescript
// GOOD: each function is independently fakeable
const api = {
  getUser: (id) => fetch(`/users/${id}`),
  getOrders: (userId) => fetch(`/users/${userId}/orders`),
  createOrder: (data) => fetch("/orders", { method: "POST", body: data }),
};

// BAD: faking requires conditional logic inside the fake
const api = {
  fetch: (endpoint, options) => fetch(endpoint, options),
};
```

The SDK approach means: each fake returns one specific shape, no conditional logic in test setup, easy
to see which endpoints a test exercises, type safety per endpoint.

**3. Manual DI only at the boundary**

Don't thread injected dependencies through every internal layer for testability. Inject at the seam
where the system meets the outside world; keep the interior wired normally.
