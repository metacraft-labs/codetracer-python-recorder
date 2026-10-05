#!/usr/bin/env python3
"""Check real Python code indentation without interpreting literal text as code.

Every Git-tracked Python file is parsed and tokenized. No files or syntax errors
are excluded. Literal contents and valid continuation alignment remain intact.
This supplements EditorConfig for exact documented literal-bearing files.
"""
from __future__ import annotations

import ast
import io
import os
from pathlib import Path
import subprocess
import tokenize


def check_file(path: Path) -> None:
    data = path.read_bytes()
    # Preserve Python's declared source encoding and reject genuine syntax errors.
    encoding, _ = tokenize.detect_encoding(io.BytesIO(data).readline)
    ast.parse(data.decode(encoding), filename=str(path))
    for token in tokenize.tokenize(io.BytesIO(data).readline):
        if token.type == tokenize.INDENT:
            indentation = token.string
            if any(character != " " for character in indentation) or len(indentation) % 4:
                raise ValueError(f"{path}:{token.start[0]}: code indentation must use multiples of four spaces")


def main() -> None:
    root = Path.cwd().resolve()
    actual_root = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
    if Path(actual_root).resolve() != root:
        raise RuntimeError("run Python indentation validation from owning Git root")
    names = subprocess.check_output(["git", "ls-files", "-z"]).split(b"\0")
    count = 0
    for name in names:
        if not name or not name.endswith(b".py"):
            continue
        path = root / os.fsdecode(name)
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f"tracked Python source is not a regular file: {path}")
        check_file(path)
        count += 1
    if count == 0:
        raise RuntimeError("no tracked Python sources were validated")
    print(f"Validated four-space code indentation and syntax in {count} tracked Python files")


if __name__ == "__main__":
    main()
