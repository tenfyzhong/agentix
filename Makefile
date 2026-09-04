SHELL := /bin/sh

CARGO ?= cargo

.DEFAULT_GOAL := build

.PHONY: build release check fmt clippy test clean help

build:
	$(CARGO) build --workspace --all-features

release:
	$(CARGO) build --workspace --all-features --release

check: fmt clippy test

fmt:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

test:
	$(CARGO) test --workspace --all-features

clean:
	$(CARGO) clean

help:
	@printf '%s\n' \
		'make          Build the workspace in debug mode' \
		'make release  Build the workspace in release mode' \
		'make check    Run formatting, lint, and tests' \
		'make clean    Remove Cargo build artifacts'
