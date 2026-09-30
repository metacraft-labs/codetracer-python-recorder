"""Sample program for the launcher <-> recorder compatibility E2E.

WHAT THIS IS FOR
    ``codetracer/ci/test/launcher-recorder-e2e.sh`` records this file through
    the REAL ``ct`` launcher binary:

        ct record launcher_compat_sample.py -o <trace-dir>
          -> codetracer-launcher routes `.py` from the codetracer-desktop
             capability file and execv()s the desktop core
             -> the core dispatches codetracer-python-recorder
                -> the recorder writes a CTFS trace
                   -> `ct-print` (codetracer-trace-format-nim) decodes it

    The driver then asserts the DECODED trace against the expectations
    declared in ``cross-repo/launcher-compat.yml``, so everything this file
    prints or calls is part of a checked contract.  Changing a function name
    or a printed line here means changing that file in the same commit.

WHY IT PRINTS ``CODETRACER_COMPONENT_DIR``
    ``CODETRACER_COMPONENT_DIR`` is exported by the LAUNCHER and by nothing
    else on this path (codetracer-launcher/src/launcher.nim sets it right
    before ``execv``-ing the component's binary).  Seeing it inside the
    recorded trace's stdout is therefore positive evidence that the recording
    really travelled launcher -> desktop core -> recorder, rather than the
    driver having invoked the core (or the recorder) directly.  A test that
    only checked "a trace appeared" could not tell those apart.

KEEP THIS PROGRAM BORING
    Fixed inputs, deterministic output, no clock, no network, no randomness,
    no third-party imports.  The trace it produces is compared against exact
    expectations; anything non-deterministic would make the gate flaky.
"""

import os

MARKER = "launcher-recorder-e2e"

# Fixed inputs -- the expected sum below is asserted by the driver.
VALUES = (1, 2, 3, 4, 5)


def accumulate(values):
    """Sum ``values`` with an explicit loop so the trace has real steps."""
    total = 0
    for value in values:
        total = total + value
    return total


def describe_launcher_route():
    """Report the component directory the launcher exported for this run."""
    return os.environ.get("CODETRACER_COMPONENT_DIR", "<unset>")


def main():
    total = accumulate(VALUES)
    print(MARKER + ": total=" + str(total))
    print(MARKER + ": component-dir=" + describe_launcher_route())
    return total


if __name__ == "__main__":
    main()
