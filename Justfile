# Development helpers for the monorepo

# Python versions used for multi-version testing/building with uv
PY_VERSIONS := "3.10 3.11 3.12 3.13"
PY_SHORT_VERSIONS := "10 11 12 13"
# Print toolchain versions to verify the dev environment
env:
    python3 --version
    cargo --version
    rustc --version
    maturin --version

# Create a local virtualenv for Python tooling
venv:
    test -d .venv || python3 -m venv .venv

# Build and develop-install the Rust-backed Python module
build-rust:
    test -d .venv || python3 -m venv .venv
    VIRTUAL_ENV=.venv maturin develop -m crates/codetracer-python-recorder/Cargo.toml

# Smoke test the Rust module after build
smoke-rust:
    .venv/bin/python -m pip install -U pip pytest
    .venv/bin/python -m pytest crates/codetracer-python-recorder/test -q

# Run the Python test suite for the pure-Python recorder
test:
    python3 -m unittest discover -v

# Run the test suite across multiple Python versions using uv
test-uv-all:
    uv python install {{PY_VERSIONS}}
    for v in {{PY_VERSIONS}}; do uv run -p "$v" -m unittest discover -v; done

# Build wheels for all target Python versions with maturin
build-rust-uv-all:
    for v in {{PY_VERSIONS}}; do \
        maturin build --interpreter "python$v" -m crates/codetracer-python-recorder/Cargo.toml --release; \
    done

# Smoke the built Rust wheels across versions using uv
smoke-rust-uv-all:
    for v in {{PY_SHORT_VERSIONS}}; do \
        file=(crates/codetracer-python-recorder/target/wheels/codetracer_python_recorder-*-cp3$v-cp3$v-*.whl); \
        file="${file[0]}"; \
        uv run -p "python3.$v" --with "${file}" --with pytest -- python -m pytest crates/codetracer-python-recorder/test -q; \
    done
