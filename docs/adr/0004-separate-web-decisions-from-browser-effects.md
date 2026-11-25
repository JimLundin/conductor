# Separate web decisions from browser effects

The web workspace has three packages. `@conductor/core` owns data,
folds, routes, ticket decoding, commands and view models. The `web` package
owns React rendering and browser effects. `@conductor/schemas` runs the
Node schema generator through Effect.

The store holds its view in an Effect atom. Its registry runs updates and
commands, while scoped fibers own host connections. React reads the view
through `@effect/atom-react`. Browser storage, events and random values use
Effect platform services.

## Consequences

- Core exports describe data and pure decisions without browser services.
- UI components render models from core and send actions through the store.
- Tests supply browser services and use `@effect/vitest` for effects and
  properties. Recorded feeds remain the source for replay assertions.
- Internal absence uses `undefined`; nullable client link fields remain
  JSON null to match the generated Rust contract.
- Exceptions name a generated-code or library boundary. There are no
  migration exceptions that waive checks across the source tree.
