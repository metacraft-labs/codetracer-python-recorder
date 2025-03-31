## codetracer-python-recorder

An unfinished prototype of a recorder of Python programs that produces [CodeTracer](https://github.com/metacraft-labs/CodeTracer) traces.

> [!WARNING]
> Currently it is in a very early phase: we're welcoming contribution and discussion!


### usage

you can currently use it directly with

```bash
python trace.py <path to python file>
# produces several trace json files in the current directory
# or in the folder of `$CODETRACER_DB_TRACE_PATH` if such an env var is defined
```

however you probably want to use it in combination with CodeTracer, which would be released soon.

## future directions

The current Python support is an unfinished prototype. We can finish it. In the future, it may be expanded to function in a way to similar to the more complete implementations, e.g. [Noir](https://github.com/blocksense-network/noir/tree/blocksense/tooling/tracer).

Currently it's very similar to our [Ruby tracer](https://github.com/metacraft-labs/ct-ruby-tracer)

### Current approach: sys.settrace API

Currently we're using the sys.settrace API: https://docs.python.org/3/library/sys.html#sys.settrace .
This is very flexible and can function with probably multiple Python versions out of the box. 
However, this is limited:

* it's not optimal
* it can't track more detailed info/state, needed for some CodeTracer features(or for more optimal replays).

For other languages, we've used a more deeply integrated approach: patching the interpreter or VM itself (e.g. Noir).

### Patching the VM

This can be a good approach for Python as well: it can let us record more precisely subvalues, assignments and subexpressions and to let
some CodeTracer features work in a deeper/better way.

One usually needs to add additional logic to places where new opcodes/lines are being ran, and to call entries/exits. Additionally
tracking assignments can be a great addition, but it really depends on the interpreter internals.

### Filtering

It would be useful to have a way to record in detail only certain periods of the program, or certain functions or modules: 
we plan on expanding the [trace format](https://github.com/metacraft-labs/runtime_tracing/) and CodeTracer' support, so that this is possible. It would let one be able to record interesting
parts of even long-running or more heavy programs.

### Cooperation

We'd be very happy if the community finds this useful, and if anyone wants to

* Cooperate with us on supporting/advancing the Python support or CodeTracer 
* Contribute Python support or anything else to this trace or to [CodeTracer](https://github.com/metacraft-labs/CodeTracer)
* Just discuss various ideas with us: here, in the issue tracker, or in our [discord](https://discord.gg/qSDCAFMP)
* Use and test the ruby support or CodeTracer

### Legal info

LICENSE: MIT

Copyright (c) 2025 Metacraft Labs Ltd
