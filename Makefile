SHELL := /bin/sh

CARGO ?= cargo

.DEFAULT_GOAL := build

.PHONY: build release completions check fmt clippy test plugin-deps clean help

build:
	$(CARGO) build --workspace --all-features

release:
	$(CARGO) build --workspace --all-features --release

completions:
	$(CARGO) build --package agentix
	mkdir -p completions
	$(CARGO) run --quiet --package agentix -- completions bash > completions/agentix.bash
	$(CARGO) run --quiet --package agentix -- completions zsh > completions/_agentix
	$(CARGO) run --quiet --package agentix -- completions fish > completions/agentix.fish

check: fmt clippy test

fmt:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

plugin-deps:
	npm ci --ignore-scripts --prefix plugins/agent-task-manager

test: plugin-deps
	$(CARGO) test --workspace --all-features
	node --test plugins/agent-task-manager/tests/*.test.mjs

clean:
	$(CARGO) clean

help:
	@printf '%s\n' \
		'make          Build the workspace in debug mode' \
		'make release  Build the workspace in release mode' \
		'make completions  Regenerate bash, zsh, and fish completion files' \
		'make check    Run formatting, lint, and tests' \
		'make clean    Remove Cargo build artifacts'
