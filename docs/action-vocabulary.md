# Where the action vocabulary lives

`spider-cloud-agent` depends on `spider-route`, never the other way round. That leaves a
question with two bad answers and one good one: which crate owns the enums that name a
request mode, a proxy pool, a country and a wait condition?

`spider-route` owns them. It is the crate with no network, no async runtime and no HTTP
client, so it can define the vocabulary without dragging anything into a consumer that
only wants the classifier. `spider-cloud-agent` re-exports those types and implements the
serde representation for the wire on its own side.

The two answers we rejected:

**Each crate defines its own copy.** Two enums that must agree forever, with a conversion
in the middle. It works on day one and drifts by the third release, and the drift is
silent because a conversion that handles every variant still compiles after someone adds
one to only one side.

**`spider-route` depends on `spider-cloud-agent` for the types.** That inverts the
dependency and makes the classifier crate pull in an HTTP stack it never calls.

The consequence to remember: a new routing action is a change in `spider-route` first. It
is a new label the model has to learn and a new column the trainer has to carry, which is
exactly the friction that should exist before the curated surface grows.

The curated method membership is checked by `curated_surface_membership_is_explicit`.
A promotion needs a matching test change and a row here, so its reason stays visible.

| Method | Reason for inclusion |
| --- | --- |
| `mode` | Choose whether the page needs rendering. |
| `proxy` | Choose the request's proxy pool. |
| `country` | Choose the country the site serves. |
| `wait_for` | Let the needed content arrive. |
| `profile` | Set a coarse identity and viewport. |
| `timeout` | Bound the time for one page. |
| `session` | Keep state across requests when needed. |
| `budget` | Bound what the operation may spend. |
| `need` | Ask only for the output the caller wants. |
| `max_tokens` | Bound the returned text. |
| `params_mut` | Reach the full documented parameter set. |
| `page_links` | Reuse a fetched page to return its links, on page operations only. |
