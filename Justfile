mod agents 

default:
    @just --list
    
# Development helpers for the monorepo

# Python version used for development
PYTHON_DEFAULT_VERSION := "3.13"

# Python versions used for multi-version testing/building with uv
PY_VERSIONS := "3.10 3.11 3.12 3.13"
PY_SHORT_VERSIONS := "10 11 12 13"

# Print toolchain versions to verify the dev environment
env:
    uv --version
    python3 --version
    cargo --version
    rustc --version
    maturin --version

clean:
    rm -rf .venv **/__pycache__ **/*.pyc **/*.pyo **/.pytest_cache
    rm -rf codetracer-python-recorder/target codetracer-python-recorder/**/*.so


# Create a clean local virtualenv for Python tooling (without editable packages installed)
venv version=PYTHON_DEFAULT_VERSION:
    uv sync -p {{version}}

# Build the module in dev mode
dev:
    uv run --directory codetracer-python-recorder maturin develop --uv

# Run unit tests of dev build
test: cargo-test py-test

# Run Rust unit tests without default features to link Python C library
cargo-test:
    uv run cargo nextest run --manifest-path codetracer-python-recorder/Cargo.toml --no-default-features

py-test:
    uv run --group dev --group test pytest codetracer-python-recorder/tests/python codetracer-pure-python-recorder
    
# Run tests only on the pure recorder
test-pure:
    uv run --group dev --group test pytest codetracer-pure-python-recorder

# Generate combined coverage artefacts for both crates
alias cov := coverage
coverage:
    just coverage-rust
    just coverage-python

coverage-rust:
    mkdir -p codetracer-python-recorder/target/coverage/rust
    LLVM_COV="$(command -v llvm-cov)" LLVM_PROFDATA="$(command -v llvm-profdata)" \
        uv run cargo llvm-cov nextest --manifest-path codetracer-python-recorder/Cargo.toml --no-default-features --lcov --output-path codetracer-python-recorder/target/coverage/rust/lcov.info
    LLVM_COV="$(command -v llvm-cov)" LLVM_PROFDATA="$(command -v llvm-profdata)" \
        uv run cargo llvm-cov report --summary-only --json --manifest-path codetracer-python-recorder/Cargo.toml --output-path codetracer-python-recorder/target/coverage/rust/summary.json
    python3 codetracer-python-recorder/scripts/render_rust_coverage_summary.py \
        codetracer-python-recorder/target/coverage/rust/summary.json --root "$(pwd)"

coverage-python:
    mkdir -p codetracer-python-recorder/target/coverage/python
    uv run --group dev --group test pytest --cov=codetracer_python_recorder --cov-report=term --cov-report=xml:codetracer-python-recorder/target/coverage/python/coverage.xml codetracer-python-recorder/tests/python

# Build the module in release mode
build:
    just venv \
    uv run --directory codetracer-python-recorder maturin build --release

# Build wheels for all target Python versions with maturin
build-all:
    just venv
    uv run --directory codetracer-python-recorder maturin build --release --interpreter {{PY_VERSIONS}}

# Smoke the built Rust wheels across versions using uv
test-all:
    for v in {{PY_SHORT_VERSIONS}}; do \
        file=(codetracer-python-recorder/target/wheels/codetracer_python_recorder-*-cp3$v-cp3$v-*.whl); \
        file="${file[0]}"; \
        uv run -p "python3.$v" --with "${file}" --with pytest -- pytest -q; \
    done
