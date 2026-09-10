SHELL := /bin/sh

CARGO ?= cargo

.DEFAULT_GOAL := build

.PHONY: build release completions check fmt clippy test plugin-deps clean help remove-plugin dev-test prod-test

build:
	$(CARGO) build --workspace --all-features

release:
	$(CARGO) build --workspace --all-features --release

completions:
	$(CARGO) build --package agentix --package taskix
	mkdir -p completions
	$(CARGO) run --quiet --package agentix -- completions bash > completions/agentix.bash
	$(CARGO) run --quiet --package agentix -- completions zsh > completions/_agentix
	$(CARGO) run --quiet --package agentix -- completions fish > completions/agentix.fish
	$(CARGO) run --quiet --package taskix -- completions bash > completions/taskix.bash
	$(CARGO) run --quiet --package taskix -- completions zsh > completions/_taskix
	$(CARGO) run --quiet --package taskix -- completions fish > completions/taskix.fish

check: fmt clippy test

fmt:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

plugin-deps:
	npm ci --ignore-scripts --prefix plugins/taskix-manager

test: plugin-deps
	$(CARGO) test --workspace --all-features
	node --test plugins/taskix-manager/tests/*.test.mjs plugins/agentix-bridge/tests/*.test.mjs

remove-plugin:
	codex plugin remove taskix-manager@agentix || true
	codex plugin marketplace remove agentix || true
	claude plugin uninstall taskix-manager@agentix || true
	claude plugin uninstall agentix-bridge@agentix || true
	claude plugin marketplace remove agentix || true
	pi remove . || true
	pi remove git:github.com/tenfyzhong/agentix || true
	omp plugin uninstall agentix-plugins || true

dev-test: remove-plugin
	codex plugin marketplace add .
	codex plugin add taskix-manager@agentix
	claude plugin marketplace add ./
	claude plugin install taskix-manager@agentix
	claude plugin install agentix-bridge@agentix
	pi install .
	omp install .

prod-test: remove-plugin
	codex plugin marketplace add tenfyzhong/agentix
	codex plugin add taskix-manager@agentix
	claude plugin marketplace add tenfyzhong/agentix
	claude plugin install taskix-manager@agentix
	claude plugin install agentix-bridge@agentix
	pi install git:github.com/tenfyzhong/agentix
	omp install github:tenfyzhong/agentix

clean:
	$(CARGO) clean

help:
	@printf '%s\n' \
		'make          Build the workspace in debug mode' \
		'make release  Build the workspace in release mode' \
		'make completions  Regenerate bash, zsh, and fish completions for both CLIs' \
		'make check    Run formatting, lint, and tests' \
		'make dev-test  Install local plugins for Codex, Claude Code, Pi, and OMP' \
		'make remove-plugin  Remove Agentix marketplaces, plugins, and extensions' \
		'make prod-test  Install GitHub plugins for Codex, Claude Code, Pi, and OMP' \
		'make clean    Remove Cargo build artifacts'
