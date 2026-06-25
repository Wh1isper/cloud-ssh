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

Build the service Docker image locally:

```bash
make docker-build
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

## Docker Publishing

The Docker workflow publishes the service image to GCR.

Required `Release` environment configuration:

- optional variable `GCR_PROJECT_ID`, defaulting to `project_id` from `GCR_JSON_KEY`
- optional variable `GCR_REGISTRY`, defaulting to `gcr.io`
- optional variable `GCR_IMAGE_NAME`, defaulting to `cloud-ssh`
- secret `GCR_JSON_KEY`, containing a service account JSON key with permission to push to the target GCR repository

Nightly builds run on a schedule from `main`. Release builds run when a GitHub Release is published.
