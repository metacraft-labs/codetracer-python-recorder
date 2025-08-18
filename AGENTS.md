# Instructions for Codex

This repository contains two related projects:

- codetracer-pure-python-recorder — the original pure-Python tracer.
- codetracer-python-recorder — a Rust-backed Python module built with PyO3 and maturin.

To build the modules in development mode run:

```sh
just venv 3.13 dev #You can use any other Python version >=3.12
``

Then to run the tests do

```sh
just test
```

# Code quality guidelines

- Strive to achieve high code quality.
- Write secure code.
- Make sure the code is well tested and edge cases are covered. Design the code for testability.
- Write defensive code and make sure all potential errors are handled.
- Strive to write highly reusable code with routines that have high fan in and low fan out.
- Keep the code DRY.
- Aim for low coupling and high cohesion. Encapsulate and hide implementation details.

# Code commenting guidelines

- Document public APIs and complex modules.
- Maintain the comments together with the code to keep them meaningful and current.
- Comment intention and rationale, not obvious facts. Write self-documenting code.
- When implementing specific formats, standards or other specifications, make sure to
  link to the relevant spec URLs.

# Writing git commit messages

The first line of the commit message should follow the "conventional commits" style:
https://www.conventionalcommits.org/en/v1.0.0/

In the remaining lines, provide a short description of the implemented functionality.
Provide sufficient details for the justification of each design decision if multiple
approaches were considered.
