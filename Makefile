SHELL := /bin/sh

CARGO ?= cargo

.DEFAULT_GOAL := build

.PHONY: build release completions check fmt clippy test plugin-deps clean help dev-test prod-test

build:
	$(CARGO) build --workspace --all-features

release:
	$(CARGO) build --workspace --all-features --release

completions:
	$(CARGO) build --package agentix --package taskcli
	mkdir -p completions
	$(CARGO) run --quiet --package agentix -- completions bash > completions/agentix.bash
	$(CARGO) run --quiet --package agentix -- completions zsh > completions/_agentix
	$(CARGO) run --quiet --package agentix -- completions fish > completions/agentix.fish
	$(CARGO) run --quiet --package taskcli -- completions bash > completions/taskcli.bash
	$(CARGO) run --quiet --package taskcli -- completions zsh > completions/_taskcli
	$(CARGO) run --quiet --package taskcli -- completions fish > completions/taskcli.fish

check: fmt clippy test

fmt:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

plugin-deps:
	npm ci --ignore-scripts --prefix plugins/agent-task-manager

test: plugin-deps
	$(CARGO) test --workspace --all-features
	node --test plugins/agent-task-manager/tests/*.test.mjs plugins/agentix-bridge/tests/*.test.mjs

dev-test: build
	cp ./target/debug/taskcli ~/.local/bin
	taskcli obsidian setup || true
	codex plugin remove agent-task-manager@agentix || true
	codex plugin marketplace remove agentix || true
	codex plugin marketplace add . || true
	codex plugin add agent-task-manager@agentix || true
	claude plugin marketplace remove agentix || true
	claude plugin marketplace add ./ || true
	claude plugin install agent-task-manager@agentix || true
	claude plugin install agentix-bridge@agentix || true
	pi install . || true
	omp install . || true

prod-test:
	rm -f ~/.local/bin/taskcli
	taskcli obsidian setup
	codex plugin remove agent-task-manager@agentix
	codex plugin marketplace remove agentix
	codex plugin marketplace add tenfyzhong/agentix
	codex plugin add agent-task-manager@agentix
	claude plugin marketplace remove agentix || true
	claude plugin marketplace add tenfyzhong/agentix
	claude plugin install agent-task-manager@agentix
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
		'make prod-test  Install GitHub plugins for Codex, Claude Code, Pi, and OMP' \
		'make clean    Remove Cargo build artifacts'
