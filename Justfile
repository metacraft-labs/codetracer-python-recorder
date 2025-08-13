# Development helpers for the monorepo

# Print toolchain versions to verify the dev environment
env:
    python3 --version
    cargo --version
    rustc --version
    maturin --version

# Create a local virtualenv for Python tooling
venv:
    test -d .venv || python3 -m venv .venv
    .venv/bin/python -m pip install -U pip

# Build and develop-install the Rust-backed Python module
build-rust:
    test -d .venv || python3 -m venv .venv
    VIRTUAL_ENV=.venv maturin develop -m crates/codetracer-python-recorder/Cargo.toml

# Smoke test the Rust module after build
smoke-rust:
    .venv/bin/python -c "import codetracer_python_recorder as m; print(m.hello())"

# Run the Python test suite for the pure-Python recorder
test:
    python3 -m unittest discover -v
