# RFC 8785 fixtures

Shared, repository-level conformance vectors for JSON canonicalization. They
describe the *standard*, not any one crate's implementation, so they live here
rather than inside whichever crate happens to canonicalize today
(`broker/src/jcs.rs` — `docs/protocol.md` §6 says why that is the broker's).

Each case is a pair:

- `<name>.input.json` — the document to canonicalize;
- `<name>.expected.json` — the exact canonical bytes, **no trailing newline**.

Both files parse to the same JSON value; only their serialization differs. Byte
equality against the expected file is the assertion, so an editor that
reformats, reorders, or adds a trailing newline breaks the vector — that is the
point.
