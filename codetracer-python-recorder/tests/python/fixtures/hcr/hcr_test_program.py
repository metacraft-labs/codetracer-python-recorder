#!/usr/bin/env python3
"""HCR test program: runs 12 iterations, reloads module at step 7."""
import sys
import importlib
import shutil
import os

sys.path.insert(0, os.path.dirname(__file__))
import mymodule

counter = 0
history = []

for i in range(12):
    counter += 1
    if counter == 7:
        shutil.copy(
            os.path.join(os.path.dirname(__file__), "mymodule_v2.py"),
            os.path.join(os.path.dirname(__file__), "mymodule.py")
        )
        importlib.reload(mymodule)
        print("RELOAD_APPLIED", flush=True)
    value = mymodule.compute(counter)
    delta = mymodule.transform(value, counter)
    history.append(delta)
    total = mymodule.aggregate(history)
    print(f"step={counter} value={value} delta={delta} total={total}", flush=True)
