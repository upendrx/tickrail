# Common tasks. Plain cargo works too; this just saves typing.

VENV ?= .venv
PY := $(VENV)/bin/python

.PHONY: help build test lint fmt bench run docs python python-test sample-data clean

help: ## Show this list
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-12s %s\n", $$1, $$2}'

build: ## Release build of the tickrail binary
	cargo build --release

test: ## Rust tests for every crate
	cargo test --workspace

lint: ## rustfmt check and clippy with warnings as errors
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt: ## Format all Rust code
	cargo fmt --all

bench: ## Criterion benchmarks (ring, book, matching engine)
	cargo bench --bench ring
	cargo bench --bench book
	cargo bench --bench matching

run: build ## Run the built-in simulator with the web console on :8080
	./target/release/tickrail run

docs: ## Build the developer book into docs/book (needs `cargo install mdbook`)
	mdbook build docs

$(VENV):
	python3 -m venv $(VENV)
	$(VENV)/bin/pip install -q maturin pytest polars matplotlib

python: $(VENV) ## Build and install the Python bindings into .venv
	cd bindings/python && ../../$(VENV)/bin/maturin build --release -i ../../$(PY) -o ../../target/wheels
	$(VENV)/bin/pip install -q --force-reinstall target/wheels/tickrail-*.whl

python-test: python build ## Python binding tests
	$(PY) -m pytest -q bindings/python/tests

sample-data: ## Regenerate examples/data/sample.csv
	python3 scripts/make_sample_data.py

clean: ## Remove build output
	cargo clean
	rm -rf bindings/python/target target/wheels
