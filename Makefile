SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help

DEV_COMPOSE := docker compose --file dev/compose.yml
BUILD_REVISION := $(shell git rev-parse --short=12 HEAD 2>/dev/null || printf unknown)

.PHONY: install
install: ## Install locked Rust, JavaScript, and Browser-test dependencies
	@cargo fetch --locked
	@pnpm install --frozen-lockfile
	@pnpm --filter @owlmux/web exec playwright install chromium

.PHONY: format
format: ## Format Rust and Web sources
	@cargo fmt --all
	@pnpm format

.PHONY: check
check: contracts-check web-build docs-build ## Run formatting, linting, static, docs, and repository checks
	@cargo fmt --all --check
	@cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
	@pnpm check
	@pnpm --filter @owlmux/docs run deploy --dry-run
	@python3 scripts/check-markdown-links.py
	@python3 scripts/check-repository.py
	@$(DEV_COMPOSE) config --quiet

.PHONY: test
test: web-build ## Run Rust and Web tests
	@cargo test --workspace --all-features --locked
	@pnpm test

.PHONY: test-containers
test-containers: ## Run Docker-backed PostgreSQL integration tests
	@OWLMUX_REQUIRE_DOCKER=1 cargo test --package owlmux-server --all-features --locked container_

.PHONY: test-e2e
test-e2e: web-build ## Run isolated Docker E2E for PostgreSQL, Relay, SSH, and target-owned tmux
	@cargo build --locked --package owlmux-server --package owlmux-relay
	@scripts/docker/e2e-blocks-0-3.sh

.PHONY: test-e2e-matrix
test-e2e-matrix: web-build ## Run Docker E2E across distribution and current-upstream tmux
	@cargo build --locked --package owlmux-server --package owlmux-relay
	@scripts/docker/e2e-tmux-matrix.sh

.PHONY: build
build: web-build docs-build ## Build both release binaries and public assets
	@cargo build --release --locked --package owlmux-server --package owlmux-relay

.PHONY: contracts-generate
contracts-generate: ## Generate Rust and TypeScript bindings from reviewed contracts
	@python3 scripts/contracts/generate.py

.PHONY: contracts-check
contracts-check: ## Verify generated contract bindings are current
	@python3 scripts/contracts/generate.py --check

.PHONY: web-build
web-build: contracts-check ## Build the Web application
	@pnpm build

.PHONY: web-dev
web-dev: ## Run the Vite development server
	@pnpm dev

.PHONY: dev
dev: web-build dev-up ## Run the Server with production Web assets and disposable development configuration
	@set -a; source dev/server.env; set +a; cargo run --locked --package owlmux-server

.PHONY: docs
docs: ## Run the documentation development server
	@pnpm docs:dev

.PHONY: docs-build
docs-build: ## Build documentation for deployment
	@pnpm docs:build

.PHONY: docs-deploy
docs-deploy: ## Deploy documentation to Cloudflare Workers
	@pnpm docs:deploy

.PHONY: dev-check
dev-check: ## Validate Docker and development infrastructure configuration
	@command -v docker >/dev/null
	@docker compose version >/dev/null
	@docker info >/dev/null
	@test -d node_modules
	@$(DEV_COMPOSE) config --quiet

.PHONY: dev-up
dev-up: dev-check ## Start PostgreSQL development infrastructure
	@$(DEV_COMPOSE) up --detach --wait

.PHONY: dev-target-up
dev-target-up: dev-check ## Start PostgreSQL and the loopback sshd/tmux target fixture
	@$(DEV_COMPOSE) --profile target up --detach --build --wait

.PHONY: dev-target-status
dev-target-status: ## Show PostgreSQL and target fixture status
	@$(DEV_COMPOSE) --profile target ps

.PHONY: dev-down
dev-down: ## Stop development infrastructure
	@$(DEV_COMPOSE) --profile target down --remove-orphans

.PHONY: dev-reset
dev-reset: ## Recreate development infrastructure and remove local data
	@$(DEV_COMPOSE) down --volumes --remove-orphans
	@$(DEV_COMPOSE) up --detach --wait

.PHONY: dev-status
dev-status: ## Show development infrastructure status
	@$(DEV_COMPOSE) ps

.PHONY: dev-postgres
dev-postgres: ## Open psql inside the development PostgreSQL container
	@$(DEV_COMPOSE) exec postgres psql --username "$${OWLMUX_POSTGRES_USER:-owlmux}" --dbname "$${OWLMUX_POSTGRES_DB:-owlmux}"

.PHONY: dev-logs
dev-logs: ## Follow development infrastructure logs
	@$(DEV_COMPOSE) logs --follow --tail=100

.PHONY: docker-build
docker-build: ## Build and smoke-test the production Server image
	@docker build --build-arg VCS_REF=$(BUILD_REVISION) --build-arg OWLMUX_BUILD_REVISION=$(BUILD_REVISION) --tag owlmux:dev .
	@scripts/docker/smoke-server-image.sh owlmux:dev

.PHONY: lint
lint: ## Run repository pre-commit hooks
	@pre-commit run --all-files

.PHONY: help
help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; printf "Usage: make <target>\n\nTargets:\n"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
