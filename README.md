# accept-header-negotiator

Parses HTTP `Accept` headers with quality factors and selects the best matching content type from a list of candidates.

```rust
use accept_header_negotiator::{negotiate, parse_accept};

let candidates = ["text/html", "application/json"];
let chosen = negotiate("application/json;q=0.9, text/html;q=0.5", &candidates);
assert_eq!(chosen, Some("application/json"));

let entries = parse_accept("text/html;q=0.9, */*;q=0.1").unwrap();
assert_eq!(entries.len(), 2);
```

## Why this exists

Content negotiation is small enough to write inline and fiddly enough to get wrong. The two recurring bugs are treating a missing `q` as `0` instead of `1`, and letting a `*/*` wildcard outrank a concrete match when qualities are equal. This crate makes both decisions explicit and tests them.

The trade-off is that the parser is lenient about parameters it does not understand. A header like `text/html;level=1;q=0.5` keeps the quality and silently drops `level`. Rejecting unknown parameters would be stricter, but real clients send them and servers that reject them break.

## The awkward edge

A quality of `0` is not the same as absence. `text/html;q=0` means "not acceptable", and it stays in the parsed list so it can suppress a later `*/*;q=1` match for that type. `negotiate` will not return a candidate whose only matches have quality `0`, but the entry is still visible through `parse_accept`.

## API

- `parse_accept(&str) -> Result<Vec<AcceptEntry>, ParseError>`
- `negotiate<'a>(&str, &'a [&'a str]) -> Option<&'a str>`
- `AcceptEntry { type_, subtype, quality, position }` with `is_full_wildcard`, `is_subtype_wildcard`, `is_concrete`
- `ParseError::{MalformedMediaRange, InvalidQuality}`
- `quality_eq(f64, f64) -> bool` and `compare_entries(&AcceptEntry, &AcceptEntry) -> Ordering` for callers writing their own selection logic

Quality factors are compared with a tolerance of `1e-9`. Ties are broken by specificity (exact beats `type/*` beats `*/*`), then by the entry's position in the header.
