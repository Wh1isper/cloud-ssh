SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help

DEV_COMPOSE := docker compose --file dev/compose.yml

.PHONY: install
install: ## Install locked Rust and JavaScript dependencies
	@cargo fetch --locked
	@pnpm install --frozen-lockfile

.PHONY: format
format: ## Format Rust and Web sources
	@cargo fmt --all
	@pnpm format

.PHONY: check
check: web-build docs-build ## Run formatting, linting, static, docs, and repository checks
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

.PHONY: build
build: web-build docs-build ## Build both release binaries and public assets
	@cargo build --release --locked --package owlmux-server --package owlmux-relay

.PHONY: web-build
web-build: ## Build the placeholder Web application
	@pnpm build

.PHONY: web-dev
web-dev: ## Run the Vite development server
	@pnpm dev

.PHONY: dev
dev: web-build ## Run the placeholder Server with production Web assets
	@cargo run --locked --package owlmux-server

.PHONY: docs
docs: ## Run the documentation development server
	@pnpm docs:dev

.PHONY: docs-build
docs-build: ## Build documentation for deployment
	@pnpm docs:build

.PHONY: docs-deploy
docs-deploy: ## Deploy documentation to Cloudflare Workers
	@pnpm docs:deploy

.PHONY: dev-up
dev-up: ## Start PostgreSQL and Redis development infrastructure
	@$(DEV_COMPOSE) up --detach --wait

.PHONY: dev-down
dev-down: ## Stop development infrastructure
	@$(DEV_COMPOSE) down --remove-orphans

.PHONY: dev-reset
dev-reset: ## Recreate development infrastructure and remove local data
	@$(DEV_COMPOSE) down --volumes --remove-orphans
	@$(DEV_COMPOSE) up --detach --wait

.PHONY: dev-status
dev-status: ## Show development infrastructure status
	@$(DEV_COMPOSE) ps

.PHONY: dev-logs
dev-logs: ## Follow development infrastructure logs
	@$(DEV_COMPOSE) logs --follow --tail=100

.PHONY: docker-build
docker-build: ## Build and smoke-test the production Server image
	@docker build --tag owlmux:dev .
	@scripts/docker/smoke-server-image.sh owlmux:dev

.PHONY: lint
lint: ## Run repository pre-commit hooks
	@pre-commit run --all-files

.PHONY: help
help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; printf "Usage: make <target>\n\nTargets:\n"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
