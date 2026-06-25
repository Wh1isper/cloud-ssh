# Development

## Prerequisites

- Rust stable toolchain
- `pre-commit`
- `mdbook` when building docs locally

## Common Commands

```bash
make fmt-check
make check
make test
```

Run the placeholder server binary:

```bash
make run
```

Run the full local gate:

```bash
make ci
```

Install developer hooks:

```bash
make install
```

Run all hooks:

```bash
make lint
```

Build docs:

```bash
make docs-build
```
