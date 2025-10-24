Implement full variable capture in codetracer-python-recorder. Add a
comprehensive test suite. Here is the spec for the task and the tests:

# Python Tracing Recorder: Capturing All Visible Variables at Each Line

## Overview of Python Variable Scopes

In CPython, the accessible variables at a given execution point consist of:

* Local variables of the current function or code block (including parameters).

* Closure (nonlocal) variables that come from enclosing functions (if any).

* Global variables defined at the module level (the current module’s namespace).

(Built-ins are also always accessible if not shadowed, but they are usually not included in “visible variables” snapshots for tracing.)

Each executing frame in CPython carries these variables in its namespace. To capture a snapshot of all variables accessible at a line, we need to inspect the frame’s environment, combining locals, nonlocals, and globals. This must work for any code construct (functions, methods, comprehensions, class bodies, etc.) under CPython 3.12 and 3.13.

## Using the CPython C API (via PyO3) to Get Variables

1. **Access the current frame**: The sys.monitoring API’s line event callback does not directly provide a frame object. We can obtain the current PyFrameObject via the C API. Using PyO3’s FFI, you can call:
* `PyEval_GetFrame()` - return current thread state's frame, NULL if no frame is executing
* `PyThreadState_GetFrame(PyThreadState *tstate)` - return a given thread state's frame, NULL if on frame is currently executing.
This yields the top-of-stack frame – if your callback is a C function, that should be the frame of the user code. If your callback is a Python function, you may need frame.f_back to get the user code’s frame.)

2. **Get all local and closure variables**: Once you have the `PyFrameObject *frame`, retrieve the frame’s local variables mapping. In Python 3.12+, `frame.f_locals` is a proxy that reflects both local variables and any closure (cell/free) variables with their current values. In C, you can use `PyFrame_GetLocals(frame)`

3. **Get global variables**: The frame’s globals are in `frame.f_globals`. You can obtain this dictionary via `PyFrame_GetGlobals(frame)`. This is the module’s global namespace.

4. Encode them in the trace state. You can use the function `encode_value` to encode each one of those variables in a format suitable for recording and then record them using the capabilities provided by `runtime_tracing` crate.

## Important Details and Edge Cases

* **Closure (free) variables**: In modern CPython, closure variables are handled seamlessly via the frame’s locals proxy. You do not need to separately fetch function.__closure__ or outer frame variables – the frame’s local mapping already includes free vars. The PEP for frame proxies explicitly states that each access to `frame.f_locals` yields a mapping of local and closure variable names to their current values. This ensures that in a nested function, variables from an enclosing scope (nonlocals) appear in the inner frame’s locals mapping (bound to the value in the closure cell).

* **Comprehensions and generators**: In Python 3, list comprehensions, generator expressions, and the like are implemented as separate function frames. The above approach still works since those have their own frames (with any needed closure variables included similarly). Just grab that frame’s locals and globals as usual.

* **Class bodies and module level**: A class body or module top-level code is executed in an unoptimized frame where `locals == globals` (module) or a new class namespace dict. You need to make sure that you don't record variables twice! Here's a sketch how to do this:
```rust
use pyo3::prelude::*;
use pyo3::ffi;
use std::ptr;

pub unsafe fn locals_is_globals_ffi(_py: Python<'_>, frame: *mut ffi::PyFrameObject) -> PyResult<bool> {
    // Ensure f_locals exists and is synced with fast-locals
    if ffi::PyFrame_FastToLocalsWithError(frame) < 0 {
        return Err(PyErr::fetch(_py));
    }
    let f_locals = (*frame).f_locals;
    let f_globals = (*frame).f_globals;
    Ok(!f_locals.is_null() && ptr::eq(f_locals, f_globals))
}
```

* **Builtins**: Typically, built-in names (from frame.f_builtins) are implicitly accessible if not shadowed, but they are usually not included in a variables snapshot. You should ignore the builtins

* **Name resolution order**: If needed, CPython 3.12 introduced PyFrame_GetVar(frame, name) which will retrieve a variable by name as the interpreter would – checking locals (including cells), then globals, then builtins. This could be used to fetch specific variables on demand. However, for capturing all variables, it’s more efficient to pull the mappings as described above rather than querying names one by one.


## Putting It Together

In your Rust/PyO3 tracing recorder, for each line event you can do something like:

* Get the current frame (`frame_obj`).

* Get the locals proxy via `PyFrame_GetLocals`. Iterate over each object, construct its representation via `encode_value` and then add it to the trace.

* If locals != globals, get the globals dict (`globals_dict = PyFrame_GetGlobals(frame_obj)`) and process it just like the locals


By using these facilities via PyO3, you can reliably capture all visible variables at each line of execution in your tracing recorder.

## References

Python C-API – Frame Objects: functions to access frame attributes (locals, globals, etc.).

PEP 667 – Frame locals proxy (Python 3.13): frame.f_locals now reflects local + cell + free variables’ values.

PEP 558 – Defined semantics for locals(): introduced Py


# Comprehensive Test Suite for Python Tracing Recorder

This test suite is designed to verify that a tracing recorder (using sys.monitoring and frame inspection) correctly captures all variables visible at each executable line of Python code. Each test covers a distinct scope or visibility scenario in Python. The tracer should record every variable that is in scope at that line, ensuring no visible name is missed. We include functions, closures, globals, class scopes, comprehensions, generators, exception blocks, and more, to guarantee full coverage of Python's LEGB (Local, Enclosing, Global, Built-in) name resolution rules.

Each test case below provides a brief description of what it covers, followed by a code snippet (Python script) that exercises that behavior. No actual tracing logic is included – we only show the source code whose execution should be monitored. The expectation is that at runtime, the tracer’s LINE event will fire on each line and the recorder will capture all variables accessible in that scope at that moment.

## 1. Simple Function: Parameters and Locals

**Scope**: This test focuses on a simple function with a parameter and local variables. It verifies that the recorder sees function parameters and any locals on each line inside the function. On entering the function, the parameter should be visible; as lines execute, newly assigned local variables become visible too. This ensures that basic function scope is handled.

```py
def simple_function(x):
    a = 1                 # Parameter x is visible; local a is being defined
    b = a + x             # Locals a, b and parameter x are visible (b defined this line)
    return a, b           # Locals a, b and x still visible at return


# Test the function
result = simple_function(5)
```

_Expected_: The tracer should capture x (parameter) and then a and b as they become defined in simple_function.

## 2. Nested Functions and Closure Variables (nonlocal)

**Scope**: This test covers nested functions, where an inner function uses a closure variable from its outer function. We verify that variables in the enclosing (nonlocal) scope are visible inside the inner function, and that the nonlocal statement allows the inner function to modify the outer variable. Both the outer function’s locals and the inner function’s locals (plus closed-over variables) should be captured appropriately.

```py
def outer_func(x):
    y = 1
    def inner_func(z):
        nonlocal y            # Declare y from outer_func as nonlocal
        w = x + y + z         # x (outer param), y (outer var), z (inner param), w (inner local)
        y = w                 # Modify outer variable y
        return w
    total = inner_func(5)     # Calls inner_func, which updates y
    return y, total           # y is updated in outer scope
result = outer_func(2)
```

_Expected_: Inside `inner_func`, the tracer should capture x, y (from outer scope), z, and w at each line. In `outer_func`, it should capture x, y, and later the returned total. This ensures enclosing scope variables are handled (nonlocal variables are accessible to nested functions).

## 3. Global and Module-Level Variables

**Scope**: This test validates visibility of module-level (global) variables. It defines globals and uses them inside a function, including modifying a global with the global statement. We ensure that at each line, global names are captured when in scope (either at the module level or when referenced inside a function).

```py
GLOBAL_VAL = 10
counter = 0

def global_test():
    local_copy = GLOBAL_VAL       # Access a global variable
    global counter
    counter += 1                  # Modify a global variable
    return local_copy, counter

# Use the function and check global effects
before = counter
result = global_test()
after = counter
```

_Expected_: The tracer should capture *GLOBAL_VAL* and counter as globals on relevant lines. At the module level, GLOBAL_VAL, counter, before, after, etc. are in the global namespace. Inside global_test(), it should capture local_copy and see GLOBAL_VAL as a global. The global counter declaration ensures counter is treated as global in that function and its updated value remains in the module scope.

## 4. Class Definition Scope and Metaclass

**Scope:** This test targets class definition bodies, including the effect of a metaclass. When a class body executes, it has a local namespace that becomes the class’s attribute dictionary. We verify that variables assigned in the class body are captured, and that references to those variables or to globals are handled. Additionally, we include a metaclass to ensure that class creation via a metaclass is also traced.

```python
CONSTANT = 42

class MetaCounter(type):
    count = 0
    def __init__(cls, name, bases, attrs):
        MetaCounter.count += 1            # cls, name, bases, attrs visible; MetaCounter.count updated
        super().__init__(name, bases, attrs)

class Sample(metaclass=MetaCounter):
    a = 10
    b = a + 5                            # uses class attribute a
    print(a, b, CONSTANT)               # can access class attrs a, b and global CONSTANT
    def method(self):
        return self.a + self.b

# After class definition, metaclass count should have incremented
instances = MetaCounter.count
```

**Expected:** Within `MetaCounter`, the tracer should capture class-level attributes like `count` as well as method parameters (`cls`, `name`, `bases`, `attrs`) during class creation. In `Sample`’s body, it should capture `a` once defined, then `b` and `a` on the next line, and even allow access to `CONSTANT` (a global) during class body execution. After definition, `Sample.a` and `Sample.b` exist as class attributes (not directly as globals outside the class). The tracer should handle the class scope like a local namespace for that block.

## 5. Lambdas and Comprehensions (List, Set, Dict, Generator)

**Scope:** This combined test covers lambda expressions and various comprehensions, each of which introduces an inner scope. We ensure the tracer captures variables inside these expressions, including any outer variables they close over and the loop variables within comprehensions. Notably, in Python 3, the loop variable in a comprehension is local to the comprehension and not visible outside.

Lambda: Tests an inline lambda function with its own parameter and expression.

List Comprehension: Uses a loop variable internally and an external variable.

Set & Dict Comprehensions: Similar scope behavior with their own loop variables.

Generator Expression: A generator comprehension that lazily produces values.

```python
factor = 2
double = lambda y: y * factor                      # 'y' is local parameter, 'factor' is captured from outer scope

squares = [n**2 for n in range(3)]                 # 'n' is local to comprehension, not visible after
scaled_set = {n * factor for n in range(3)}        # set comprehension capturing outer 'factor'
mapping = {n: n*factor for n in range(3)}          # dict comprehension with local n
gen_exp = (n * factor for n in range(3))           # generator expression (lazy evaluated)
result_list = list(gen_exp)                        # force generator to evaluate
```

**Expected:** Inside the lambda, `y` (parameter) and `factor` (enclosing variable) are visible to the tracer. In each comprehension, the loop variable (e.g., `n`) and any outer variables (`factor`) should be captured during the comprehension's execution. After the comprehension, the loop variable is no longer defined (e.g., `n` is not accessible outside the list comprehension). The generator expression has a similar scope to a comprehension; its variables should be captured when it's iterated. All these ensure the recorder handles anonymous function scopes and comprehension internals.

## 6. Generators and Coroutines (async/await)

**Scope:** This test covers a generator function and an async coroutine function. Generators use yield to produce values and suspend execution, while async coroutines use await. We ensure that local variables persist across yields/awaits and remain visible when execution resumes (on each line hit). This verifies that the tracer captures the state in suspended functions.

```python
def counter_gen(n):
    total = 0
    for i in range(n):
        total += i
        yield total        # At yield: i and total are visible and persisted across resumes
    return total

import asyncio
async def async_sum(data):
    total = 0
    for x in data:
        total += x
        await asyncio.sleep(0)   # At await: x and total persist in coroutine
    return total

# Run the generator
gen = counter_gen(3)
gen_results = list(gen)          # exhaust the generator

# Run the async coroutine
coroutine_result = asyncio.run(async_sum([1, 2, 3]))
```

**Expected:** In `counter_gen`, at each yield line the tracer should capture `i` and `total` (and after resumption, those values are still available). In `async_sum`, at the await line, `x` and `total` are captured and remain after the await. The tracer must handle the resumption of these functions (triggered by `PY_RESUME` events) and still see previously defined locals. This test ensures generator state and coroutine state do not lose any variables between pauses.

## 7. Try/Except/Finally and With Statement

**Scope:** This test combines exception handling blocks and context manager usage. It verifies that the tracer captures variables introduced in a try/except flow (including the exception variable, which has a limited scope) as well as in a with statement context manager. We specifically ensure the exception alias is only visible inside the except block, and that variables from try, else, and finally blocks, as well as the with target, are all accounted for.

```python
def exception_and_with_demo(x):
    try:
        inv = 10 / x                       # In try: 'inv' defined if no error
    except ZeroDivisionError as e:
        error_msg = f"Error: {e}"          # In except: 'e' (exception) and 'error_msg' are visible
    else:
        inv += 1                           # In else: 'inv' still visible here
    finally:
        final_flag = True                  # In finally: 'final_flag' visible (e is out of scope here)

    with open(__file__, 'r') as f:
        first_line = f.readline()          # Inside with: 'f' (file handle) and 'first_line' visible
    return locals()  # return all locals for inspection

# Execute with a case that triggers exception and one that does not
result1 = exception_and_with_demo(0)       # triggers ZeroDivisionError
result2 = exception_and_with_demo(5)       # normal execution
```

**Expected:** In the except block, the tracer should capture the exception object name (`e`) and any locals like `error_msg`, but after the block `e` goes out of scope (no longer in `locals()`). The else block runs when no exception, and the tracer sees `inv` there. The finally block executes in both cases, with `final_flag` visible. During the with block, the tracer captures the context manager’s target (`f`) and any inner variables (`first_line`). This test ensures all branches of try/except/else/finally and the scope entering/exiting a with are handled.

## 8. Decorators and Function Wrappers

**Scope:** This test involves function decorators, which themselves often use closures. We have a decorator that closes over a free variable and wraps a function. The goal is to ensure that when the decorated function is defined and called, the tracer captures variables both in the decorator’s scope and in the wrapped function’s scope. This covers the scenario of variables visible during decoration and invocation.

```python
setting = "Hello"

def my_decorator(func):
    def wrapper(*args, **kwargs):
        # Inside wrapper: 'args', 'kwargs', and 'setting' from outer scope are visible
        print("Decorator wrapping with setting:", setting)
        return func(*args, **kwargs)
    return wrapper

@my_decorator
def greet(name):
    message = f"Hi, {name}"     # Inside greet: 'name' and 'message' are locals
    return message

# Call the decorated function
output = greet("World")
```

**Expected:** When defining `greet`, the decorator `my_decorator` is applied. The tracer should capture that process: inside `my_decorator`, the `func` parameter and the outer variable `setting` are visible. Within `wrapper`, on each call, `args`, `kwargs`, and the closed-over `setting` are visible to the tracer. Inside `greet`, normal function locals apply (`name`, `message`). This test ensures decorated functions don’t hide any variables from the tracer (it must trace through the decorator and the function execution).

## 9. Dynamic Execution (eval and exec)

**Scope:** This test checks dynamic creation and access of variables using `eval()` and `exec()`. The recorder should capture variables introduced by an exec at the moment they become available, as well as usage of variables via eval strings. We ensure that even dynamically created names or accessed names are seen by the tracer just like normal variables.

```python
expr_code = "dynamic_var = 99"
exec(expr_code)                          # Executes code, defining a new variable dynamically
check = dynamic_var + 1                  # Uses the dynamically created variable

def eval_test():
    value = 10
    formula = "value * 2"
    result = eval(formula)              # 'value' (local) is accessed dynamically via eval
    return result
out = eval_test()
```

**Expected:** At the `exec(expr_code)` line, the tracer should capture that `dynamic_var` gets created in the global scope. On the next line, `dynamic_var` is visible and used. Inside `eval_test()`, when `eval(formula)` is executed, the tracer should see the local `value` (and `formula`) in that frame, confirming that eval could access `value`. All dynamically introduced or accessed names should be recorded as they appear.

## 10. Import Statements and Visibility

**Scope:** This test covers the effect of import statements on variable visibility. Importing modules or names introduces new variables (module objects or imported names) into the local or global namespace. We test both a global import and a local (within-function) import to ensure the tracer captures these names when they become available.

```python
import math                         # Import at module level introduces 'math' in globals

def import_test():
    import os                      # Import inside function introduces 'os' as a local name
    constant = math.pi             # Can use global import inside function
    cwd = os.getcwd()              # Uses the locally imported module
    return constant, cwd

val, path = import_test()
```

**Expected:** After the top-level import `math`, the tracer should list `math` as a new global variable. Inside `import_test()`, after the `import os` line, `os` should appear as a local variable in that function’s scope. The usage of `math.pi` shows that globals remain accessible in the function, and the use of `os.getcwd()` confirms `os` is in the local namespace. This test ensures imported names are captured at the appropriate scope (global or local) when they are introduced.

## 11. Built-in Scope (Builtins)

**Scope:** This test highlights built-in names, which are always available via Python’s built-in scope (e.g., `len`, `print`, `ValueError`). The tracer is not required to explicitly list all built-ins at each line (as that would be overwhelming), but we include this case to note that built-in functions or constants are accessible in any scope. We ensure usage of a built-in is traced like any other variable access, although the recorder 
may choose not to list the entire built-in namespace.

```python
def builtins_test(seq):
    n = len(seq)            # 'len' is a built-in function
    m = max(seq)            # 'max' is another built-in
    return n, m

result = builtins_test([5, 3, 7])
```

**Expected:** In the `builtins_test` function, calls to `len` and `max` are made. The tracer would see `seq`, `n`, and `m` as local variables, and while `len`/`max` are resolved from the built-in scope, the recorder may not list them as they are implicitly available (built-ins are found after global scope in name resolution). The important point is that using built-ins does not introduce new names in the user-defined scopes. This test is mostly a note that built-in scope exists and built-in names are always accessible (the tracer could capture them, but it's typically unnecessary to record every built-in name).

---

**Conclusion:** The above tests collectively cover all major visibility scenarios in Python. By running a tracing recorder with these snippets, one can verify that at every executable line, the recorder correctly identifies all variables that are in scope (function locals, closure variables, globals, class locals, comprehension temporaries, exception variables, etc.). This comprehensive coverage ensures the tracing tool is robust against Python’s various scoping rules and constructs.

# General Rules

* This spec is for `/codetracer-python-recorder` project and NOT for `/codetracer-pure-python-recorder`
* Code and tests should be added under `/codetracer-python-recorder/src/runtime/tracer/` (primarily `runtime_tracer.rs` and its collaborators)
* Performance is important. Avoid using Python modules and functions and prefer PyO3 methods including the FFI API.
* If you want to run Python do it like so `uv run python` This will set up the right venv. Similarly for running tests `uv run pytest`.
* After every code change you need to run `just dev` to make sure that you are testing the new code. Otherwise some tests might run against the old code

* Avoid defensive programming: when encountering edge cases which are
  not explicitly mentioned in the specification, the default behaviour
  should be to crash (using `panic!`). We will only handle them after
  we receive a report from a user which confirms that the edge case
  does happen in real life.
* Do not make any code changes to unrelated parts of the code. The only callback that should change behaviour is `on_line`
* If the code has already implemented part of the specification described here find out what is missing and implement that
* If a test fails repeatedly after three attempts to fix the code STOP. Let a human handle it. DON'T DELETE TESTS!!!
* When writing tests be careful with concurrency. If two tests run at the same time using the same Python interpreter (or same Rust process?) they will both try to register callbacks via sys.monitoring and could deadlock.
* If you want to test Rust code without using just, use `cargo nextest`, not `cargo test`
