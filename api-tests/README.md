# Debug Tests

## Testcontainers for Docker Backend

```
export RUST_LOG = "info,testcontainers=debug"
cargo test --manifest-path api-tests/Cargo.toml --test docker -- --test-threads=1 --nocapture
```
