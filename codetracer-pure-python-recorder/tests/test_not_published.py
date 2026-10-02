"""The pure-Python recorder is a test oracle and must never reach PyPI.

PyPI refuses an upload whose metadata carries the ``Private :: Do Not
Upload`` classifier, so the classifier in ``pyproject.toml`` is what stops
an accidental ``twine upload`` or a new release workflow from publishing it.
"""

import tomllib
import unittest
from pathlib import Path

PYPROJECT = Path(__file__).resolve().parent.parent / "pyproject.toml"


class NotPublishedTests(unittest.TestCase):
    def test_pypi_refuses_the_package(self):
        with open(PYPROJECT, "rb") as f:
            project = tomllib.load(f)["project"]
        self.assertIn(
            "Private :: Do Not Upload",
            project.get("classifiers", []),
            "the test oracle must carry the classifier PyPI refuses uploads with",
        )


if __name__ == "__main__":
    unittest.main()
