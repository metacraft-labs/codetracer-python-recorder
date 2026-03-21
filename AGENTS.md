# Instructions for Codex

To build the Rust-backed recorder in dev mode, execute:

```
just dev
```

To build a release wheel, execute:

```
just build
```

To run the full test suite (Rust unit tests + Python integration tests), execute:

```
just test
```

This first builds the dev extension with `maturin develop`, then runs Rust
tests with `cargo nextest` and Python tests with `pytest`.

To run the benchmark, execute:

```
just bench
```

# Codebase structure

This is a monorepo containing two separate recorder implementations:

* `codetracer-python-recorder/` — The main Rust-backed recorder.
  - `src/` — Rust source (PyO3 extension via maturin).
  - `codetracer_python_recorder/` — Python wrapper package (CLI entry point
    at `cli.py`, auto-start hook at `auto_start.py`, trace filters, etc.).
  - `tests/python/` — Python integration and unit tests.
  - `Cargo.toml` — Rust workspace member.
* `codetracer-pure-python-recorder/` — A pure-Python fallback recorder
  (no native extension needed). Used as a prototype and for environments
  where Rust compilation is not available.
* `scripts/` — Helper scripts (version bumping, coverage rendering, etc.).

# You don't have access to the internet

During development, certain commands will fail because you don't have
access to the internet.

The script `.agents/download_internet_resources.sh` is executed before
your development session starts while your computer is still connected
to the internet.

You can examine this script to see what kind of internet resources
have been downloaded for offline use. If it's difficult for you to
achieve a task without access to additional internet resources, you
can always propose a PR that modifies the download.sh script instead
of completing your main task.

Downloading development dependencies may also fail due to the lack of
internet connectivity. We are trying to maintain the script `.agents/codex-setup`
that is also executed before your development session starts while
your computer is still connected to the internet. It tries to run
all build commands that need development dependencies in order to
cache the dependencies for offline use. Please propose changes to
this script when you introduce new build targets with dependencies.

When you need to consult the documentation or source code modules
for a particular dependency, always try to find where this dependency
have been downloaded and try to access the necessary files through
the file system (i.e. depending on the programming language, the
operating system and the package manager being used, they should
be in their standard location).

# Windows development

### Environment setup (auto-installs tools on first run)
```bash
# Git Bash / MSYS2
source env.sh

# PowerShell
. .\env.ps1
```

### Build (Windows)
```bash
uv sync                    # create venv and install deps
uv run maturin develop     # build Rust extension into venv
```

### Test (Windows)
```bash
cargo test                 # Rust unit tests
uv run pytest              # Python integration tests
```

### Dependencies
- Rust 1.92.0 (via bootstrap)
- Cap'n Proto 1.3.0 (via bootstrap)
- uv 0.9.28 (via bootstrap)
- Python 3.12+ (managed by uv)

# Code quality guidelines

- ALWAYS strive to achieve high code quality.
- ALWAYS write secure code.
- ALWAYS make sure the code is well tested and edge cases are covered. Design the code for testability and be extremely thorough.
- ALWAYS write defensive code and make sure all potential errors are handled.
- ALWAYS strive to write highly reusable code with routines that have high fan in and low fan out.
- ALWAYS keep the code DRY.
- Aim for low coupling and high cohesion. Encapsulate and hide implementation details.
- Rust code uses `cargo clippy` with `-D clippy::panic` and `cargo fmt`.
  No `.unwrap()` calls in production Rust code — use proper error handling.
- Python code is formatted with `ruff format` and linted with `ruff check`.
- Nix files are formatted with `nixfmt`.

# Code commenting guidelines

- Document public APIs and complex modules using standard code documentation conventions.
- Comment the intention behind your code extensively. Omit comments only for very obvious
  facts that almost any developer would know.
- Maintain the comments together with the code to keep them meaningful and current.
- When the code is based on specific formats, standards or well-specified behavior of
  other software, always make sure to include relevant links (URLs) that provide the
  necessary technical details.

# Writing git commit messages

- You MUST use multiline git commit messages.
- Use the conventional commits style for the first line of the commit message.
- Use the summary section of your final response as the remaining lines in the commit message.
