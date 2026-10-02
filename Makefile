SHELL := /bin/sh

CARGO ?= cargo
PYTHON ?= python3
BREW ?= brew
CODEX ?= codex
PI ?= pi
OMP ?= omp
CLAUDE ?= claude
HOSTS ?= codex pi omp claude
VERSION ?= stable
FORMULAE ?= agentix taskix
SOURCE ?= main
PROFILE ?= release
BREW_FORMULAE = $(addprefix tenfyzhong/tap/,$(FORMULAE))
DEBUG_TARGET_DIR = $(if $(CARGO_TARGET_DIR),$(CARGO_TARGET_DIR),target)

.DEFAULT_GOAL := build

.PHONY: build release completions check fmt clippy test test-backup clean help remove-plugin link-debug install-local install update switch plugin

build:
	$(CARGO) build --workspace --all-features

link-debug:
	$(CARGO) build --package agentix --package taskix --all-features --target-dir "$(DEBUG_TARGET_DIR)"
	@set -eu; \
	debug_dir=$$(cd "$(DEBUG_TARGET_DIR)/debug" && pwd -P); \
	test -x "$$debug_dir/agentix"; \
	test -x "$$debug_dir/taskix"; \
	prefix=$$($(BREW) --prefix); \
	$(BREW) unlink agentix taskix; \
	ln -sfn "$$debug_dir/agentix" "$$prefix/bin/agentix"; \
	ln -sfn "$$debug_dir/taskix" "$$prefix/bin/taskix"

release:
	$(CARGO) build --workspace --all-features --release

install-local:
	@case "$(PROFILE)" in release|debug) ;; *) echo 'PROFILE must be release or debug' >&2; exit 2 ;; esac
	@test -n "$(strip $(FORMULAE))" || { echo 'FORMULAE must not be empty' >&2; exit 2; }
	@for formula in $(FORMULAE); do \
		case "$$formula" in agentix|taskix) ;; *) echo 'FORMULAE must contain only agentix or taskix' >&2; exit 2 ;; esac; \
	done
	HOMEBREW_AGENTIX_LOCAL_SOURCE="$(CURDIR)" HOMEBREW_AGENTIX_LOCAL_PROFILE="$(PROFILE)" \
	HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1 HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1 \
	$(BREW) reinstall --build-from-source $(BREW_FORMULAE)

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

test: test-backup
	$(CARGO) test --workspace --all-features
	node --test plugins/taskix-manager/tests/*.test.mjs plugins/agentix-bridge/tests/*.test.mjs

test-backup:
	$(PYTHON) -m unittest discover -s scripts/tests -v

# Explicit install specs avoid inheriting HEAD from an existing installation.
install update:
	@case "$(VERSION)" in stable|head) ;; *) echo 'VERSION must be stable or head' >&2; exit 2 ;; esac
	$(BREW) update
	$(BREW) install $(if $(filter head,$(VERSION)),--HEAD $(if $(filter update,$@),--fetch-HEAD)) --skip-link $(BREW_FORMULAE)
	$(MAKE) switch VERSION="$(VERSION)" FORMULAE="$(FORMULAE)"

# Validate every requested keg before unlinking any commands. In particular,
# plain brew link can fall back to HEAD when no stable keg exists.
# unlink by formula follows opt, which --skip-link may already have moved.
# Use Homebrew's locked unlink operation on the actual linked keg instead.
switch:
	@set -eu; \
	case "$(VERSION)" in stable|head) ;; *) echo 'VERSION must be stable or head' >&2; exit 2 ;; esac; \
	test -n "$(strip $(FORMULAE))"; \
	for formula in $(BREW_FORMULAE); do \
		versions=$$($(BREW) list --versions "$$formula"); \
		if ! printf '%s\n' "$$versions" | awk -v version="$(VERSION)" '{ for (i = 2; i <= NF; i++) if ((version == "head") == ($$i ~ /^HEAD-/)) found = 1 } END { exit !found }'; then \
			echo "$$formula: $(VERSION) is not installed; run make install VERSION=$(VERSION)" >&2; \
			exit 1; \
		fi; \
	done; \
	$(BREW) ruby -e 'require "keg"; require "unlink"; ARGV.each { |name| ref = HOMEBREW_LINKED_KEGS/name.split("/").last; Homebrew::Unlink.unlink(Keg.new(ref.realpath)) if ref.symlink? }' $(BREW_FORMULAE); \
	$(BREW) link $(if $(filter head,$(VERSION)),--HEAD) $(BREW_FORMULAE)

# Validate the complete selection before removing anything.
define check-plugin-hosts
	@set -eu; \
	test -n "$(strip $(HOSTS))" || { echo 'HOSTS must not be empty' >&2; exit 2; }; \
	for host in $(HOSTS); do \
		case "$$host" in \
			codex) command -v "$(CODEX)" >/dev/null ;; \
			pi) command -v "$(PI)" >/dev/null ;; \
			omp) command -v "$(OMP)" >/dev/null ;; \
			claude) command -v "$(CLAUDE)" >/dev/null ;; \
			*) echo "Unknown plugin host: $$host" >&2; exit 2 ;; \
		esac || { echo "Missing $$host CLI; install it or adjust HOSTS" >&2; exit 1; }; \
	done
endef

remove-plugin:
	$(check-plugin-hosts)
	@set -eu; \
	for host in $(HOSTS); do \
		case "$$host" in \
			codex) \
				$(CODEX) plugin remove taskix-manager@agentix || true; \
				$(CODEX) plugin marketplace remove agentix || true ;; \
			pi) \
				$(PI) remove . || true; \
				$(PI) remove git:github.com/tenfyzhong/agentix || true; \
				$(PI) remove git:github.com/tenfyzhong/agentix@main || true ;; \
			omp) \
				$(OMP) plugin uninstall agentix-plugins || true; \
				$(OMP) plugin uninstall taskix-manager@agentix || true; \
				$(OMP) plugin uninstall agentix-bridge@agentix || true; \
				$(OMP) plugin marketplace remove agentix || true ;; \
			claude) \
				$(CLAUDE) plugin uninstall taskix-manager@agentix || true; \
				$(CLAUDE) plugin uninstall agentix-bridge@agentix || true; \
				$(CLAUDE) plugin marketplace remove agentix || true ;; \
		esac; \
	done

plugin:
	@case "$(SOURCE)" in local|main) ;; *) echo 'SOURCE must be local or main' >&2; exit 2 ;; esac
	$(check-plugin-hosts)
	$(MAKE) remove-plugin
	@set -eu; \
	for host in $(HOSTS); do \
		case "$$host" in \
			codex) \
				$(CODEX) plugin marketplace add $(if $(filter main,$(SOURCE)),tenfyzhong/agentix --ref main,./); \
				$(CODEX) plugin add taskix-manager@agentix ;; \
			pi) \
				$(PI) install $(if $(filter main,$(SOURCE)),git:github.com/tenfyzhong/agentix@main,.) ;; \
			omp) \
				$(OMP) plugin marketplace add $(if $(filter main,$(SOURCE)),tenfyzhong/agentix,./); \
				$(OMP) plugin install taskix-manager@agentix; \
				$(OMP) plugin install agentix-bridge@agentix ;; \
			claude) \
				$(CLAUDE) plugin marketplace add $(if $(filter main,$(SOURCE)),tenfyzhong/agentix@main,./); \
				$(CLAUDE) plugin install taskix-manager@agentix; \
				$(CLAUDE) plugin install agentix-bridge@agentix ;; \
		esac; \
	done

clean:
	$(CARGO) clean

help:
	@printf '%s\n' \
		'make          Build the workspace in debug mode' \
		'make release  Build the workspace in release mode' \
		'make completions  Regenerate bash, zsh, and fish completions for both CLIs' \
		'make check    Run formatting, lint, and tests' \
		'make link-debug  Build debug CLIs and replace Homebrew command links' \
		'make install-local [PROFILE=release|debug]  Build local source through Homebrew Formulae (default: release)' \
		'make install [VERSION=stable|head]  Install and use both Homebrew CLIs (default: stable)' \
		'make update [VERSION=stable|head]  Update and use the selected version' \
		'make switch [VERSION=stable|head]  Use an already installed version without downloading' \
		'  FORMULAE=agentix or FORMULAE=taskix selects one CLI (default: both)' \
		'make plugin [SOURCE=local|main]  Install plugins for all four hosts (default: main)' \
		'  HOSTS="codex pi omp claude" selects plugin hosts (default: all four)' \
		'make remove-plugin  Remove Agentix plugins for selected hosts' \
		'make clean    Remove Cargo build artifacts'
