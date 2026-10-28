# Local DataWeave preview

`dataweave.rs` interprets source text with a bounded tokenizer, expression tree and evaluator. Its API accepts source and named values, and returns an ordered value tree, rendered text, log observations and media type. The interpreter has no script-name dispatch, filesystem access, network access or Apex runtime context. The Apex bridge supplies project scripts and converts values.

The supported subset covers declared CSV/JSON/Java inputs, CSV separators and escaped quotes, custom functions and literal match cases, selectors, arrays, dynamic object keys/elements, XML attributes, `map`, `mapObject`, `filter`, explicit/implicit lambda arguments, exact rational arithmetic/comparison, logging, typed object annotations, UTC shifts and supported date-format tokens. JSON and XML rendering preserve constructed field order. Pluralization uses general English suffix rules plus an irregular-noun dictionary; it is not a complete linguistic or locale implementation.

Unknown syntax/functions/formats and host-budget exhaustion remain unsupported diagnostics. Division by zero and malformed input are runtime errors. Excel output is explicitly unavailable in Apex and produces a catchable script error; other unimplemented output formats remain unsupported rather than pretending the platform rejects them. No script returns a prerecorded expected result.

Host bounds include 1 MiB source/output, 20,000 tokens, 512 expression nodes, depth 64, 100,000 evaluation steps, 10,000 collection entries, 4 KiB numeric magnitude and a 16 MiB cumulative value-allocation budget. Scopes share immutable inputs to avoid cloning entire input collections for every row. These are implementation safeguards, not Salesforce governor limits.

Unit tests include changed arithmetic/source selectors, changed CSV payloads, malformed quotes/row widths/headers, missing functions, recursive functions, extreme numeric exponents, flat expression trees, XML control characters, exact decimal transformations, dynamic keys and ordered JSON. The optional `dataweave_pinned_sources` integration test executes all 14 unchanged `.dwl` files at the pinned Apex Recipes revision with independent input values; it expects 12 successful transformations and two real error results.

```text
APX_RECIPES_REPO=<unchanged pinned clone>
cargo test --test dataweave_pinned_sources -- --ignored --nocapture
```

Set the environment variable using the syntax for your shell. This source test is explicitly ignored in the ordinary unit suite because it requires an external clone; it does not remove any of the 322 native Apex corpus tests. Run `scripts/corpus.mjs` to measure the complete Apex integration and its negative mutations.

The language/API design follows the [DataWeave function reference](https://docs.mulesoft.com/dataweave/2.9/dw-functions) and [Strings reference](https://dataweave.mulesoft.com/learn/docs.html). No Salesforce or MuleSoft oracle qualification is claimed.
