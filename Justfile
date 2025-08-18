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
    cargo test --manifest-path codetracer-python-recorder/Cargo.toml --no-default-features

py-test:
    uv run --group dev --group test pytest
    
# Run tests only on the pure recorder
test-pure:
    uv run --group dev --group test pytest codetracer-pure-python-recorder

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
